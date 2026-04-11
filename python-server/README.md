# supercollider-mcp-python

Python port of the Rust SuperCollider MCP server, keeping the same tool names and parameters.

## Run

```bash
# stdio transport
python -m supercollider_mcp_py.main

# streamable HTTP transport
python -m supercollider_mcp_py.main --http --bind 0.0.0.0:8787
```

If you install the package, you can also run:

```bash
supercollider-mcp-python --http
```
