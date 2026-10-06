//! MCP server surface: tool schemas (`rmcp` macros), `tokio::task::spawn_blocking` wrappers for blocking work,
//! and `ServerHandler` metadata consumed by clients (e.g. Open WebUI).

use rmcp::{
    ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
};
use serde::{Deserialize, Serialize};

use crate::{sc_docs, sc_process, supercollider_model::SupercolliderServerState};

/// Root MCP service; holds the generated tool router and placeholder graph state.
#[derive(Clone)]
pub struct SupercolliderMcpServer {
    tool_router: ToolRouter<Self>,
    #[allow(dead_code)]
    state: SupercolliderServerState,
    // Future: graph / OSC tools will read and update `state`.
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
    /// Response shape: `text` (default) or `json` (structured hits and citations).
    pub output: Option<String>,
    /// Docs source: `local` (default) or `auto`. `web` is not supported (offline-first).
    pub source: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DocsAnswerParams {
    /// Natural language question answered from indexed SuperCollider docs.
    pub question: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SclangSyntaxParams {
    /// SuperCollider language source to compile-check (not executed; no audio).
    pub code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct StopSupercolliderSynthsParams {
    /// Target `scsynth` / `supernova` PID from `get_servers`. Omit to use first OSC-reachable server.
    pub server_pid: Option<u32>,
    /// Override UDP port scsynth listens on (e.g. 57110). Omit to use PID / auto detection.
    pub osc_port: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ExecuteSupercolliderParams {
    /// sclang to run on the live server. Examples: `Synth(\\default);` or `x = Synth(\\default);`. For audio from a UGen function use `{ SinOsc.ar * 0.1 }.play;`. Do not use `{ Synth(\\default) }.play` (that wraps Synth in a Function; `.play` on `{ }` is only for functions that return a UGen graph). To stop all synths use tool `stop_supercollider_synths` or `Server.default.freeAll;` (not `Synth.freeAll`).
    pub code: String,
    /// Target `scsynth` / `supernova` PID from `get_servers`. Omit to use first OSC-reachable server.
    pub server_pid: Option<u32>,
    /// Override UDP port scsynth listens on (e.g. 57110). Omit to use PID / auto detection.
    pub osc_port: Option<u16>,
}

#[tool_router]
impl SupercolliderMcpServer {
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
        description = "Search local SuperCollider docs with ranked snippets and citations. Set output=json for structured machine-readable hits. source=web is unsupported (offline-first)."
    )]
    async fn search_supercollider_docs(
        &self,
        Parameters(params): Parameters<DocsSearchParams>,
    ) -> String {
        eprintln!("[supercollider-mcp] tool start: search_supercollider_docs params={params:?}");
        let query = params.query;
        let max_results = params.max_results.unwrap_or(5);
        let output = params.output.unwrap_or_else(|| "text".to_string());
        let source_owned = params.source;
        let report = match tokio::task::spawn_blocking(move || {
            sc_docs::search_supercollider_docs(
                &query,
                max_results,
                &output,
                source_owned.as_deref(),
            )
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
        description = "Report SuperCollider docs index status: resolved Help/HelpSource root, on-disk doc file count, and whether the in-memory index is loaded (without building it)."
    )]
    async fn get_docs_index_status(&self, Parameters(_p): Parameters<EmptyParams>) -> String {
        eprintln!("[supercollider-mcp] tool start: get_docs_index_status");
        let report = match tokio::task::spawn_blocking(sc_docs::get_docs_index_status).await {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "[supercollider-mcp] tool error: get_docs_index_status — spawn_blocking join failed: {e}"
                );
                return format!("get_docs_index_status failed: {e}");
            }
        };
        eprintln!("[supercollider-mcp] tool ok: get_docs_index_status — finished successfully");
        report
    }

    #[tool(
        description = "Return a concise routing table: which MCP tool to use for server health, version, docs index, search, and grounded Q&A (offline-first)."
    )]
    async fn get_mcp_tool_routing_hints(&self, Parameters(_p): Parameters<EmptyParams>) -> String {
        eprintln!("[supercollider-mcp] tool start: get_mcp_tool_routing_hints");
        let report = sc_docs::get_mcp_tool_routing_hints();
        eprintln!("[supercollider-mcp] tool ok: get_mcp_tool_routing_hints — finished successfully");
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

    #[tool(
        description = "Compile-check sclang source with the installed sclang (snippet only, not executed; class library still loads — ~0.5–2s typical)."
    )]
    async fn check_sclang_syntax(&self, Parameters(params): Parameters<SclangSyntaxParams>) -> String {
        eprintln!("[supercollider-mcp] tool start: check_sclang_syntax ({} bytes)", params.code.len());
        let code = params.code;
        let report = match tokio::task::spawn_blocking(move || sc_process::check_sclang_syntax(&code)).await {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "[supercollider-mcp] tool error: check_sclang_syntax — spawn_blocking join failed: {e}"
                );
                return format!("check_sclang_syntax failed: {e}");
            }
        };
        eprintln!("[supercollider-mcp] tool ok: check_sclang_syntax — finished successfully");
        report
    }

    #[tool(
        description = "SECURITY: runs arbitrary sclang against your live scsynth (Server.remote + interpret). Prefer check_sclang_syntax first. Pass server_pid/osc_port from get_servers or omit for auto-target. For default synth use `Synth(\\default);` not `{ Synth(\\default) }.play`. Trust this MCP only on your own machine."
    )]
    async fn execute_supercollider_code(
        &self,
        Parameters(params): Parameters<ExecuteSupercolliderParams>,
    ) -> String {
        eprintln!(
            "[supercollider-mcp] tool start: execute_supercollider_code ({} bytes) pid={:?} port={:?}",
            params.code.len(),
            params.server_pid,
            params.osc_port
        );
        let code = params.code;
        let server_pid = params.server_pid;
        let osc_port = params.osc_port;
        let report = match tokio::task::spawn_blocking(move || {
            sc_process::execute_supercollider_code(&code, server_pid, osc_port)
        })
        .await
        {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "[supercollider-mcp] tool error: execute_supercollider_code — spawn_blocking join failed: {e}"
                );
                return format!("execute_supercollider_code failed: {e}");
            }
        };
        eprintln!("[supercollider-mcp] tool ok: execute_supercollider_code — finished");
        report
    }

    #[tool(
        description = "Stops all synthesis controlled by this MCP client on the target scsynth: runs `Server.default.freeAll` via headless sclang (same trust model as execute_supercollider_code). Use for “stop sound / silence / free synths”. Not `Synth.freeAll` (invalid)."
    )]
    async fn stop_supercollider_synths(
        &self,
        Parameters(params): Parameters<StopSupercolliderSynthsParams>,
    ) -> String {
        eprintln!(
            "[supercollider-mcp] tool start: stop_supercollider_synths pid={:?} port={:?}",
            params.server_pid, params.osc_port
        );
        let server_pid = params.server_pid;
        let osc_port = params.osc_port;
        let report = match tokio::task::spawn_blocking(move || {
            sc_process::stop_supercollider_synths(server_pid, osc_port)
        })
        .await
        {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "[supercollider-mcp] tool error: stop_supercollider_synths — spawn_blocking join failed: {e}"
                );
                return format!("stop_supercollider_synths failed: {e}");
            }
        };
        eprintln!("[supercollider-mcp] tool ok: stop_supercollider_synths — finished");
        report
    }

    #[tool(
        description = "Stops the scsynth/supernova process via raw OSC /quit (Server.quit/reboot do not work with Server.remote). Does not restart the binary — use reboot_supercollider_server or boot from IDE."
    )]
    async fn quit_supercollider_server(
        &self,
        Parameters(params): Parameters<StopSupercolliderSynthsParams>,
    ) -> String {
        eprintln!(
            "[supercollider-mcp] tool start: quit_supercollider_server pid={:?} port={:?}",
            params.server_pid, params.osc_port
        );
        let server_pid = params.server_pid;
        let osc_port = params.osc_port;
        let report = match tokio::task::spawn_blocking(move || {
            sc_process::quit_supercollider_server(server_pid, osc_port)
        })
        .await
        {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "[supercollider-mcp] tool error: quit_supercollider_server — spawn_blocking join failed: {e}"
                );
                return format!("quit_supercollider_server failed: {e}");
            }
        };
        eprintln!("[supercollider-mcp] tool ok: quit_supercollider_server — finished");
        report
    }

    #[tool(
        description = "OSC /quit then respawns scsynth or supernova with -u <port> only. Use when the model suggests s.reboot (that fails on Server.remote). Trusted host: kills and restarts the audio server process."
    )]
    async fn reboot_supercollider_server(
        &self,
        Parameters(params): Parameters<StopSupercolliderSynthsParams>,
    ) -> String {
        eprintln!(
            "[supercollider-mcp] tool start: reboot_supercollider_server pid={:?} port={:?}",
            params.server_pid, params.osc_port
        );
        let server_pid = params.server_pid;
        let osc_port = params.osc_port;
        let report = match tokio::task::spawn_blocking(move || {
            sc_process::reboot_supercollider_server(server_pid, osc_port)
        })
        .await
        {
            Ok(s) => s,
            Err(e) => {
                eprintln!(
                    "[supercollider-mcp] tool error: reboot_supercollider_server — spawn_blocking join failed: {e}"
                );
                return format!("reboot_supercollider_server failed: {e}");
            }
        };
        eprintln!("[supercollider-mcp] tool ok: reboot_supercollider_server — finished");
        report
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for SupercolliderMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "Use ping_supercollider for quick health; get_servers for detailed server stats; discover_supercollider for full JSON discovery; get_server_status for one PID; detect_supercollider_install/get_supercollider_version/get_server_docs for install/version/docs paths; get_docs_index_status before heavy docs use; refresh_supercollider_docs_index/search_supercollider_docs (output=json optional)/answer_supercollider_docs for grounded local docs; check_sclang_syntax to compile-check sclang; execute_supercollider_code / stop_supercollider_synths for synth graph control; quit_supercollider_server / reboot_supercollider_server for process-level /quit+respawn (raw OSC; s.reboot unavailable on remote); get_mcp_tool_routing_hints for intent→tool mapping.".to_string(),
        )
    }
}
