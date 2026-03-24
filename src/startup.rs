//! One-shot stderr banners (tool lists, URLs).  
//! **Important:** with stdio transport, MCP JSON-RPC must stay on **stdout**; all startup hints go here.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};

pub fn stdio() {
    eprintln!(
        "[supercollider-mcp] MCP over stdio (default). JSON-RPC on stdout; logs on stderr. Use --http for Streamable HTTP."
    );
    eprintln!(
        "[supercollider-mcp] tools: ping_supercollider, get_servers, discover_supercollider, get_server_status, detect_supercollider_install, get_supercollider_version, get_server_docs, list_server_candidates, get_docs_index_status, get_mcp_tool_routing_hints, refresh_supercollider_docs_index, search_supercollider_docs, answer_supercollider_docs, check_sclang_syntax, execute_supercollider_code"
    );
}

pub fn streamable_http(addr: SocketAddr) {
    eprintln!("[supercollider-mcp] listening on http://{addr}/mcp");
    if addr.ip() == IpAddr::V4(Ipv4Addr::UNSPECIFIED) {
        eprintln!(
            "[supercollider-mcp] Open WebUI URL: http://127.0.0.1:{}/mcp (use 127.0.0.1, not localhost, if tools fail)",
            addr.port()
        );
    }
    eprintln!(
        "[supercollider-mcp] tools: ping_supercollider, get_servers, discover_supercollider, get_server_status, detect_supercollider_install, get_supercollider_version, get_server_docs, list_server_candidates, get_docs_index_status, get_mcp_tool_routing_hints, refresh_supercollider_docs_index, search_supercollider_docs, answer_supercollider_docs, check_sclang_syntax, execute_supercollider_code · Ctrl+C to stop"
    );
}
