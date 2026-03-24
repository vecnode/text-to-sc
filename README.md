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
| `refresh_supercollider_docs_index` | *(none)*          | Builds or refreshes a local index from the installed SuperCollider Help docs. Run this after installation upgrades or if search quality drops.                                                                                           |
| `search_supercollider_docs`    | `query`, `max_results` optional | Searches indexed local docs and returns ranked snippets with source citations (file paths under the Help tree).                                                                                                                          |
| `answer_supercollider_docs`    | `question`            | Returns a grounded docs answer with evidence snippets and citations from local indexed docs.                                                                                                                                             |

*Tools are process + OSC aware. Node/synth graph introspection over OSC is not included yet.*

## Docs QA quick start

1. Run `detect_supercollider_install` to confirm install paths.
2. Run `refresh_supercollider_docs_index` once per session (or after updates).
3. Use `search_supercollider_docs` for retrieval and `answer_supercollider_docs` for grounded Q/A.

**Robustness:** `--bind` is validated at startup. Tools log `tool error:` on stderr if the background task fails; `tool ok:` only after a successful run. Process scans are panic-guarded so a bad `sysinfo` state returns a string error instead of crashing the MCP server.