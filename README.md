# supercollider-mcp

MCP Server for SuperCollider.

Fully local pipeline with Supercollider MCP, using OpenWebUI as front-end and Ollama `gemma3:27b` model, runs on RTX3090 24Gb.

## Build

```bash
# Start OpenWebUI
open-webui serve

# Start the Python MCP
cd python-server
python -m supercollider_mcp_py.main --http --bind 0.0.0.0:8787

# Previous Rust MCP Tester
cargo run -- --http

# To test tools (preconfigured: Streamable HTTP + URL)
npx -y @modelcontextprotocol/inspector --transport http --server-url http://127.0.0.1:8787/mcp

```


## MCP tools (callable)

**Session**
- `initialize_supercollider_session()` — runs the full startup sequence in one call: detect install → ensure docs index loaded → return routing hints

**Discovery & Health**
- `ping_supercollider(message?)` — health check: process scan, OSC probe, install detect, `server_alive` flag
- `get_servers()` — running `scsynth`/`supernova` list with PID, CPU/mem, uptime, OSC port
- `discover_supercollider()` — full JSON report: processes, OSC reachability, install info
- `get_server_status(pid?)` — OSC status for one PID or auto-selected server
- `list_server_candidates()` — all SC-related processes (`scsynth`, `supernova`, `sclang`, `scide`) for debugging

**Install & Version**
- `detect_supercollider_install()` — resolve `sclang`, `scsynth`, `supernova` exe paths
- `get_supercollider_version()` — version from install path + `sclang -v`
- `get_server_docs()` — local `HelpSource` paths + doc.sccode.org mirrors

**Audio Hardware (raw python)**
- `get_audio_cards_raw_info()` — detailed raw sound card inventory from Windows/CIM
- `get_audio_endpoints_raw_info()` — raw MEDIA endpoint/device inventory (playback/capture path candidates)
- `get_audio_stack_report()` — combined report: sound cards, endpoints, Sound Mapper defaults, and `winmm` capabilities

**Docs**
- `get_docs_index_status()` — HelpSource root, file count, index loaded?
- `refresh_supercollider_docs_index()` — build/refresh in-memory index from `.schelp` files
- `search_supercollider_docs(query, max_results?, output?, source?)` — ranked local search; `output=json` for citations
- `answer_supercollider_docs(question)` — grounded Q&A from indexed docs
- `get_mcp_tool_routing_hints()` — intent → tool routing table

**Execution** *(trusted host only)*
- `check_sclang_syntax(code)` — compile-check without executing (~0.5–2s class library load)
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



### Open WebUI (example)

- Add MCP server URL `http://127.0.0.1:8787/mcp` (Streamable HTTP).
- In chat, enable tools.
