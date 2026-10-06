from __future__ import annotations

import sys
import threading


TOOLS_BANNER = (
    "ensure_supercollider_app_on, get_server_status, "
    "search_supercollider_docs, answer_supercollider_docs, check_sclang_syntax, "
    "execute_supercollider_code, play_test_tone, stop_supercollider_synths, "
    "reboot_supercollider_server, get_audio_diagnostics"
)


def _run_probe() -> None:
    """Background probe: scan processes, OSC ports, install, and warm the docs index."""
    try:
        from . import sc_docs, sc_process

        # 1. Full process + OSC + install scan
        snap = sc_process.collect_snapshot()

        server_count = len(snap.servers)
        candidate_count = len(snap.candidates)
        responding = [p for p, ok in snap.osc_port_results.items() if ok]
        install_dir = snap.install.base_dir if snap.install else "<not found>"

        print(
            f"[supercollider-mcp] probe: install={install_dir} | "
            f"servers={server_count} | candidates={candidate_count} | "
            f"osc_responding={responding or 'none'}",
            file=sys.stderr,
        )

        # 2. Warm the docs index (non-blocking for the main server thread)
        try:
            sc_docs.ensure_index(force=False)
            idx = sc_docs.DOCS_INDEX
            if idx is not None:
                print(
                    f"[supercollider-mcp] docs index ready: "
                    f"{idx.files_indexed} files, {len(idx.chunks)} chunks",
                    file=sys.stderr,
                )
        except Exception as exc:
            print(f"[supercollider-mcp] docs index warning: {exc}", file=sys.stderr)

    except Exception as exc:
        print(f"[supercollider-mcp] startup probe error: {exc}", file=sys.stderr)


def run_startup_probe() -> None:
    """Launch the startup probe in a daemon thread so it never blocks server boot."""
    t = threading.Thread(target=_run_probe, name="sc-startup-probe", daemon=True)
    t.start()


def stdio() -> None:
    print(
        "[supercollider-mcp-python] MCP over stdio (default). "
        "Use --http for Streamable HTTP.",
        file=sys.stderr,
    )
    print(f"[supercollider-mcp-python] tools: {TOOLS_BANNER}", file=sys.stderr)


def streamable_http(bind_host: str, bind_port: int) -> None:
    print(
        f"[supercollider-mcp-python] listening on http://{bind_host}:{bind_port}/mcp",
        file=sys.stderr,
    )
    if bind_host == "0.0.0.0":
        print(
            "[supercollider-mcp-python] Open WebUI URL: "
            f"http://127.0.0.1:{bind_port}/mcp",
            file=sys.stderr,
        )
    print(f"[supercollider-mcp-python] tools: {TOOLS_BANNER}", file=sys.stderr)
