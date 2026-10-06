//! Binary entry point: MCP over **stdio** (default) or **Streamable HTTP** (`--http`).
//!
//! Logging for humans belongs on stderr; stdio transport keeps JSON-RPC on stdout only.

use std::net::SocketAddr;

use anyhow::{Context, Result};
use clap::Parser;
use rmcp::{transport::stdio, ServiceExt};

mod sc_process;
mod sc_docs;
mod server;
mod startup;
mod streamable_http;
mod supercollider_model;

use crate::server::SupercolliderMcpServer;

#[derive(Parser)]
#[command(name = "supercollider-mcp")]
struct Args {
    /// Run MCP over Streamable HTTP (Open WebUI, etc.) instead of stdio.
    #[arg(long)]
    http: bool,

    /// Bind address for HTTP mode. Use 0.0.0.0:8787 so Dockerized Open WebUI can reach the host.
    #[arg(long, default_value = "0.0.0.0:8787", value_name = "ADDR")]
    bind: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    if args.http {
        let addr: SocketAddr = args
            .bind
            .trim()
            .parse()
            .with_context(|| format!("invalid --bind {:?}", args.bind.trim()))?;
        streamable_http::run(addr).await
    } else {
        run_stdio().await
    }
}

async fn run_stdio() -> Result<()> {
    startup::stdio();

    let server = SupercolliderMcpServer::new().serve(stdio()).await?;
    eprintln!("[supercollider-mcp] ready — waiting for MCP requests");

    server.waiting().await?;

    Ok(())
}
