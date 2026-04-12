# supercollider-mcp

MCP Server for SuperCollider.

Fully local pipeline with Supercollider MCP, using OpenWebUI as front-end and Ollama `gemma3:27b` model, runs on RTX3090 24Gb.

### Build

```bash
# Start OpenWebUI
open-webui serve

# Start the Python MCP
python -m mcp_py.main --http --bind 0.0.0.0:8787

# Test MCP Tools (preconfigured: Streamable HTTP + URL)
npx -y @modelcontextprotocol/inspector --transport http --server-url http://127.0.0.1:8787/mcp

# Optional: Rust MCP implementation
cargo run --manifest-path rust-server/Cargo.toml -- --http
```

### MCP tools

**MCP Core (exposed)**
- `ensure_supercollider_app_on`
- `get_server_status`
- `search_supercollider_docs`
- `answer_supercollider_docs`
- `check_sclang_syntax`
- `execute_supercollider_code`
- `play_test_tone`
- `stop_supercollider_synths`
- `reboot_supercollider_server`
- `get_audio_diagnostics`

**Docs**
- `get_docs_index_status()` — HelpSource root, file count, index loaded?
- `refresh_supercollider_docs_index()` — build/refresh in-memory index from `.schelp` files
- `search_supercollider_docs(query, max_results?, output?, source?)` — ranked local search; `output=json` for citations
- `answer_supercollider_docs(question)` — grounded Q&A from indexed docs
- `get_mcp_tool_routing_hints()` — intent → tool routing table

**Execution**
- `check_sclang_syntax(code)` — compile-check without executing (~0.5–2s class library load)
- `ensure_supercollider_app_on(port?, use_supernova?, boot_server?)` — ensures SuperCollider app/runtime (`scide`/`sclang`) is ON; optionally boots/reuses audio server too
- `ensure_supercollider_server_on(port?, use_supernova?)` — ensure server is ON; reuse existing reachable `scsynth`/`supernova` or boot one if none are running
- `start_supercollider_server(port?, use_supernova?)` — boot scsynth/supernova directly from the detected install; no IDE needed
- `set_supercollider_server_active(server_pid?, osc_port?)` — ensure server is ACTIVE for MCP control; starts one if none is running
- `set_supercollider_server_inactive()` — mark server control state INACTIVE without killing the process
- `turn_down_supercollider_server(server_pid?, osc_port?)` — stop the server and mark control state INACTIVE
- `play_test_tone(freq_hz?, amp?, duration_s?, server_pid?, osc_port?)` — deterministic short beep to verify actual audio output path (auto-loads `mcpTone` SynthDef if needed)
- `get_audio_diagnostics(server_pid?, osc_port?)` — inspect target PID/port, `/status.reply` metrics, and scsynth log tail (if launched by MCP)
- `execute_supercollider_code(code, server_pid?, osc_port?)` — run sclang against live `scsynth`; e.g. `{ SinOsc.ar(440, 0, 0.3) }.play`
- `stop_supercollider_synths(server_pid?, osc_port?)` — `Server.default.freeAll` (silence all synths)
- `quit_supercollider_server(server_pid?, osc_port?)` — send OSC `/quit` to stop the server process
- `reboot_supercollider_server(server_pid?, osc_port?)` — `/quit` then respawn with `-u <port>`


### API

**API/Internal only (remove from MCP surface, keep in app API)**
- `discover_supercollider`
- `list_server_candidates`
- `get_servers`
- `detect_supercollider_install`
- `get_supercollider_version`
- `get_server_docs`
- `get_docs_index_status`
- `refresh_supercollider_docs_index`
- `get_mcp_tool_routing_hints`
- `get_audio_cards_raw_info`
- `get_audio_endpoints_raw_info`
- `get_audio_stack_report`
- `start_supercollider_server`
- `ensure_supercollider_server_on`
- `set_supercollider_server_active`
- `set_supercollider_server_inactive`
- `turn_down_supercollider_server`
- `quit_supercollider_server`

### Open WebUI (example)

- Add MCP server URL `http://127.0.0.1:8787/mcp` (Streamable HTTP).
- In chat, enable tools.
