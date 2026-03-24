# Roadmap

## Ongoing

- [ ] OSC server introspection (nodes / synth stats) as opt-in tools with strict timeouts.
- [ ] Persist docs index to disk for faster cold start and version-stamped cache invalidation.
- [ ] Semantic re-ranking (local embeddings) for docs — only if keyword retrieval plateaus.

## Completed

- [x] Stable MCP transport support: stdio and Streamable HTTP.
- [x] Fixed Windows port conflict workflow for MCP HTTP mode.
- [x] Added robust server health checks (`scsynth`/`supernova`/`sclang` detection).
- [x] Replaced hardcoded OSC probe with multi-port probing and cmdline port parsing.
- [x] Added detailed server inventory (`get_servers`) with PID, CPU, memory, uptime.
- [x] Added per-PID status tool (`get_server_status`) and discovery tool (`discover_supercollider`).
- [x] Improved SuperCollider install detection (standard paths + PATH + process hints).
- [x] Added explicit version tool (`get_supercollider_version`) with `sclang -v` support.
- [x] Added local docs indexing/search/QA tools (`refresh` / `search` / `answer` docs).
- [x] Fixed docs root detection to support both `Help` and `HelpSource` layouts.
- [x] `get_docs_index_status` — resolved help root, disk file count, index warm/cold, no auto-build.
- [x] Section-aware `.schelp` chunking — parses `class::`, `Description::`, `Examples::`, `code::` blocks; section-weighted scoring.
- [x] Query normalization — synonym expansion for common musical/technical phrasings before retrieval.
- [x] Structured docs search — `search_supercollider_docs` supports `output=json` with ranked citations.
- [x] `source` parameter on docs search — `local` default; `web` explicitly rejected (offline-first policy).
- [x] `get_mcp_tool_routing_hints` — intent → tool routing table for clients and models.
- [x] Integration-style test — non-trivial doc tree count when standard `HelpSource` paths exist.
- [x] README — Open WebUI-oriented docs workflow and tool table updates.
- [x] Richer `.schelp` indexing — `link::` / `related::` token extraction for cross-ref scoring; `LIST::` / `##` line normalization; inline link expansion in prose; `list` section tie-break boost.
- [x] `check_sclang_syntax` MCP tool — `sclang` compile-only bootstrap (`assets/sclang_syntax_bootstrap.sc`), no snippet execution / no audio.
- [x] `execute_supercollider_code` MCP tool — `sclang` + `Server.remote` + `interpret` against live scsynth (`assets/sclang_remote_execute.sc`); optional `server_pid` / `osc_port` from `get_servers`.
