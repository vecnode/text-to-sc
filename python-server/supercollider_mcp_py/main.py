from __future__ import annotations

import runpy


def _run_src_entrypoint() -> None:
    runpy.run_module("src.supercollider_mcp_py.main", run_name="__main__")


if __name__ == "__main__":
    _run_src_entrypoint()
