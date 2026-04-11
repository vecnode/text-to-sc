from __future__ import annotations

import sys
import threading


TOOLS_BANNER = (
    "initialize_supercollider_session, ping_supercollider, get_servers, "
    "discover_supercollider, get_server_status, detect_supercollider_install, "
    "get_supercollider_version, get_server_docs, list_server_candidates, "
    "get_docs_index_status, get_mcp_tool_routing_hints, "
    "refresh_supercollider_docs_index, search_supercollider_docs, "
    "answer_supercollider_docs, check_sclang_syntax, execute_supercollider_code, play_test_tone, get_audio_diagnostics, "
    "get_audio_cards_raw_info, get_audio_endpoints_raw_info, get_audio_stack_report, "
    "start_supercollider_server, set_supercollider_server_active, set_supercollider_server_inactive, "
    "turn_down_supercollider_server, stop_supercollider_synths, "
    "quit_supercollider_server, reboot_supercollider_server"
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
