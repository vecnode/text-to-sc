use rmcp::{
    ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
};
use serde::{Deserialize, Serialize};

use crate::{sc_docs, sc_process, supercollider_model::SupercolliderServerState};

#[derive(Clone)]
pub struct SupercolliderMcpServer {
    tool_router: ToolRouter<Self>,
    #[allow(dead_code)]
    state: SupercolliderServerState,
    // TODO: graph tools + OSC will use `state`.
}

impl SupercolliderMcpServer {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
            state: SupercolliderServerState::bootstrap_placeholder(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct PingParams {
    /// Optional note (echoed in the reply).
    pub message: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct EmptyParams {}

#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ServerStatusParams {
    /// Optional server process PID. If omitted, MCP picks the first live server.
    pub pid: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DocsSearchParams {
    /// Natural language query, symbol, class, or keyword.
    pub query: String,
    /// Maximum results to return (1..12, default 5).
    pub max_results: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DocsAnswerParams {
    /// Natural language question answered from indexed SuperCollider docs.
    pub question: String,
}

#[tool_router]
impl SupercolliderMcpServer {
    /// Simple ping tool that will later check connectivity with SuperCollider.
    #[tool(
        description = "Quick SuperCollider health check: scans scsynth/supernova/sclang processes, probes OSC /status on likely ports, and reports install detection."
    )]
    async fn ping_supercollider(&self, Parameters(params): Parameters<PingParams>) -> String {
        eprintln!(
            "[supercollider-mcp] tool start: ping_supercollider params={params:?}"
        );

        let msg = params.message.clone();
        let report = match tokio::task::spawn_blocking(move || sc_process::probe(msg.as_deref())).await {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "[supercollider-mcp] tool error: ping_supercollider — spawn_blocking join failed: {e}"
                );
                return format!("ping_supercollider failed: {e}");
            }
        };

        eprintln!("[supercollider-mcp] tool ok: ping_supercollider — finished successfully");

        report
    }

    #[tool(
        description = "List running SuperCollider server processes (scsynth/supernova) with PID, CPU, memory, candidate OSC ports, responding OSC port, and install-path match status."
    )]
    async fn get_servers(&self, Parameters(_p): Parameters<EmptyParams>) -> String {
        eprintln!("[supercollider-mcp] tool start: get_servers");

        let report = match tokio::task::spawn_blocking(sc_process::get_servers).await {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "[supercollider-mcp] tool error: get_servers — spawn_blocking join failed: {e}"
                );
                return format!("get_servers failed: {e}");
            }
        };

        eprintln!("[supercollider-mcp] tool ok: get_servers — finished successfully");

        report
    }

    #[tool(
        description = "High-fidelity SuperCollider discovery: process inventory, OSC probe results across candidate ports, install matching, and machine-friendly JSON."
    )]
    async fn discover_supercollider(&self, Parameters(_p): Parameters<EmptyParams>) -> String {
        eprintln!("[supercollider-mcp] tool start: discover_supercollider");

        let report = match tokio::task::spawn_blocking(sc_process::discover_supercollider).await {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "[supercollider-mcp] tool error: discover_supercollider — spawn_blocking join failed: {e}"
                );
                return format!("discover_supercollider failed: {e}");
            }
        };

        eprintln!("[supercollider-mcp] tool ok: discover_supercollider — finished successfully");
        report
    }

    #[tool(
        description = "Get status for a specific SuperCollider server PID (or auto-select active server): OSC reachability, responding port, memory, CPU, and install match."
    )]
    async fn get_server_status(
        &self,
        Parameters(params): Parameters<ServerStatusParams>,
    ) -> String {
        eprintln!("[supercollider-mcp] tool start: get_server_status params={params:?}");

        let report =
            match tokio::task::spawn_blocking(move || sc_process::get_server_status(params.pid))
                .await
            {
                Ok(s) => s,
                Err(e) => {
                    eprintln!(
                        "[supercollider-mcp] tool error: get_server_status — spawn_blocking join failed: {e}"
                    );
                    return format!("get_server_status failed: {e}");
                }
            };

        eprintln!("[supercollider-mcp] tool ok: get_server_status — finished successfully");
        report
    }

    #[tool(
        description = "Detect SuperCollider installation and executable paths on this machine (sclang/scsynth/supernova)."
    )]
    async fn detect_supercollider_install(
        &self,
        Parameters(_p): Parameters<EmptyParams>,
    ) -> String {
        eprintln!("[supercollider-mcp] tool start: detect_supercollider_install");

        let report = match tokio::task::spawn_blocking(sc_process::detect_supercollider_install).await
        {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "[supercollider-mcp] tool error: detect_supercollider_install — spawn_blocking join failed: {e}"
                );
                return format!("detect_supercollider_install failed: {e}");
            }
        };

        eprintln!("[supercollider-mcp] tool ok: detect_supercollider_install — finished successfully");
        report
    }

    #[tool(
        description = "Return local SuperCollider documentation paths (if installed) and key online docs links."
    )]
    async fn get_server_docs(&self, Parameters(_p): Parameters<EmptyParams>) -> String {
        eprintln!("[supercollider-mcp] tool start: get_server_docs");

        let report = match tokio::task::spawn_blocking(sc_process::get_server_docs).await {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "[supercollider-mcp] tool error: get_server_docs — spawn_blocking join failed: {e}"
                );
                return format!("get_server_docs failed: {e}");
            }
        };

        eprintln!("[supercollider-mcp] tool ok: get_server_docs — finished successfully");
        report
    }

    #[tool(
        description = "List all SuperCollider-related process candidates (scsynth/supernova/sclang/scide) including PID, executable path, and command line for debugging."
    )]
    async fn list_server_candidates(&self, Parameters(_p): Parameters<EmptyParams>) -> String {
        eprintln!("[supercollider-mcp] tool start: list_server_candidates");

        let report = match tokio::task::spawn_blocking(sc_process::list_server_candidates).await {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "[supercollider-mcp] tool error: list_server_candidates — spawn_blocking join failed: {e}"
                );
                return format!("list_server_candidates failed: {e}");
            }
        };

        eprintln!("[supercollider-mcp] tool ok: list_server_candidates — finished successfully");
        report
    }

    #[tool(
        description = "Build or refresh local SuperCollider docs index from the installed Help directory."
    )]
    async fn refresh_supercollider_docs_index(
        &self,
        Parameters(_p): Parameters<EmptyParams>,
    ) -> String {
        eprintln!("[supercollider-mcp] tool start: refresh_supercollider_docs_index");
        let report = match tokio::task::spawn_blocking(sc_docs::refresh_supercollider_docs_index).await
        {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "[supercollider-mcp] tool error: refresh_supercollider_docs_index — spawn_blocking join failed: {e}"
                );
                return format!("refresh_supercollider_docs_index failed: {e}");
            }
        };
        eprintln!("[supercollider-mcp] tool ok: refresh_supercollider_docs_index — finished successfully");
        report
    }

    #[tool(
        description = "Search local SuperCollider docs with ranked snippets and citations."
    )]
    async fn search_supercollider_docs(
        &self,
        Parameters(params): Parameters<DocsSearchParams>,
    ) -> String {
        eprintln!("[supercollider-mcp] tool start: search_supercollider_docs params={params:?}");
        let query = params.query;
        let max_results = params.max_results.unwrap_or(5);
        let report = match tokio::task::spawn_blocking(move || {
            sc_docs::search_supercollider_docs(&query, max_results)
        })
        .await
        {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "[supercollider-mcp] tool error: search_supercollider_docs — spawn_blocking join failed: {e}"
                );
                return format!("search_supercollider_docs failed: {e}");
            }
        };
        eprintln!("[supercollider-mcp] tool ok: search_supercollider_docs — finished successfully");
        report
    }

    #[tool(
        description = "Answer a SuperCollider question grounded in local docs, returning evidence snippets and citations."
    )]
    async fn answer_supercollider_docs(
        &self,
        Parameters(params): Parameters<DocsAnswerParams>,
    ) -> String {
        eprintln!("[supercollider-mcp] tool start: answer_supercollider_docs params={params:?}");
        let question = params.question;
        let report = match tokio::task::spawn_blocking(move || {
            sc_docs::answer_supercollider_docs(&question)
        })
        .await
        {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "[supercollider-mcp] tool error: answer_supercollider_docs — spawn_blocking join failed: {e}"
                );
                return format!("answer_supercollider_docs failed: {e}");
            }
        };
        eprintln!("[supercollider-mcp] tool ok: answer_supercollider_docs — finished successfully");
        report
    }

    #[tool(
        description = "Get detected SuperCollider version from install path and sclang -v (when available), plus running binary paths/version hints."
    )]
    async fn get_supercollider_version(&self, Parameters(_p): Parameters<EmptyParams>) -> String {
        eprintln!("[supercollider-mcp] tool start: get_supercollider_version");
        let report = match tokio::task::spawn_blocking(sc_process::get_supercollider_version).await {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "[supercollider-mcp] tool error: get_supercollider_version — spawn_blocking join failed: {e}"
                );
                return format!("get_supercollider_version failed: {e}");
            }
        };
        eprintln!("[supercollider-mcp] tool ok: get_supercollider_version — finished successfully");
        report
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for SupercolliderMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "Use ping_supercollider for quick health; get_servers for detailed server stats; discover_supercollider for full JSON discovery; get_server_status for one PID; detect_supercollider_install/get_supercollider_version/get_server_docs for install/version/docs paths; refresh_supercollider_docs_index/search_supercollider_docs/answer_supercollider_docs for grounded docs retrieval and QA.".to_string(),
        )
    }
}
