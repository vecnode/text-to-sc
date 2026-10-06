# legacy — the MCP servers this package was ported from

Everything in this folder is the **previous design**: a SuperCollider MCP server,
written twice — once in Rust and once in Python — each exposing a set of tools
over stdio or Streamable HTTP, each spawning a fresh `sclang` for every call.

It is kept, working, for three reasons:

1. **It is the reference implementation the port followed.** The Windows install
   discovery (`where.exe`, `Program Files`, the versioned directory scan), the
   four `-u` spellings a command line can use, the `/status` probe and its
   250 ms timeouts, the `.schelp` chunking and ranking formula, and the two
   `sclang` bootstrap scripts are all documented by this code and were
   transcribed from it rather than re-derived. When the new engine and this one
   disagree about a SuperCollider fact, this is where to look.
2. **It still runs.** Anyone already driving SuperCollider through an MCP client
   can keep doing so while the plugin is adopted. `mcp_py` and `rust-server` are
   unmodified apart from having moved.
3. **It records what was tried.** The split surface (18 tools in Rust, 10 in
   Python, a long "internal API" list in the README), the global control-target
   state, and the per-call process spawning are decisions that were made and
   then unmade. The new design's comments reference them by line
   (`legacy/rust-server/src/sc_process.rs:516`, `legacy/mcp_py/sc_process.py:33-40`)
   so a future reader can see what changed and why.

## What is here

```
legacy/
├─ rust-server/          a Rust MCP server (rmcp + tokio + axum + sysinfo)
│  ├─ Cargo.toml
│  └─ src/{main,server,sc_process,sc_docs,startup,streamable_http,supercollider_model}.rs
├─ mcp_py/               a Python MCP server (FastMCP + psutil)
│  ├─ {main,server,sc_process,sc_docs,startup,supercollider_model}.py
│  └─ (its entry point: supercollider-mcp-python)
├─ assets/               the two sclang bootstrap scripts
│  ├─ sclang_syntax_bootstrap.sc      compile-only, exit 0/1
│  └─ sclang_remote_execute.sc        Server.remote + interpret on a live server
└─ pyproject.toml        the Python package definition
```

(The `assets/` scripts are also the ones the new engine uses: see
`plugin/lib/assets/`. They are copied, not moved, because both halves need them.)

## Running the Rust server

```bash
cargo run --manifest-path legacy/rust-server/Cargo.toml -- --http
cargo run --manifest-path legacy/rust-server/Cargo.toml              # stdio
```

`--http` serves Streamable HTTP on `0.0.0.0:8787` at `/mcp`, which is what an
Open WebUI or any HTTP MCP client wants. The stdio mode is what an MCP client
that spawns servers wants.

## Running the Python server

```bash
python -m legacy.mcp_py.main --http --bind 0.0.0.0:8787
python -m legacy.mcp_py.main                                        # stdio
```

The Python package looks for its sclang assets at `../assets` relative to itself
(`mcp_py/sc_process.py`), which is why the two folders must stay side by side.

## What changed in the port

| Old | New | Why |
|---|---|---|
| A fresh `sclang` per call | One session that stays alive | The class library loads in 0.5–2 s **per call**, and a fresh interpreter forgets everything: no `~variable`, no `Ndef`, no `Pbind` still playing. A warm one answers in ~62 ms and keeps its state. |
| 18 tools (Rust) / 10 (Python) with heavy overlap | 10 tools by intent | Six near-synonyms for "what is running" cost the model attention at every step. |
| `sysinfo` / `psutil` for the process table | `Get-CimInstance` on Windows, `ps` elsewhere | The pack ships zero npm dependencies, so the platform's own tool is the source. |
| Windows-only install discovery | Three platforms | A package that claims macOS and Linux has to find SuperCollider there. |
| `Server.default.freeAll` through `sclang` to stop things | Raw OSC `/g_freeAll`, `/n_set`, `/n_free` | Stopping a synth should not require starting an interpreter. |
| Docs index built at boot by the Rust server | Built on first search | A tool call that never asks about documentation should not read 1146 files. |

## What the port deliberately dropped

- **Per-process disk I/O** and `psutil`-style per-process CPU: no
  zero-dependency source on Windows. The new tools say the field is unavailable
  rather than printing a zero.
- **Audio-device enumeration** via `winreg` and `ctypes`/winmm: Node has no
  FFI. `sc_server action=diagnose` reports what `scsynth` itself printed about
  the device it opened instead — which is the fact that actually explains
  "no sound".
- **Streamable HTTP as a transport for the new engine.** The DSH plugin speaks
  to the harness directly and the MCP face is stdio (`plugin/mcp/stdio.js`);
  HTTP MCP was there to serve a Dockerized Open WebUI, which is not a target any
  more.
