from __future__ import annotations

import argparse
import ipaddress

from . import startup
from .server import build_server


def _parse_bind(bind: str) -> tuple[str, int]:
    value = bind.strip()
    if ":" not in value:
        raise ValueError(f"invalid --bind {value!r}")
    host, raw_port = value.rsplit(":", 1)
    if not host:
        raise ValueError(f"invalid --bind {value!r}")
    try:
        ipaddress.ip_address(host)
    except ValueError as exc:
        raise ValueError(f"invalid --bind {value!r}: bad host") from exc
    try:
        port = int(raw_port)
    except ValueError as exc:
        raise ValueError(f"invalid --bind {value!r}: bad port") from exc
    if not (1 <= port <= 65535):
        raise ValueError(f"invalid --bind {value!r}: port out of range")
    return host, port


def cli() -> None:
    parser = argparse.ArgumentParser(prog="supercollider-mcp-python")
    parser.add_argument("--http", action="store_true", help="Run Streamable HTTP transport")
    parser.add_argument(
        "--bind",
        default="0.0.0.0:8787",
        help="Bind address for HTTP mode (default: 0.0.0.0:8787)",
    )
    args = parser.parse_args()

    if args.http:
        host, port = _parse_bind(args.bind)
        mcp = build_server(host=host, port=port, streamable_http_path="/mcp")
        startup.streamable_http(host, port)
        startup.run_startup_probe()
        mcp.run(transport="streamable-http")
    else:
        mcp = build_server()
        startup.stdio()
        startup.run_startup_probe()
        mcp.run(transport="stdio")


if __name__ == "__main__":
    cli()
