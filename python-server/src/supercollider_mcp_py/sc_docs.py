from __future__ import annotations

import json
import re
import threading
from dataclasses import dataclass
from pathlib import Path
from time import time

from . import sc_process

QUERY_SYNONYMS: tuple[tuple[str, tuple[str, ...]], ...] = (
    ("guitar", ("pluck", "dwgpluck", "karplus", "string", "physical", "model")),
    ("synthesizer", ("synthdef", "ugen", "synth")),
    ("synthesiser", ("synthdef", "ugen", "synth")),
    ("envelope", ("env", "envgen", "doneaction")),
    ("sample", ("playbuf", "bufnum", "buffer")),
    ("midi", ("midiin", "noteon", "midifunc")),
    ("sequencer", ("pattern", "pevent", "pbind")),
    ("drum", ("perc", "trig", "impulse", "decay2")),
)


@dataclass(slots=True)
class DocChunk:
    source_path: str
    title: str
    section: str
    content: str
    content_lower: str
    cross_ref_lower: str


@dataclass(slots=True)
class DocsIndex:
    root: Path
    built_at_epoch_s: float
    chunks: list[DocChunk]
    files_indexed: int


DOCS_INDEX: DocsIndex | None = None
DOCS_LOCK = threading.Lock()


def normalize_ws(text: str) -> str:
    return re.sub(r"\s+", " ", text).strip()


def strip_html_tags(text: str) -> str:
    return re.sub(r"<[^>]+>", "", text)


def split_sections(text: str, max_chars: int) -> list[str]:
    sections: list[str] = []
    cur = ""
    for para in text.split("\n\n"):
        p = para.strip()
        if not p:
            continue
        if len(cur) + len(p) + 2 > max_chars and cur:
            sections.append(cur.strip())
            cur = ""
        if cur:
            cur += "\n\n"
        cur += p
    if cur.strip():
        sections.append(cur.strip())
    return sections


def title_from_path(path: Path) -> str:
    return path.stem or "untitled"


def is_doc_file(path: Path) -> bool:
    return path.suffix.lower() in {".schelp", ".html", ".htm", ".md", ".txt"}


def parse_schelp_header_line(line: str) -> tuple[str, str] | None:
    t = line.lstrip()
    if "::" not in t:
        return None
    key, rest = t.split("::", 1)
    key = key.strip()
    if not key:
        return None
    if not re.match(r"^[A-Za-z_][A-Za-z0-9_]*$", key):
        return None
    return key.lower(), rest.lstrip()


def split_schelp_sections(raw: str) -> list[tuple[str, str]]:
    sections: list[tuple[str, str]] = []
    cur_key = "preamble"
    cur_body: list[str] = []

    for line in raw.splitlines():
        parsed = parse_schelp_header_line(line)
        if parsed is not None:
            if not (cur_key == "preamble" and not "".join(cur_body).strip()):
                sections.append((cur_key, "\n".join(cur_body)))
            key, rest = parsed
            cur_key = key
            cur_body = [rest] if rest else []
            continue
        cur_body.append(line)

    sections.append((cur_key, "\n".join(cur_body)))
    return sections


def link_tokens_from_segment(path: str, acc: set[str]) -> None:
    p = path.strip()
    if not p:
        return
    for part in re.split(r"[/\\]", p):
        token = part.strip().lower()
        if len(token) >= 2:
            acc.add(token)
    stem = re.split(r"[/\\]", p)[-1].strip().lower()
    if len(stem) >= 2:
        acc.add(stem)


def collect_schelp_cross_ref_tokens(raw: str) -> str:
    acc: set[str] = set()

    for m in re.finditer(r"link::(.*?)::", raw, flags=re.DOTALL):
        link_tokens_from_segment(m.group(1), acc)

    for line in raw.splitlines():
        t = line.lstrip()
        if not t.startswith("related::"):
            continue
        body = t[len("related::") :]
        for part in body.split(","):
            link_tokens_from_segment(part.strip().removesuffix(".schelp"), acc)

    return " ".join(sorted(acc))


def expand_help_links_inline(line: str) -> str:
    def repl(match: re.Match[str]) -> str:
        p = match.group(1).strip()
        stem = re.split(r"[/\\]", p)[-1].strip()
        return (stem if stem else p) + " "

    return re.sub(r"link::(.*?)::", repl, line)


def strip_schelp_list_item_prefix(line: str) -> str:
    t = line.lstrip()
    if t.startswith("##"):
        return t[2:].lstrip()
    return line


def simple_strip_code_markers(text: str) -> str:
    def repl(match: re.Match[str]) -> str:
        return match.group(1).strip()

    return re.sub(r"code::(.*?)::", repl, text)


def strip_schelp_markup_for_index(text: str) -> str:
    out: list[str] = []
    in_code = False

    for line in text.splitlines():
        t = line.strip()
        if t == "code::":
            in_code = True
            out.append("")
            continue
        if in_code and t == "::":
            in_code = False
            out.append("")
            continue
        if in_code:
            out.append(line)
            continue

        prose = strip_schelp_list_item_prefix(line)
        prose = expand_help_links_inline(prose)
        prose = simple_strip_code_markers(prose)
        out.append(prose)

    return normalize_ws("\n".join(out))


def min_chunk_len(section: str) -> int:
    if section == "examples" or "example" in section:
        return 20
    return 40


def push_chunks_from_text(
    chunks: list[DocChunk],
    source_path: str,
    title: str,
    section: str,
    text: str,
    cross_ref_lower: str,
) -> None:
    normalized = normalize_ws(text)
    if len(normalized) < min_chunk_len(section):
        return

    max_chars = 1200 if (section == "examples" or "example" in section) else 900
    for sec in split_sections(normalized, max_chars):
        if len(sec) < min_chunk_len(section):
            continue
        chunks.append(
            DocChunk(
                source_path=source_path,
                title=title,
                section=section,
                content=sec,
                content_lower=sec.lower(),
                cross_ref_lower=cross_ref_lower,
            )
        )


def index_file(path: Path, root: Path, chunks: list[DocChunk]) -> None:
    try:
        raw = path.read_text(encoding="utf-8")
    except UnicodeDecodeError:
        raw = path.read_text(encoding="latin-1", errors="ignore")
    except OSError:
        return

    rel = str(path.relative_to(root)) if path.is_relative_to(root) else str(path)
    title = title_from_path(path)

    if path.suffix.lower() == ".schelp":
        for sec_key, body in split_schelp_sections(raw):
            cross = collect_schelp_cross_ref_tokens(body)
            clean = strip_schelp_markup_for_index(body)
            push_chunks_from_text(chunks, rel, title, sec_key, clean, cross)
        return

    text = strip_html_tags(raw) if path.suffix.lower() in {".html", ".htm"} else raw
    text = normalize_ws(text)
    if len(text) < 40:
        return
    push_chunks_from_text(chunks, rel, title, "body", text, "")


def walk_docs(root: Path) -> list[Path]:
    files: list[Path] = []
    if not root.exists():
        return files
    for p in root.rglob("*"):
        if p.is_file() and is_doc_file(p):
            files.append(p)
    return files


def help_root() -> Path | None:
    base = sc_process.detect_install_base_dir()
    if not base:
        return None

    candidates = [
        base / "Help",
        base / "HelpSource",
        base / "share" / "SuperCollider" / "Help",
        base / "share" / "SuperCollider" / "HelpSource",
    ]

    if base.name.lower() == "bin":
        parent = base.parent
        candidates.extend(
            [
                parent / "Help",
                parent / "HelpSource",
                parent / "share" / "SuperCollider" / "Help",
                parent / "share" / "SuperCollider" / "HelpSource",
            ]
        )
    else:
        candidates.extend(
            [
                base / "bin" / "Help",
                base / "bin" / "HelpSource",
                base / "bin" / "share" / "SuperCollider" / "Help",
                base / "bin" / "share" / "SuperCollider" / "HelpSource",
            ]
        )

    for c in candidates:
        if c.exists():
            return c
    return None


def count_doc_files_under(root: Path) -> int:
    return len(walk_docs(root))


def build_index_impl() -> DocsIndex:
    root = help_root()
    if not root:
        raise RuntimeError(
            "SuperCollider Help directory not found from detected install path. "
            "Run detect_supercollider_install and get_server_docs to inspect discovered paths."
        )

    files = sorted(walk_docs(root))
    chunks: list[DocChunk] = []
    for path in files:
        index_file(path, root, chunks)

    if not chunks:
        raise RuntimeError("No docs content indexed from Help directory.")

    return DocsIndex(root=root, built_at_epoch_s=time(), chunks=chunks, files_indexed=len(files))


def ensure_index(force_refresh: bool) -> DocsIndex:
    global DOCS_INDEX
    with DOCS_LOCK:
        if force_refresh or DOCS_INDEX is None:
            DOCS_INDEX = build_index_impl()
        return DOCS_INDEX


def expand_synonyms(tokens: set[str]) -> None:
    add: set[str] = set()
    for t in list(tokens):
        for key, extras in QUERY_SYNONYMS:
            if t == key:
                add.update(extras)
    tokens.update(add)


def tokenize(query: str) -> list[str]:
    tokens = {t for t in re.split(r"[^A-Za-z0-9_]", query.lower()) if len(t.strip()) >= 2}
    expand_synonyms(tokens)
    return sorted(tokens)


def synonym_note(query: str) -> str:
    lowered = query.lower()
    parts: list[str] = []
    for key, extras in QUERY_SYNONYMS:
        if key in lowered:
            parts.append(f"{key}->{','.join(extras)}")
    return "none" if not parts else "; ".join(parts)


def score_chunk(query_tokens: list[str], chunk: DocChunk) -> int:
    score = 0
    sec_lower = chunk.section.lower()
    examples_boost = "example" in sec_lower

    title_lower = chunk.title.lower()
    source_lower = chunk.source_path.lower()

    for token in query_tokens:
        if token in chunk.content_lower:
            score += 4
        if token in title_lower:
            score += 6
        if token in source_lower:
            score += 3
        if token in sec_lower:
            score += 2
        if chunk.cross_ref_lower and token in chunk.cross_ref_lower:
            score += 5

    if examples_boost:
        score += 2
    if sec_lower == "list":
        score += 1
    return score


def clip_for_answer(text: str, max_chars: int) -> str:
    if len(text) <= max_chars:
        return text
    return text[:max_chars] + " ..."


def resolve_docs_source(source: str | None) -> None:
    s = (source or "local").lower().strip()
    if s in {"", "local", "auto"}:
        return
    if s == "web":
        raise ValueError("docs source=`web` is not implemented (offline-first). Use `local` or omit.")
    raise ValueError(f"unknown docs source `{s}`; use `local` (default) or `auto`.")


def get_docs_index_status() -> str:
    resolved_root = help_root()
    disk_files = count_doc_files_under(resolved_root) if resolved_root else 0

    with DOCS_LOCK:
        idx = DOCS_INDEX

    lines = ["SuperCollider docs index status"]
    if resolved_root:
        lines.append(f"- help_root_resolved={resolved_root}")
        lines.append(f"- doc_files_on_disk={disk_files}")
    else:
        lines.append("- help_root_resolved=<not found>")
        lines.append("- doc_files_on_disk=0")

    if idx is not None:
        lines.append("- index_loaded=true")
        lines.append(f"- index_root={idx.root}")
        lines.append(f"- files_indexed={idx.files_indexed}")
        lines.append(f"- chunks={len(idx.chunks)}")
        lines.append(f"- built_at={idx.built_at_epoch_s}")
        if resolved_root and resolved_root != idx.root:
            lines.append(
                "- note: resolved help root differs from index root; "
                "call refresh_supercollider_docs_index if install moved."
            )
    else:
        lines.append("- index_loaded=false")
        lines.append("- hint: call refresh_supercollider_docs_index to build the in-memory index.")

    lines.append("- query_synonyms_active=true")
    return "\n".join(lines)


def refresh_supercollider_docs_index() -> str:
    try:
        idx = ensure_index(True)
    except Exception as exc:
        return f"refresh_supercollider_docs_index failed: {exc}"

    return (
        "Docs index refreshed.\n"
        f"- help_root={idx.root}\n"
        f"- files_indexed={idx.files_indexed}\n"
        f"- chunks={len(idx.chunks)}"
    )


def search_supercollider_docs(
    query: str,
    max_results: int = 5,
    output: str = "text",
    source: str | None = None,
) -> str:
    try:
        resolve_docs_source(source)
    except Exception as exc:
        return f"search_supercollider_docs failed: {exc}"

    if not query.strip():
        return "search_supercollider_docs: query is empty."

    try:
        idx = ensure_index(False)
    except Exception as exc:
        return f"search_supercollider_docs failed: {exc}"

    tokens = tokenize(query)
    if not tokens:
        return "search_supercollider_docs: query has no searchable tokens."

    scored: list[tuple[int, int]] = []
    for i, chunk in enumerate(idx.chunks):
        s = score_chunk(tokens, chunk)
        if s > 0:
            scored.append((s, i))
    scored.sort(key=lambda t: t[0], reverse=True)

    take_n = max(1, min(max_results, 12))
    json_mode = output.lower() in {"json", "structured"}

    if json_mode:
        hits = []
        for rank, (score, i) in enumerate(scored[:take_n], start=1):
            ch = idx.chunks[i]
            hits.append(
                {
                    "rank": rank,
                    "score": score,
                    "title": ch.title,
                    "section": ch.section,
                    "source_path": ch.source_path,
                    "excerpt": clip_for_answer(ch.content, 420),
                }
            )

        envelope = {
            "format_version": 1,
            "query": query,
            "synonym_expansions_note": synonym_note(query),
            "index_loaded": True,
            "help_root": str(idx.root),
            "files_indexed": idx.files_indexed,
            "chunk_count": len(idx.chunks),
            "built_at": str(idx.built_at_epoch_s),
            "results": hits,
        }
        return json.dumps(envelope, indent=2)

    lines = [
        f"SuperCollider docs search results for: {query}",
        f"- synonym_hints={synonym_note(query)}",
        (
            f"- help_root={idx.root} files_indexed={idx.files_indexed} "
            f"chunks={len(idx.chunks)} built_at={idx.built_at_epoch_s}"
        ),
    ]

    if not scored:
        lines.append("- No matching docs sections found.")
        lines.append("- Try simpler terms (example: SynthDef, Node, Server, UGen).")
        return "\n".join(lines)

    for rank, (score, i) in enumerate(scored[:take_n], start=1):
        ch = idx.chunks[i]
        lines.append("")
        lines.append(
            f"[{rank}] score={score} title={ch.title} section={ch.section} source={ch.source_path}"
        )
        lines.append(clip_for_answer(ch.content, 420))

    return "\n".join(lines)


def answer_supercollider_docs(question: str) -> str:
    if not question.strip():
        return "answer_supercollider_docs: question is empty."

    try:
        idx = ensure_index(False)
    except Exception as exc:
        return f"answer_supercollider_docs failed: {exc}"

    tokens = tokenize(question)
    if not tokens:
        return "answer_supercollider_docs: question has no searchable tokens."

    scored: list[tuple[int, int]] = []
    for i, chunk in enumerate(idx.chunks):
        s = score_chunk(tokens, chunk)
        if s > 0:
            scored.append((s, i))
    scored.sort(key=lambda t: t[0], reverse=True)

    if not scored:
        return (
            f"No grounded docs answer found for: {question}\n"
            "Try search_supercollider_docs with broader keywords "
            "(synonym expansion is applied automatically)."
        )

    lines = [
        "Grounded answer from local SuperCollider docs:",
        f"- synonym_hints={synonym_note(question)}",
        "Evidence sections:",
    ]

    for rank, (_, i) in enumerate(scored[:3], start=1):
        ch = idx.chunks[i]
        lines.append(f"- [{rank}] {ch.title} [{ch.section}] ({ch.source_path})")
        lines.append(f"  {clip_for_answer(ch.content, 260)}")

    lines.append("")
    lines.append("Use the cited sources above as ground truth for sclang/API facts. ")
    lines.append(
        "For verbatim examples, open the source file or call search_supercollider_docs."
    )
    return "\n".join(lines)


def initialize_supercollider_session() -> str:
    """Run the full session-start sequence in one call:
    1. Detect install paths
    2. Check docs index status; refresh if not loaded
    3. Return routing hints so the LLM knows which tool to call for every intent.
    """
    parts: list[str] = ["=== SuperCollider session init ===\n"]

    # Step 1: install detection
    parts.append("--- [1/3] Install detection ---")
    try:
        parts.append(sc_process.detect_supercollider_install())
    except Exception as exc:
        parts.append(f"detect_supercollider_install error: {exc}")

    # Step 2: docs index — refresh only if not already loaded
    parts.append("\n--- [2/3] Docs index ---")
    try:
        with DOCS_LOCK:
            already_loaded = DOCS_INDEX is not None
        if already_loaded:
            parts.append("index_loaded=true (skipping rebuild)")
            parts.append(get_docs_index_status())
        else:
            parts.append("index_loaded=false — building index now …")
            parts.append(refresh_supercollider_docs_index())
    except Exception as exc:
        parts.append(f"docs index error: {exc}")

    # Step 3: routing hints
    parts.append("\n--- [3/3] Tool routing hints ---")
    parts.append(get_mcp_tool_routing_hints())

    parts.append("\n=== Session ready ===")
    return "\n".join(parts)


def get_mcp_tool_routing_hints() -> str:
    return (
        "SuperCollider MCP - tool routing (offline docs)\n\n"
        "| User intent | Preferred tool(s) |\n"
        "|-------------|-------------------|\n"
        "| Is SC running? / server up? | ping_supercollider, get_server_status, get_servers |\n"
        "| Version / install path | get_supercollider_version, detect_supercollider_install |\n"
        "| Where are docs on disk? | get_server_docs, get_docs_index_status |\n"
        "| Connect to server / Server.remote / OSC | search_supercollider_docs or answer_supercollider_docs first; get_server_docs lists local Server-Command-Reference.schelp + Server.schelp |\n"
        "| Is the docs index warm? | get_docs_index_status |\n"
        "| (re)build docs index | refresh_supercollider_docs_index |\n"
        "| Search help for class/topic | search_supercollider_docs (use output=json for structured citations) |\n"
        "| Explain from help only | answer_supercollider_docs |\n"
        "| Validate sclang syntax (no audio) | check_sclang_syntax |\n"
        "| Run code on live scsynth (trusted host) | execute_supercollider_code (optional server_pid/osc_port from get_servers) |\n"
        "| Stop / silence all synths (default group, this client) | stop_supercollider_synths (uses `Server.default.freeAll` - not `Synth.freeAll`) |\n"
        "| Kill scsynth/supernova process (OSC /quit) | quit_supercollider_server |\n"
        "| Restart audio server process (OSC /quit then spawn -u port) | reboot_supercollider_server (`s.reboot` fails on Server.remote) |\n"
        "| Debug process detection | list_server_candidates, discover_supercollider |\n\n"
        "Docs tools use local Help/HelpSource only. source=web is not supported.\n\n"
        "Workflow: get_docs_index_status -> refresh_supercollider_docs_index (if needed) -> search or answer."
    )
