//! Local SuperCollider documentation indexing/search/QA helpers.

use std::cmp::Reverse;
use std::collections::HashSet;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use crate::sc_process;

#[derive(Clone)]
struct DocChunk {
    source_path: String,
    title: String,
    content: String,
    content_lower: String,
}

#[derive(Clone)]
struct DocsIndex {
    root: PathBuf,
    built_at: SystemTime,
    chunks: Vec<DocChunk>,
    files_indexed: usize,
}

static DOCS_INDEX: OnceLock<Mutex<Option<DocsIndex>>> = OnceLock::new();

fn index_cell() -> &'static Mutex<Option<DocsIndex>> {
    DOCS_INDEX.get_or_init(|| Mutex::new(None))
}

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

fn read_doc_text(path: &Path) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default();
    let text = if matches!(ext.to_ascii_lowercase().as_str(), "html" | "htm") {
        strip_html_tags(&raw)
    } else {
        raw
    };
    let text = normalize_ws(&text);
    if text.len() < 40 {
        return None;
    }
    Some(text)
}

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
            candidates.push(parent.join("share").join("SuperCollider").join("HelpSource"));
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

fn build_index_impl() -> Result<DocsIndex, String> {
    let root = help_root().ok_or_else(|| {
        "SuperCollider Help directory not found from detected install path. Run detect_supercollider_install and get_server_docs to inspect discovered paths.".to_string()
    })?;
    let mut files = Vec::new();
    walk_docs(&root, &mut files);
    files.sort();

    let mut chunks = Vec::new();
    for path in &files {
        let Some(text) = read_doc_text(path) else {
            continue;
        };
        let title = title_from_path(path);
        let rel = path
            .strip_prefix(&root)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| path.to_string_lossy().to_string());
        for sec in split_sections(&text, 900) {
            chunks.push(DocChunk {
                source_path: rel.clone(),
                title: title.clone(),
                content_lower: sec.to_lowercase(),
                content: sec,
            });
        }
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

fn ensure_index(force_refresh: bool) -> Result<DocsIndex, String> {
    let cell = index_cell();
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
    let mut v: Vec<String> = set.into_iter().collect();
    v.sort();
    v
}

fn score_chunk(query_tokens: &[String], chunk: &DocChunk) -> i64 {
    let mut score = 0_i64;
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

pub fn refresh_supercollider_docs_index() -> String {
    match ensure_index(true) {
        Ok(idx) => format!(
            "Docs index refreshed.\n- help_root={}\n- files_indexed={}\n- chunks={}",
            idx.root.to_string_lossy(),
            idx.files_indexed,
            idx.chunks.len()
        ),
        Err(e) => format!("refresh_supercollider_docs_index failed: {e}"),
    }
}

pub fn search_supercollider_docs(query: &str, max_results: usize) -> String {
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
    let mut out = String::new();
    let _ = writeln!(out, "SuperCollider docs search results for: {query}");
    let _ = writeln!(
        out,
        "- help_root={} files_indexed={} chunks={} built_at={:?}",
        idx.root.to_string_lossy(),
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
            "\n[{}] score={} title={} source={}",
            rank + 1,
            score,
            ch.title,
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
            "No grounded docs answer found for: {question}\nTry search_supercollider_docs with broader keywords."
        );
    }

    let mut out = String::new();
    out.push_str("Grounded answer from local SuperCollider docs:\n");
    out.push_str("I found relevant documentation sections. Key evidence:\n");

    for (rank, (_score, i)) in scored.into_iter().take(3).enumerate() {
        let ch = &idx.chunks[i];
        let _ = writeln!(
            out,
            "- [{}] {} ({})",
            rank + 1,
            ch.title,
            ch.source_path
        );
        let _ = writeln!(out, "  {}", clip_for_answer(&ch.content, 260));
    }

    out.push_str("\nUse the cited sources above as ground truth. ");
    out.push_str("If you want, call search_supercollider_docs for deeper excerpts.");
    out
}

