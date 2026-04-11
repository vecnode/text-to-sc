from __future__ import annotations

from typing import Annotated, Literal, Optional

from mcp.server.fastmcp import FastMCP

from . import sc_docs, sc_process
from .supercollider_model import SupercolliderServerState


def build_server(host: str = "127.0.0.1", port: int = 8000, streamable_http_path: str = "/mcp") -> FastMCP:
    mcp = FastMCP(
        name="supercollider-mcp",
        host=host,
        port=port,
        streamable_http_path=streamable_http_path,
        instructions=(
            "Use ping_supercollider for quick health; get_servers for detailed server stats; "
            "discover_supercollider for full JSON discovery; get_server_status for one PID; "
            "detect_supercollider_install/get_supercollider_version/get_server_docs for "
            "install/version/docs paths; get_docs_index_status before heavy docs use; "
            "refresh_supercollider_docs_index/search_supercollider_docs "
            "(output=json optional)/answer_supercollider_docs for grounded local docs; "
            "check_sclang_syntax to compile-check sclang; execute_supercollider_code / "
            "stop_supercollider_synths for synth graph control; quit_supercollider_server / "
            "reboot_supercollider_server for process-level /quit+respawn; "
            "get_mcp_tool_routing_hints for intent->tool mapping."
        ),
    )

    _state = SupercolliderServerState.bootstrap_placeholder()

    @mcp.tool()
    def ping_supercollider(
        message: Annotated[Optional[str], "Optional greeting or label included in the health-check response."] = None,
    ) -> str:
        """Quick SuperCollider health check: process scan, OSC probe, install detection."""
        return sc_process.probe(message)

    @mcp.tool()
    def get_servers() -> str:
        """List running SuperCollider server processes (scsynth/supernova) with PID, CPU/memory, OSC status."""
        return sc_process.get_servers()

    @mcp.tool()
    def discover_supercollider() -> str:
        """Full discovery report: processes, OSC ports, server instances, install info as JSON."""
        return sc_process.discover_supercollider()

    @mcp.tool()
    def get_server_status(
        pid: Annotated[Optional[int], "PID of the scsynth/supernova process to query; omit to auto-select."] = None,
    ) -> str:
        """Get OSC reachability, CPU/memory, and uptime for a specific server PID or auto-pick active server."""
        return sc_process.get_server_status(pid)

    @mcp.tool()
    def detect_supercollider_install() -> str:
        """Detect SuperCollider installation and resolve sclang/scsynth/supernova executable paths."""
        return sc_process.detect_supercollider_install()

    @mcp.tool()
    def get_supercollider_version() -> str:
        """Report detected SuperCollider version from install path and running binary hints."""
        return sc_process.get_supercollider_version()

    @mcp.tool()
    def get_server_docs() -> str:
        """Return local HelpSource paths and optional online doc.sccode.org mirror links."""
        return sc_process.get_server_docs()

    @mcp.tool()
    def list_server_candidates() -> str:
        """List all SuperCollider-related process candidates (scsynth/supernova/sclang/scide) for debugging."""
        return sc_process.list_server_candidates()

    @mcp.tool()
    def initialize_supercollider_session() -> str:
        """Run the full session-start sequence in one call: detect install, ensure docs index is loaded, return tool routing hints. Call this once at the start of any session instead of chaining individual setup tools."""
        return sc_docs.initialize_supercollider_session()

    @mcp.tool()
    def get_docs_index_status() -> str:
        """Report Help/HelpSource root, doc file count, and whether the in-memory index is loaded."""
        return sc_docs.get_docs_index_status()

    @mcp.tool()
    def get_mcp_tool_routing_hints() -> str:
        """Return offline-first intent-to-tool routing table for health, version, docs, search, and Q&A."""
        return sc_docs.get_mcp_tool_routing_hints()

    @mcp.tool()
    def refresh_supercollider_docs_index() -> str:
        """Build or refresh the local in-memory index from Help/HelpSource .schelp files."""
        return sc_docs.refresh_supercollider_docs_index()

    @mcp.tool()
    def search_supercollider_docs(
        query: Annotated[str, "Search query for SuperCollider docs (class names, topics, keywords)."],
        max_results: Annotated[int, "Maximum number of doc matches to return."] = 5,
        output: Annotated[str, "Output format: 'text' for readable, 'json' for structured citations."] = "text",
        source: Annotated[Literal["local", "auto", "web"], "Doc source: 'local' searches local HelpSource (default)."] = "local",
    ) -> str:
        """Search local SuperCollider docs with synonym expansion and ranked results."""
        return sc_docs.search_supercollider_docs(
            query=query,
            max_results=max_results,
            output=output,
            source=source,
        )

    @mcp.tool()
    def answer_supercollider_docs(
        question: Annotated[str, "Natural-language question about SuperCollider to answer from local docs."],
    ) -> str:
        """Answer a SuperCollider question grounded in local indexed docs with evidence snippets."""
        return sc_docs.answer_supercollider_docs(question)

    @mcp.tool()
    def check_sclang_syntax(
        code: Annotated[str, "sclang source code to compile-check. Not executed; class library loads (~0.5-2s)."],
    ) -> str:
        """Compile-check sclang code with sclang without executing it or producing audio."""
        return sc_process.check_sclang_syntax(code)

    @mcp.tool()
    def execute_supercollider_code(
        code: Annotated[str, "sclang code to execute on the live scsynth server. Example: '{ SinOsc.ar(440, 0, 0.3) }.play'"],
        server_pid: Annotated[Optional[int], "PID of the target scsynth/supernova process; omit to auto-detect."] = None,
        osc_port: Annotated[Optional[int], "OSC UDP port of the target server (e.g. 57110); omit to auto-detect."] = None,
    ) -> str:
        """TRUSTED HOST ONLY: Execute sclang code against the live scsynth server. Use this to play sounds, create synths, etc. The code runs inside a Server.remote session."""
        return sc_process.execute_supercollider_code(
            code=code,
            server_pid=server_pid,
            osc_port=osc_port,
        )

    @mcp.tool()
    def start_supercollider_server(
        port: Annotated[int, "UDP port for the scsynth server to listen on (default 57110)."] = 57110,
        use_supernova: Annotated[bool, "Boot supernova instead of scsynth if available."] = False,
    ) -> str:
        """TRUSTED HOST ONLY: Boot scsynth (or supernova) directly from the detected SC install. Use this when get_servers shows no running server. Waits up to 25s for OSC readiness."""
        return sc_process.start_supercollider_server(port=port, use_supernova=use_supernova)

    @mcp.tool()
    def stop_supercollider_synths(
        server_pid: Annotated[Optional[int], "PID of the target scsynth/supernova process; omit to auto-detect."] = None,
        osc_port: Annotated[Optional[int], "OSC UDP port of the target server; omit to auto-detect."] = None,
    ) -> str:
        """TRUSTED HOST ONLY: Run Server.default.freeAll to silence all synths on the target server."""
        return sc_process.stop_supercollider_synths(
            server_pid=server_pid,
            osc_port=osc_port,
        )

    @mcp.tool()
    def quit_supercollider_server(
        server_pid: Annotated[Optional[int], "PID of the target scsynth/supernova process; omit to auto-detect."] = None,
        osc_port: Annotated[Optional[int], "OSC UDP port of the target server; omit to auto-detect."] = None,
    ) -> str:
        """TRUSTED HOST ONLY: Send raw OSC /quit to stop the scsynth/supernova process."""
        return sc_process.quit_supercollider_server(
            server_pid=server_pid,
            osc_port=osc_port,
        )

    @mcp.tool()
    def reboot_supercollider_server(
        server_pid: Annotated[Optional[int], "PID of the target scsynth/supernova process; omit to auto-detect."] = None,
        osc_port: Annotated[Optional[int], "OSC UDP port of the target server; omit to auto-detect."] = None,
    ) -> str:
        """TRUSTED HOST ONLY: OSC /quit then respawn scsynth/supernova with -u <port>."""
        return sc_process.reboot_supercollider_server(
            server_pid=server_pid,
            osc_port=osc_port,
        )

    return mcp
