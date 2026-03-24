//! Local SuperCollider **documentation** pipeline: index on-disk help, search, optional JSON hits, grounded Q&A hints.
//!
//! - **Offline-first** — only `Help` / `HelpSource` under the detected install; `source=web` is rejected at the API layer.
//! - **In-memory cache** — one `Mutex<Option<DocsIndex>>`; refresh explicitly or on first search after startup.
//! - **`.schelp`** — section headers (`class::`, `Examples::`, …) drive chunk boundaries; `link::` / `related::` / `LIST::` (`##` items) enrich text and cross-ref scoring for retrieval.

use std::cmp::Reverse;
use std::collections::HashSet;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use serde::Serialize;

use crate::sc_process;

// --- Query expansion (token aliasing for plain-language queries) ---

/// Maps conversational terms to extra retrieval tokens (never hits the network).
const QUERY_SYNONYMS: &[(&str, &[&str])] = &[
    (
        "guitar",
        &["pluck", "dwgpluck", "karplus", "string", "physical", "model"],
    ),
    ("synthesizer", &["synthdef", "ugen", "synth"]),
    ("synthesiser", &["synthdef", "ugen", "synth"]),
    ("envelope", &["env", "envgen", "doneaction"]),
    ("sample", &["playbuf", "bufnum", "buffer"]),
    ("midi", &["midiin", "noteon", "midifunc"]),
    ("sequencer", &["pattern", "pevent", "pbind"]),
    ("drum", &["perc", "trig", "impulse", "decay2"]),
];

// --- Index record types ---

/// One searchable slice of a help file (section + body substring).
#[derive(Clone)]
struct DocChunk {
    source_path: String,
    title: String,
    /// Logical section from `.schelp` (e.g. `examples`, `description`) or `body` for other formats.
    section: String,
    content: String,
    content_lower: String,
    /// `link::` / `related::` targets from the raw section (lowercased, space-separated) for retrieval boosts.
    cross_ref_lower: String,
}

/// Whole help tree index at a point in time (cloned out of the mutex for search).
#[derive(Clone)]
struct DocsIndex {
    root: PathBuf,
    built_at: SystemTime,
    chunks: Vec<DocChunk>,
    files_indexed: usize,
}

static DOCS_INDEX: OnceLock<Mutex<Option<DocsIndex>>> = OnceLock::new();

fn docs_index_lock() -> &'static Mutex<Option<DocsIndex>> {
    DOCS_INDEX.get_or_init(|| Mutex::new(None))
}

// --- Text cleanup (whitespace, HTML, schelp `code::` / markers) ---

fn normalize_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_space = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !last_space {
                out.push(' ');
                last_space = true;
            }
        } else {
            out.push(ch);
            last_space = false;
        }
    }
    out.trim().to_string()
}

fn strip_html_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for ch in s.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

/// Split long prose into chunks; keeps paragraphs together until `max_chars`.
fn split_sections(text: &str, max_chars: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    for para in text.split("\n\n") {
        let para = para.trim();
        if para.is_empty() {
            continue;
        }
        if current.len() + para.len() + 2 > max_chars && !current.is_empty() {
            out.push(current.trim().to_string());
            current.clear();
        }
        if !current.is_empty() {
            current.push_str("\n\n");
        }
        current.push_str(para);
    }
    if !current.trim().is_empty() {
        out.push(current.trim().to_string());
    }
    out
}

fn title_from_path(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "untitled".to_string())
}

fn is_doc_file(path: &Path) -> bool {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => matches!(
            ext.to_ascii_lowercase().as_str(),
            "schelp" | "html" | "htm" | "md" | "txt"
        ),
        None => false,
    }
}

// --- `.schelp` line-oriented parser (`class::`, `Examples::`, nested `code::` blocks) ---

/// Parse a SuperCollider help line like `class:: SinOsc` or `Examples::`.
fn parse_schelp_header_line(line: &str) -> Option<(String, String)> {
    let t = line.trim_start();
    let idx = t.find("::")?;
    let key = t[..idx].trim();
    if key.is_empty() {
        return None;
    }
    let first = key.chars().next()?;
    if !(first.is_ascii_alphabetic() || first == '_') {
        return None;
    }
    if !key
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return None;
    }
    let rest = t[idx + 2..].trim_start();
    Some((key.to_lowercase(), rest.to_string()))
}

/// Split `.schelp` body into `(section_key, section_body)` preserving Examples blocks.
fn split_schelp_sections(raw: &str) -> Vec<(String, String)> {
    let mut sections: Vec<(String, String)> = Vec::new();
    let mut cur_key = "preamble".to_string();
    let mut cur_body = String::new();

    for line in raw.lines() {
        if let Some((key, rest)) = parse_schelp_header_line(line) {
            if !(cur_body.trim().is_empty() && cur_key == "preamble") {
                sections.push((cur_key.clone(), cur_body));
            }
            cur_key = key;
            cur_body = String::new();
            if !rest.is_empty() {
                cur_body.push_str(&rest);
                cur_body.push('\n');
            }
        } else {
            cur_body.push_str(line);
            cur_body.push('\n');
        }
    }
    sections.push((cur_key, cur_body));
    sections
}

/// Insert path tokens and stems from `link::path/to/Name::` into `acc`.
fn link_tokens_from_segment(path: &str, acc: &mut HashSet<String>) {
    let path = path.trim();
    if path.is_empty() {
        return;
    }
    for part in path.split(|c| c == '/' || c == '\\') {
        let p = part.trim().to_lowercase();
        if p.len() >= 2 {
            acc.insert(p);
        }
    }
    if let Some(stem) = path
        .rsplit(['/', '\\'])
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let st = stem.to_lowercase();
        if st.len() >= 2 {
            acc.insert(st);
        }
    }
}

/// Collect lowercase tokens from `link::…::` and comma-separated `related::` lines in raw `.schelp`.
fn collect_schelp_cross_ref_tokens(raw: &str) -> String {
    let mut acc = HashSet::new();
    let mut rest = raw;
    while let Some(i) = rest.find("link::") {
        rest = &rest[i + 6..];
        let Some(j) = rest.find("::") else { break };
        let path = &rest[..j];
        link_tokens_from_segment(path, &mut acc);
        rest = &rest[j + 2..];
    }
    for line in raw.lines() {
        let t = line.trim_start();
        let Some(body) = t.strip_prefix("related::") else {
            continue;
        };
        for part in body.split(',') {
            let mut p = part.trim();
            p = p.trim_end_matches(".schelp");
            link_tokens_from_segment(p, &mut acc);
        }
    }
    let mut v: Vec<_> = acc.into_iter().collect();
    v.sort();
    v.join(" ")
}

/// Turn `link::Classes/SinOsc::` into readable `SinOsc` (and path is already indexed via [`collect_schelp_cross_ref_tokens`]).
fn expand_help_links_inline(line: &str) -> String {
    let mut out = String::with_capacity(line.len() + 16);
    let mut rest = line;
    while let Some(i) = rest.find("link::") {
        out.push_str(&rest[..i]);
        rest = &rest[i + 6..];
        let Some(j) = rest.find("::") else {
            out.push_str("link::");
            out.push_str(rest);
            return out;
        };
        let path = rest[..j].trim();
        rest = &rest[j + 2..];
        if let Some(stem) = path.rsplit('/').next().or_else(|| path.rsplit('\\').next()) {
            let st = stem.trim();
            if !st.is_empty() {
                out.push_str(st);
                out.push(' ');
            }
        } else if !path.is_empty() {
            out.push_str(path);
            out.push(' ');
        }
    }
    out.push_str(rest);
    out
}

/// In `LIST::` sections, items often start with `##`.
fn strip_schelp_list_item_prefix<'a>(line: &'a str) -> &'a str {
    let t = line.trim_start();
    match t.strip_prefix("##") {
        Some(rest) => rest.trim_start(),
        None => line,
    }
}

fn strip_schelp_markup_for_index(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_code = false;
    for line in text.lines() {
        let t = line.trim();
        if t == "code::" {
            in_code = true;
            out.push('\n');
            continue;
        }
        if in_code && t == "::" {
            in_code = false;
            out.push('\n');
            continue;
        }
        if in_code {
            out.push_str(line);
            out.push('\n');
        } else {
            let prose = strip_schelp_list_item_prefix(line);
            let expanded = expand_help_links_inline(prose);
            out.push_str(&simple_strip_code_markers(&expanded));
            out.push('\n');
        }
    }
    normalize_ws(&out)
}

/// Strip inline `code::...::` and `link::` markers from prose lines.
fn simple_strip_code_markers(s: &str) -> String {
    let mut r = s.to_string();
    loop {
        let Some(i) = r.find("code::") else { break };
        let after_start = i + 6;
        if after_start > r.len() {
            break;
        }
        let Some(j) = r[after_start..].find("::") else {
            break;
        };
        let inner = r[after_start..after_start + j].trim().to_string();
        let end = after_start + j + 2;
        r.replace_range(i..end, &inner);
    }
    r
}

fn read_raw_doc(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

fn is_schelp(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("schelp"))
}

/// Minimum body length for a chunk (Examples can be shorter if code-dense).
fn min_chunk_len(section: &str) -> usize {
    if section == "examples" || section.contains("example") {
        20
    } else {
        40
    }
}

fn push_chunks_from_text(
    chunks: &mut Vec<DocChunk>,
    source_path: &str,
    title: &str,
    section: &str,
    text: &str,
    cross_ref_lower: &str,
) {
    let text = normalize_ws(text);
    if text.len() < min_chunk_len(section) {
        return;
    }
    let max_chars = if section == "examples" || section.contains("example") {
        1200
    } else {
        900
    };
    for sec in split_sections(&text, max_chars) {
        if sec.len() < min_chunk_len(section) {
            continue;
        }
        chunks.push(DocChunk {
            source_path: source_path.to_string(),
            title: title.to_string(),
            section: section.to_string(),
            content_lower: sec.to_lowercase(),
            content: sec,
            cross_ref_lower: cross_ref_lower.to_string(),
        });
    }
}

fn index_file(path: &Path, root: &Path, chunks: &mut Vec<DocChunk>) {
    let Some(raw) = read_raw_doc(path) else {
        return;
    };
    let rel = path
        .strip_prefix(root)
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| path.to_string_lossy().to_string());
    let title = title_from_path(path);

    if is_schelp(path) {
        let sections = split_schelp_sections(&raw);
        for (sec_key, body) in sections {
            let cross = collect_schelp_cross_ref_tokens(&body);
            let body_clean = strip_schelp_markup_for_index(&body);
            let sec_display = sec_key.clone();
            push_chunks_from_text(
                chunks,
                &rel,
                &title,
                &sec_display,
                &body_clean,
                cross.as_str(),
            );
        }
        return;
    }

    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default();
    let text = if matches!(ext.to_ascii_lowercase().as_str(), "html" | "htm") {
        strip_html_tags(&raw)
    } else {
        raw
    };
    let text = normalize_ws(&text);
    if text.len() < 40 {
        return;
    }
    push_chunks_from_text(chunks, &rel, &title, "body", &text, "");
}

// --- Filesystem: collect indexable paths under Help / HelpSource ---

fn walk_docs(root: &Path, out: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(root) {
        Ok(r) => r,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            walk_docs(&p, out);
            continue;
        }
        if is_doc_file(&p) {
            out.push(p);
        }
    }
}

// --- Resolve doc tree root via `sc_process::detect_install_base_dir` ---

fn help_root() -> Option<PathBuf> {
    let base = sc_process::detect_install_base_dir()?;
    let mut candidates = vec![
        base.join("Help"),
        base.join("HelpSource"),
        base.join("share").join("SuperCollider").join("Help"),
        base.join("share").join("SuperCollider").join("HelpSource"),
    ];
    if base.ends_with("bin") {
        if let Some(parent) = base.parent() {
            candidates.push(parent.join("Help"));
            candidates.push(parent.join("HelpSource"));
            candidates.push(parent.join("share").join("SuperCollider").join("Help"));
            candidates.push(
                parent
                    .join("share")
                    .join("SuperCollider")
                    .join("HelpSource"),
            );
        }
    } else {
        candidates.push(base.join("bin").join("Help"));
        candidates.push(base.join("bin").join("HelpSource"));
        candidates.push(
            base.join("bin")
                .join("share")
                .join("SuperCollider")
                .join("Help"),
        );
        candidates.push(
            base.join("bin")
                .join("share")
                .join("SuperCollider")
                .join("HelpSource"),
        );
    }
    for c in candidates {
        if c.exists() {
            return Some(c);
        }
    }
    None
}

pub(crate) fn count_doc_files_under(root: &Path) -> usize {
    let mut v = Vec::new();
    walk_docs(root, &mut v);
    v.len()
}

fn build_index_impl() -> Result<DocsIndex, String> {
    let root = help_root().ok_or_else(|| {
        "SuperCollider Help directory not found from detected install path. Run detect_supercollider_install and get_server_docs to inspect discovered paths.".to_string()
    })?;
    let mut files = Vec::new();
    walk_docs(&root, &mut files);
    files.sort();

    let mut chunks = Vec::new();
    for path in &files {
        index_file(path, &root, &mut chunks);
    }

    if chunks.is_empty() {
        return Err("No docs content indexed from Help directory.".to_string());
    }

    Ok(DocsIndex {
        root,
        built_at: SystemTime::now(),
        chunks,
        files_indexed: files.len(),
    })
}

// --- In-memory index: build once (or on refresh), clone for readers ---

fn ensure_index(force_refresh: bool) -> Result<DocsIndex, String> {
    let cell = docs_index_lock();
    let mut guard = cell
        .lock()
        .map_err(|_| "docs index lock poisoned".to_string())?;
    if force_refresh || guard.is_none() {
        let idx = build_index_impl()?;
        *guard = Some(idx);
    }
    guard
        .as_ref()
        .cloned()
        .ok_or_else(|| "docs index unavailable".to_string())
}

// --- Search: tokenizer, synonym expansion, heuristic chunk scores ---

fn expand_synonyms(tokens: &mut HashSet<String>) {
    let mut add: Vec<String> = Vec::new();
    for t in tokens.iter() {
        for (key, extras) in QUERY_SYNONYMS {
            if t == key {
                for e in *extras {
                    add.push((*e).to_string());
                }
            }
        }
    }
    for a in add {
        tokens.insert(a);
    }
}

fn tokenize(query: &str) -> Vec<String> {
    let mut set = HashSet::new();
    for tok in query
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '_')
    {
        let t = tok.trim();
        if t.len() >= 2 {
            set.insert(t.to_string());
        }
    }
    expand_synonyms(&mut set);
    let mut v: Vec<String> = set.into_iter().collect();
    v.sort();
    v
}

fn synonym_note(query: &str) -> String {
    let lowered = query.to_lowercase();
    let mut parts = Vec::new();
    for (key, extras) in QUERY_SYNONYMS {
        if lowered.contains(key) {
            parts.push(format!("{}→{}", key, extras.join(",")));
        }
    }
    if parts.is_empty() {
        "none".to_string()
    } else {
        parts.join("; ")
    }
}

fn score_chunk(query_tokens: &[String], chunk: &DocChunk) -> i64 {
    let mut score = 0_i64;
    let sec_lower = chunk.section.to_lowercase();
    let examples_boost = sec_lower.contains("example");
    for t in query_tokens {
        if chunk.content_lower.contains(t) {
            score += 4;
        }
        if chunk.title.to_lowercase().contains(t) {
            score += 6;
        }
        if chunk.source_path.to_lowercase().contains(t) {
            score += 3;
        }
        if sec_lower.contains(t) {
            score += 2;
        }
        if !chunk.cross_ref_lower.is_empty() && chunk.cross_ref_lower.contains(t) {
            score += 5;
        }
    }
    if examples_boost {
        score += 2;
    }
    // `list::` sections benefit queries that name linked classes (LINK tokens also in cross_ref_lower).
    if sec_lower == "list" {
        score += 1;
    }
    score
}

fn clip_for_answer(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut s = text[..max].to_string();
    s.push_str(" ...");
    s
}

fn resolve_docs_source(source: Option<&str>) -> Result<(), String> {
    match source.map(|s| s.to_ascii_lowercase()).as_deref() {
        None | Some("local") | Some("") => Ok(()),
        Some("auto") => Ok(()),
        Some("web") => Err(
            "docs source=`web` is not implemented (offline-first). Use `local` or omit."
                .to_string(),
        ),
        Some(other) => Err(format!(
            "unknown docs source `{other}`; use `local` (default) or `auto`."
        )),
    }
}

/// Report help path resolution and in-memory index state (does not build index).
pub fn get_docs_index_status() -> String {
    let resolved_root = help_root();
    let disk_files = resolved_root
        .as_ref()
        .map(|p| count_doc_files_under(p))
        .unwrap_or(0);

    let cell = docs_index_lock();
    let guard = match cell.lock() {
        Ok(g) => g,
        Err(_) => return "get_docs_index_status failed: docs index lock poisoned".to_string(),
    };

    let mut out = String::new();
    let _ = writeln!(out, "SuperCollider docs index status");
    match &resolved_root {
        Some(p) => {
            let _ = writeln!(out, "- help_root_resolved={}", p.display());
            let _ = writeln!(out, "- doc_files_on_disk={disk_files}");
        }
        None => {
            out.push_str("- help_root_resolved=<not found>\n");
            let _ = writeln!(out, "- doc_files_on_disk=0");
        }
    }

    match guard.as_ref() {
        Some(idx) => {
            let _ = writeln!(out, "- index_loaded=true");
            let _ = writeln!(out, "- index_root={}", idx.root.display());
            let _ = writeln!(out, "- files_indexed={}", idx.files_indexed);
            let _ = writeln!(out, "- chunks={}", idx.chunks.len());
            let _ = writeln!(out, "- built_at={:?}", idx.built_at);
            if resolved_root.as_ref() != Some(&idx.root) {
                out.push_str("- note: resolved help root differs from index root; call refresh_supercollider_docs_index if install moved.\n");
            }
        }
        None => {
            out.push_str("- index_loaded=false\n");
            out.push_str("- hint: call refresh_supercollider_docs_index to build the in-memory index.\n");
        }
    }

    let _ = writeln!(out, "- query_synonyms_active=true");
    out
}

pub fn refresh_supercollider_docs_index() -> String {
    match ensure_index(true) {
        Ok(idx) => format!(
            "Docs index refreshed.\n- help_root={}\n- files_indexed={}\n- chunks={}",
            idx.root.display(),
            idx.files_indexed,
            idx.chunks.len()
        ),
        Err(e) => format!("refresh_supercollider_docs_index failed: {e}"),
    }
}

// --- `search_supercollider_docs` with `output=json` (machine-readable citations) ---

#[derive(Serialize)]
struct SearchHitJson {
    rank: usize,
    score: i64,
    title: String,
    section: String,
    source_path: String,
    excerpt: String,
}

#[derive(Serialize)]
struct SearchJsonEnvelope {
    format_version: u32,
    query: String,
    synonym_expansions_note: String,
    index_loaded: bool,
    help_root: String,
    files_indexed: usize,
    chunk_count: usize,
    built_at: Option<String>,
    results: Vec<SearchHitJson>,
}

pub fn search_supercollider_docs(
    query: &str,
    max_results: usize,
    output: &str,
    source: Option<&str>,
) -> String {
    if let Err(e) = resolve_docs_source(source) {
        return format!("search_supercollider_docs failed: {e}");
    }
    if query.trim().is_empty() {
        return "search_supercollider_docs: query is empty.".to_string();
    }
    let idx = match ensure_index(false) {
        Ok(v) => v,
        Err(e) => return format!("search_supercollider_docs failed: {e}"),
    };
    let tokens = tokenize(query);
    if tokens.is_empty() {
        return "search_supercollider_docs: query has no searchable tokens.".to_string();
    }

    let mut scored: Vec<(i64, usize)> = idx
        .chunks
        .iter()
        .enumerate()
        .map(|(i, ch)| (score_chunk(&tokens, ch), i))
        .filter(|(s, _)| *s > 0)
        .collect();
    scored.sort_by_key(|(s, _)| Reverse(*s));

    let take_n = max_results.clamp(1, 12);
    let json_mode = matches!(
        output.to_ascii_lowercase().as_str(),
        "json" | "structured"
    );

    if json_mode {
        let hits: Vec<SearchHitJson> = scored
            .iter()
            .take(take_n)
            .enumerate()
            .map(|(rank, (score, i))| {
                let ch = &idx.chunks[*i];
                SearchHitJson {
                    rank: rank + 1,
                    score: *score,
                    title: ch.title.clone(),
                    section: ch.section.clone(),
                    source_path: ch.source_path.clone(),
                    excerpt: clip_for_answer(&ch.content, 420),
                }
            })
            .collect();

        let env = SearchJsonEnvelope {
            format_version: 1,
            query: query.to_string(),
            synonym_expansions_note: synonym_note(query),
            index_loaded: true,
            help_root: idx.root.display().to_string(),
            files_indexed: idx.files_indexed,
            chunk_count: idx.chunks.len(),
            built_at: Some(format!("{:?}", idx.built_at)),
            results: hits,
        };
        return serde_json::to_string_pretty(&env)
            .unwrap_or_else(|e| format!(r#"{{"error":"json serialize failed: {e}"}}"#));
    }

    let mut out = String::new();
    let _ = writeln!(out, "SuperCollider docs search results for: {query}");
    let _ = writeln!(
        out,
        "- synonym_hints={}",
        synonym_note(query)
    );
    let _ = writeln!(
        out,
        "- help_root={} files_indexed={} chunks={} built_at={:?}",
        idx.root.display(),
        idx.files_indexed,
        idx.chunks.len(),
        idx.built_at
    );

    if scored.is_empty() {
        out.push_str("- No matching docs sections found.\n");
        out.push_str("- Try simpler terms (example: SynthDef, Node, Server, UGen).\n");
        return out;
    }

    for (rank, (score, i)) in scored.into_iter().take(take_n).enumerate() {
        let ch = &idx.chunks[i];
        let _ = writeln!(
            out,
            "\n[{}] score={} title={} section={} source={}",
            rank + 1,
            score,
            ch.title,
            ch.section,
            ch.source_path
        );
        let _ = writeln!(out, "{}", clip_for_answer(&ch.content, 420));
    }

    out
}

pub fn answer_supercollider_docs(question: &str) -> String {
    if question.trim().is_empty() {
        return "answer_supercollider_docs: question is empty.".to_string();
    }
    let idx = match ensure_index(false) {
        Ok(v) => v,
        Err(e) => return format!("answer_supercollider_docs failed: {e}"),
    };
    let tokens = tokenize(question);
    if tokens.is_empty() {
        return "answer_supercollider_docs: question has no searchable tokens.".to_string();
    }

    let mut scored: Vec<(i64, usize)> = idx
        .chunks
        .iter()
        .enumerate()
        .map(|(i, ch)| (score_chunk(&tokens, ch), i))
        .filter(|(s, _)| *s > 0)
        .collect();
    scored.sort_by_key(|(s, _)| Reverse(*s));

    if scored.is_empty() {
        return format!(
            "No grounded docs answer found for: {question}\nTry search_supercollider_docs with broader keywords (synonym expansion is applied automatically)."
        );
    }

    let mut out = String::new();
    out.push_str("Grounded answer from local SuperCollider docs:\n");
    let _ = writeln!(out, "- synonym_hints={}", synonym_note(question));
    out.push_str("Evidence sections:\n");

    for (rank, (_score, i)) in scored.into_iter().take(3).enumerate() {
        let ch = &idx.chunks[i];
        let _ = writeln!(
            out,
            "- [{}] {} [{}] ({})",
            rank + 1,
            ch.title,
            ch.section,
            ch.source_path
        );
        let _ = writeln!(out, "  {}", clip_for_answer(&ch.content, 260));
    }

    out.push_str("\nUse the cited sources above as ground truth for sclang/API facts. ");
    out.push_str("For verbatim examples, open the source file or call search_supercollider_docs.");
    out
}

/// Static routing guide for clients/models (offline-oriented).
pub fn get_mcp_tool_routing_hints() -> String {
    r#"SuperCollider MCP — tool routing (offline docs)

| User intent | Preferred tool(s) |
|-------------|-------------------|
| Is SC running? / server up? | ping_supercollider, get_server_status, get_servers |
| Version / install path | get_supercollider_version, detect_supercollider_install |
| Where are docs on disk? | get_server_docs, get_docs_index_status |
| Is the docs index warm? | get_docs_index_status |
(re)build docs index | refresh_supercollider_docs_index |
| Search help for class/topic | search_supercollider_docs (use output=json for structured citations) |
| Explain from help only | answer_supercollider_docs |
| Validate sclang syntax (no audio) | check_sclang_syntax |
| Run code on live scsynth (trusted host) | execute_supercollider_code (optional server_pid/osc_port from get_servers) |
| Debug process detection | list_server_candidates, discover_supercollider |

Docs tools use local Help/HelpSource only. source=web is not supported.

Workflow: get_docs_index_status → refresh_supercollider_docs_index (if needed) → search or answer."#
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_schelp_class_line() {
        assert_eq!(
            parse_schelp_header_line("class:: SinOsc"),
            Some(("class".to_string(), "SinOsc".to_string()))
        );
        assert_eq!(
            parse_schelp_header_line("Examples::"),
            Some(("examples".to_string(), String::new()))
        );
    }

    #[test]
    fn split_schelp_sections_minimal() {
        let raw = r"class:: SinOsc
summary:: sine
Description::
Hello world.

Examples::

code::
{ SinOsc.ar }.play;
::

";
        let v = split_schelp_sections(raw);
        assert!(v.iter().any(|(k, _)| k == "class"));
        assert!(v.iter().any(|(k, _)| k == "examples"));
    }

    #[test]
    fn tokenize_expands_guitar_synonym() {
        let mut set = HashSet::new();
        set.insert("guitar".to_string());
        expand_synonyms(&mut set);
        assert!(set.contains("pluck"));
    }

    #[test]
    fn code_marker_strip_basic() {
        let s = "Uses code::SinOsc:: for sound.";
        let t = simple_strip_code_markers(s);
        assert!(t.contains("SinOsc"));
    }

    #[test]
    fn cross_ref_collects_link_and_related() {
        let raw = "related:: Classes/Osc, Classes/FSinOsc\nlink::Classes/SinOsc::\n";
        let s = collect_schelp_cross_ref_tokens(raw);
        assert!(s.contains("sinosc"));
        assert!(s.contains("fsinosc"));
        assert!(s.contains("osc"));
    }

    #[test]
    fn expand_inline_link_strips_marker() {
        let t = expand_help_links_inline("See link::Classes/Pluck:: help.");
        assert!(t.contains("Pluck"));
        assert!(!t.contains("link::"));
    }

    /// If a standard SuperCollider layout exists on this machine, assert the doc tree is non-trivial.
    #[test]
    fn integration_help_tree_non_trivial_when_present() {
        for candidate in [
            r"C:\Program Files\SuperCollider-3.14.1\HelpSource",
            r"/usr/share/SuperCollider/HelpSource",
            r"/usr/local/share/SuperCollider/HelpSource",
        ] {
            let p = Path::new(candidate);
            if p.is_dir() {
                let n = count_doc_files_under(p);
                assert!(
                    n > 50,
                    "expected many doc files under {}, found {n}",
                    p.display()
                );
                return;
            }
        }
    }
}
