# supercollider-mcp

Rust MCP Server boilerplate for SuperCollider.

Fully local pipeline with MCP Client OpenWebUI and Ollama `gemma3:27b` model.

## Build

```bash
# Start OpenWebUI
open-webui serve

# Start the MCP
cargo run -- --http
```

## Run as an MCP server

**stdio (default)** — subprocess clients (Cursor, ChatMCP, etc.): `cargo run`

**Streamable HTTP** — `cargo run -- --http` (default `0.0.0.0:8787`). Open WebUI: MCP (Streamable HTTP), URL `http://127.0.0.1:8787/mcp`, Auth None; enable tool in chat. Docker: `host.docker.internal`.

## MCP tools (callable)


| Tool                           | Args                  | What it does                                                                                                                                                                                                                             |
| ------------------------------ | --------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `ping_supercollider`           | `message` optional    | Quick health check with process scan (`scsynth`/`supernova`/`sclang`), OSC `/status` probing on candidate ports (not hardcoded to one), install detection, and `server_alive=<bool>`.                                              |
| `get_servers`                  | *(none)*              | Detailed list of running server processes (`scsynth`/`supernova`) with PID, CPU/memory, uptime, disk I/O counters, candidate ports, responding OSC port, and install-path match status.                                            |
| `discover_supercollider`       | *(none)*              | Unified discovery report plus machine-friendly JSON: processes, server instances, checked OSC ports and reachability, and install information.                                                                                         |
| `get_server_status`            | `pid` optional        | Focused status for one server process (or auto-picked active server): OSC reachability, responding port, CPU/memory, uptime, install match.                                                                                           |
| `detect_supercollider_install` | *(none)*              | Detects local SuperCollider installation and resolved executable paths (`sclang.exe`, `scsynth.exe`, `supernova.exe`) in standard Windows locations.                                                                                   |
| `get_supercollider_version`    | *(none)*              | Reports detected SuperCollider version from install path/version hints and `sclang -v` when available, plus running binary paths.                                                                                                        |
| `get_server_docs`              | *(none)*              | Returns local docs/install paths (if detected) and official online SuperCollider documentation links.                                                                                                                                   |
| `list_server_candidates`       | *(none)*              | Debug helper that lists all SuperCollider-related process candidates (`scsynth`, `supernova`, `sclang`, `scide`) with PID, exe path, and command line to diagnose detection mismatches.                                              |
| `get_docs_index_status`        | *(none)*              | Reports resolved Help/HelpSource root, on-disk doc file count, and whether the in-memory index is loaded (does not build the index).                                                                                                  |
| `get_mcp_tool_routing_hints`   | *(none)*              | Returns an offline-first routing table: which tool to call for server health, version, docs index, search, and grounded Q&A.                                                                                                         |
| `refresh_supercollider_docs_index` | *(none)*          | Builds or refreshes a local in-memory index from `Help` / `HelpSource` (section-aware `.schelp`). Run after SC upgrades or if search quality drops.                                                                                     |
| `search_supercollider_docs`    | `query`, `max_results`, `output`, `source` optional | Ranked local doc search with synonym expansion. Set `output=json` for structured citations. `source=local` (default); `web` is not supported (offline-first).                                                                           |
| `answer_supercollider_docs`    | `question`            | Grounded Q&A from indexed local docs with evidence snippets (synonym expansion applied).                                                                                                                                                |
| `check_sclang_syntax`          | `code`                | Runs `sclang` compile-only on a temp snippet (not executed; class library load ~0.5–2s). Returns OK or stderr tail on parse errors.                                                                                                    |
| `execute_supercollider_code`   | `code`, `server_pid` optional, `osc_port` optional | **Trusted host only.** Spawns `sclang` with `Server.remote` to the live `scsynth` (use PID/port from `get_servers` or omit for auto-target), `interpret`s your snippet, exits. Arbitrary code + audio side effects. |

*Tools are process + OSC aware. Node/synth graph introspection over OSC is not included yet.*

**Security:** `execute_supercollider_code` runs user-supplied sclang on your machine and can start/stop synths. Enable this MCP only for local, trusted Open WebUI / agent sessions.

## Docs QA quick start

1. Run `detect_supercollider_install` (or `get_supercollider_version`) to confirm install paths.
2. Run `get_docs_index_status` — if `index_loaded=false`, run `refresh_supercollider_docs_index`.
3. For class/symbol lookup, call `search_supercollider_docs` with `output=json` so the model gets structured citations.
4. For explanations, call `answer_supercollider_docs`; encourage the model to quote cited paths and example code from the tool output.
5. If the model picks the wrong tool, call `get_mcp_tool_routing_hints` once per session.

### Open WebUI (example flow)

- Add MCP server URL `http://127.0.0.1:8787/mcp` (Streamable HTTP).
- In chat, enable tools. For a coding question: first **search** (`search_supercollider_docs`), then **answer** (`answer_supercollider_docs`) using the same topic so retrieval and summary stay aligned.

**Robustness:** `--bind` is validated at startup. Tools log `tool error:` on stderr if the background task fails; `tool ok:` only after a successful run. Process scans are panic-guarded so a bad `sysinfo` state returns a string error instead of crashing the MCP server.