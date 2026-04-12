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
            "Use initialize_supercollider_session first; ensure_supercollider_app_on to boot app/runtime; "
            "get_server_status for control target health; search_supercollider_docs/answer_supercollider_docs "
            "for grounded docs; check_sclang_syntax before execution; execute_supercollider_code/play_test_tone "
            "for sound actions; stop_supercollider_synths/reboot_supercollider_server for recovery; "
            "get_audio_diagnostics when audio path fails."
        ),
    )

    _state = SupercolliderServerState.bootstrap_placeholder()

    @mcp.tool()
    def get_server_status(
        pid: Annotated[Optional[int], "PID of the scsynth/supernova process to query; omit to auto-select."] = None,
    ) -> str:
        """Get OSC reachability, CPU/memory, and uptime for a specific server PID or auto-pick active server."""
        return sc_process.get_server_status(pid)

    @mcp.tool()
    def initialize_supercollider_session() -> str:
        """Run the full session-start sequence in one call: detect install, ensure docs index is loaded, return tool routing hints. Call this once at the start of any session instead of chaining individual setup tools."""
        return sc_docs.initialize_supercollider_session()


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
    def play_test_tone(
        freq_hz: Annotated[float, "Tone frequency in Hz (default 440)."] = 440.0,
        amp: Annotated[float, "Amplitude 0..1 (default 0.15)."] = 0.15,
        duration_s: Annotated[float, "Tone duration in seconds (default 1.0)."] = 1.0,
        server_pid: Annotated[Optional[int], "PID of the target scsynth/supernova process; omit to auto-detect."] = None,
        osc_port: Annotated[Optional[int], "OSC UDP port of the target server (e.g. 57110); omit to auto-detect."] = None,
    ) -> str:
        """TRUSTED HOST ONLY: Play a short deterministic sine test tone on the live server. Use this to verify that audio output actually works before running complex snippets."""
        return sc_process.play_test_tone(
            freq_hz=freq_hz,
            amp=amp,
            duration_s=duration_s,
            server_pid=server_pid,
            osc_port=osc_port,
        )

    @mcp.tool()
    def get_audio_diagnostics(
        server_pid: Annotated[Optional[int], "PID of the target scsynth/supernova process; omit to auto-detect."] = None,
        osc_port: Annotated[Optional[int], "OSC UDP port of the target server (e.g. 57110); omit to auto-detect."] = None,
    ) -> str:
        """TRUSTED HOST ONLY: Diagnose 'server is up but no sound' issues. Returns current control target, /status.reply metrics, and tracked scsynth log tail when available."""
        return sc_process.get_audio_diagnostics(server_pid=server_pid, osc_port=osc_port)

    @mcp.tool()
    def ensure_supercollider_app_on(
        port: Annotated[int, "Preferred OSC UDP port if server boot is needed (default 57110)."] = 57110,
        use_supernova: Annotated[bool, "When booting server, prefer supernova instead of scsynth if available."] = False,
        boot_server: Annotated[bool, "If true, also ensure a reachable audio server after app startup."] = True,
    ) -> str:
        """TRUSTED HOST ONLY: Ensure SuperCollider app/runtime is ON (scide/sclang). Optionally boots/reuses the audio server."""
        return sc_process.ensure_supercollider_app_on(
            port=port,
            use_supernova=use_supernova,
            boot_server=boot_server,
        )

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
