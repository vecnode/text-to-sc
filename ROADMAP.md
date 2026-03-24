# Roadmap

## Ongoing

- [ ] Add `get_docs_index_status` tool (docs root, files/chunks count, last refresh).
- [ ] Add fallback to online docs when local docs are unavailable.
- [ ] Improve `.schelp` parsing with section-aware chunking.
- [ ] Add query normalization for common SC terms/synonyms.
- [ ] Add optional `source` selector to docs tools (`local`/`web`/`auto`).
- [ ] Add structured JSON mode for docs search output.
- [ ] Add integration tests for docs indexing against real SC install layouts.
- [ ] Add diagnostics command for tool-routing hints (which tool to call by intent).
- [ ] Add OSC server introspection phase (nodes/synth stats) as opt-in tools.
- [ ] Improve README with end-to-end OpenWebUI examples for docs QA.

## Completed

- [x] Stable MCP transport support: stdio and Streamable HTTP.
- [x] Fixed Windows port conflict workflow for MCP HTTP mode.
- [x] Added robust server health checks (`scsynth`/`supernova`/`sclang` detection).
- [x] Replaced hardcoded OSC probe with multi-port probing and cmdline port parsing.
- [x] Added detailed server inventory (`get_servers`) with PID, CPU, memory, uptime.
- [x] Added per-PID status tool (`get_server_status`) and discovery tool (`discover_supercollider`).
- [x] Improved SuperCollider install detection (standard paths + PATH + process hints).
- [x] Added explicit version tool (`get_supercollider_version`) with `sclang -v` support.
- [x] Added local docs indexing/search/QA tools (`refresh/search/answer` docs).
- [x] Fixed docs root detection to support both `Help` and `HelpSource` layouts.

