# text-to-sc

![Language: JavaScript](https://img.shields.io/badge/language-JavaScript-f7df1e?logo=javascript&logoColor=black)
![License: MIT](https://img.shields.io/badge/license-MIT-blue)
![SuperCollider 3.13+](https://img.shields.io/badge/supercollider-3.13%2B-ff6b6b)
![DeepSeek Harness 0.2.0-rc.2](https://img.shields.io/badge/dsh-0.2.0--rc.2-4f8cff)
![Platforms: Windows | macOS | Linux](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-lightgrey)

**Give your agent SuperCollider superpowers.** Five skills teach it the language,
SynthDefs and live coding; a local engine boots the audio server, compiles and
loads patches, and plays and rewrites them while they sound — in DSH today, and
in any MCP-capable agent through the same engine.

```supercollider
Ndef(\drone, { |freq = 55, amp = 0.1| LFTri.ar(freq) * amp }).play;
Ndef(\drone).set(\freq, 41.2);      // still playing, no gap
```

## What this is

An agent that knows about audio DSP still gets SuperCollider wrong, because
SuperCollider is three specific things: a **language** with its own idioms, a
**server** that only speaks OSC, and a **reference** of over a thousand `.schelp`
files. This repository gives an agent all three.

- **It listens.** One `sclang` session stays alive across tool calls, so what the
  agent defines is still there — and still playing — next time. Measured on the
  machine this was written on: ready in ~1 s, then **62 ms** per evaluation.
  Spawning a fresh interpreter per call, which is what the MCP servers here used
  to do, costs 0.5–2 s *every time* and forgets everything.
- **It plays.** The audio server is booted, controlled and diagnosed by a
  hand-written OSC client: `/status`, `/d_load`, `/s_new`, `/n_set`, `/n_free`,
  `/g_queryTree`, `/quit`. No OSC library, no Python, no Rust, no MCP server —
  plain JavaScript with zero dependencies.
- **It reads.** The `.schelp` reference that ships with the install is searched
  and read, so the agent looks up what `RLPF` actually takes instead of guessing
  from the shape of the name.
- **It teaches.** Five skills carry the language, the SynthDef lifecycle, the
  live-coding workflow, the documentation map and the project layout. Every
  code example in them is **compiled** by a check, so a skill cannot quietly
  teach the model a mistake.
- **It stays out of the way.** `.scd` files open in the editor vncode already
  ships, and the agent works on them as files. The plugin's own UI is one console
  panel: the session's output, an input line, a Stop.

## Install

**With the [vncode](https://github.com/vecnode) pack (DSH):** the pack installs
this package from its `packages/` folder. On Windows run `scripts\install.bat` in
that repository; on macOS or Linux, `./scripts/install.sh`. Then restart the
harness and hard-refresh the browser.

**On its own (this repository):**

```
scripts\install.ps1 -Force        # Windows
./scripts/install.sh --force      # macOS / Linux
```

**For an agent that is not DSH:** `plugin/mcp/stdio.js` is an MCP server over the
same engine.

```json
{
  "mcpServers": {
    "supercollider": {
      "command": "node",
      "args": ["<this repo>/plugin/mcp/stdio.js"]
    }
  }
}
```

**Skills alone**, for any agent that reads them: point it at
`plugin/skills/*/SKILL.md`, or copy those folders into the agent's skills
directory. They are ordinary skill documents with `name`, `description` and
`whenToUse` frontmatter.

## Requirements

- **SuperCollider 3.13 or newer** (`sclang` and `scsynth`), installed the normal
  way. This is the only requirement, and there is nothing else to install.
- **Node.js 22 or newer** for the plugin host.

The install is found on `PATH`, at the platform's standard locations — including
the *versioned* folders a real install uses, such as
`C:\Program Files\SuperCollider-3.14.1` — or through `DSH_SC_SCLANG` and
`DSH_SC_SCSYNTH`.

## The tools

Eleven, organised by intent rather than by mechanism.

| Tool | What it does |
|---|---|
| `sc_status` | What is installed, what is running, which ports answered `/status` |
| `sc_help` | Search, read and quote the installed `.schelp` reference |
| `sc_check` | Compile-check sclang without running it |
| `sc_exec` | Run code in the session that stays alive — the real-time core |
| `sc_play` | A known-good tone, and the report of what the server actually did |
| `sc_capture` | **Measure what a sound really is**: plays it, records its output, reports peak, RMS, clipping, tonality and the strongest partials |
| `sc_project` | Read, list and write `.scd` files; send one to the live session; **browse the sixteen bundled example instruments and load one** |
| `sc_load` | Evaluate a `.scd` that is on disk — the edit-here / hear-it loop |
| `sc_synthdef` | Compile, cache, load, list and free SynthDefs |
| `sc_nodes` | The running graph: tree, set a control while it plays, free, free all |
| `sc_server` | Boot, quit, reboot and diagnose the audio server |

## The skills

| Skill | What it teaches |
|---|---|
| `supercollider-live-coding` | Building a piece while it plays: `Ndef`, `Pbind`, changing a parameter in place, and stopping cleanly |
| `supercollider-synthdefs` | The SynthDef lifecycle and how to read the compiler when it refuses |
| `supercollider-language` | The language itself: function blocks, `var` scoping, `.ar`/`.kr`, multi-channel expansion, and what each error means |
| `supercollider-scout-docs` | Finding the answer in the installed reference instead of guessing |
| `supercollider-projects` | `.scd` files, session state versus file state, and how SuperCollider packages a piece |

## What is in the repository

```
plugin/          THE PACKAGE - this is what vncode ships as packages/dsh-supercollider/
  lib/engine/    the engine: OSC, install and process discovery, the sclang session,
                 scsynth control, the .schelp index, the .scd file workflow,
                 the example catalogue, and the measurement arithmetic
  lib/tools.js   the eleven tools
  lib/client.js  the console tab
  skills/        the five skills
  examples/      sixteen playable instruments, each explaining its DSP idea
  mcp/stdio.js   the MCP face for other clients
  checks/        the four checks, so they travel with the package
scripts/         install (PowerShell + POSIX shell) and the vncode sync
legacy/          the Rust and Python MCP servers this was ported from, still working
```

**Why `text-to-sc` is the source and vncode gets a copy.** The pack ships
`packages/` as a live-linked directory inside a distribution, and its version
manifest is checked against every bundle's `package.json`; a git submodule at an
external path satisfies neither, and a `node_modules` dependency is skipped by the
distribution outright. So [`scripts/sync-to-vncode.mjs`](scripts/sync-to-vncode.mjs)
writes the copy — and `--check` fails when it has fallen behind.

**Why the old MCP servers are still here.** They are the reference the port
followed and they still run: see [`legacy/README.md`](legacy/README.md) for what
was kept, what was changed and what was deliberately dropped.

## Verify

```
cd plugin
npm run check                 # all four, in order
npm run check:node            # the engine, offline
npm run check:wiring          # the package's contract, without the harness
npm run check:examples        # every skill example, compiled
npm run check:library         # every bundled instrument, compiled and shaped
```

The checks live **inside the package** (`plugin/checks/`), so they travel with it
into vncode, where they run as `npm run check` in
`packages/dsh-supercollider/`. All four skip a section loudly and exit 0 on a
machine with no SuperCollider, and none is ever weakened to make a change pass.

`check:wiring` is the one to run before touching packaging: it asserts the
manifest the harness reads (`dsh.bundle.patch`, `dsh.client.platform`, the
`./client` export), runs the browser bundle as the classic script a loader
instantiates — including calling its factory, since that is where the stylesheet
and the plugin face live — and drives `apply(ctx)` against a stub context to
prove eleven tools, three routes and five skills register and that every tool
declares the output contract the registry enforces.

## Limits

- **No SuperCollider binary is shipped.** It is ~50 MB and installer-specific, so
  the plugin finds yours or tells you how to get one.
- **One interpreter and one audio server are shared** by every conversation, in
  both the plugin and the console. An audio device is one global resource;
  `sc_status` says who owns it rather than pretending otherwise.
- **Per-process disk I/O and audio-device enumeration are not reported**: neither
  has a zero-dependency source in Node. `sc_server action=diagnose` reads the
  server's own log for the device instead.
- **Answers over 32 KiB go through a file** under `$DSH_HOME`, with a SHA-256 in
  the inline answer. That is measured, not defensive: a large print through the
  `sclang` prompt arrives interleaved with the input echo and truncated.

## License

MIT. See [LICENSE](LICENSE).
