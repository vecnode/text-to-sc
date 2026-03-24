//! Streamable HTTP transport for MCP (`/mcp` on Axum).
//!
//! Middleware adapts `Accept` headers so clients that omit SSE still get a valid session.

use std::net::SocketAddr;

use anyhow::{Context, Result};
use axum::extract::Request;
use axum::http::{header::ACCEPT, HeaderValue, Method};
use axum::middleware::{self, Next};
use axum::response::Response;
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use tokio_util::sync::CancellationToken;

use crate::{server::SupercolliderMcpServer, startup};

/// rmcp's Streamable HTTP handler requires strict `Accept` headers. Open WebUI often
/// omits `text/event-stream` on POST or `Accept` on GET, which yields 406 and surfaces
/// in the UI as "Failed to connect".
async fn normalize_mcp_accept(mut req: Request, next: Next) -> Response {
    match *req.method() {
        Method::POST => {
            let ok = req
                .headers()
                .get(&ACCEPT)
                .and_then(|h| h.to_str().ok())
                .is_some_and(|h| h.contains("application/json") && h.contains("text/event-stream"));
            if !ok {
                req.headers_mut().insert(
                    ACCEPT,
                    HeaderValue::from_static("application/json, text/event-stream"),
                );
            }
        }
        Method::GET => {
            let ok = req
                .headers()
                .get(&ACCEPT)
                .and_then(|h| h.to_str().ok())
                .is_some_and(|h| h.contains("text/event-stream"));
            if !ok {
                req.headers_mut()
                    .insert(ACCEPT, HeaderValue::from_static("text/event-stream"));
            }
        }
        _ => {}
    }
    next.run(req).await
}

async fn log_requests(req: Request, next: Next) -> Response {
    eprintln!("[supercollider-mcp] {} {}", req.method(), req.uri());
    next.run(req).await
}

pub async fn run(addr: SocketAddr) -> Result<()> {
    let ct = CancellationToken::new();
    let service = StreamableHttpService::new(
        || Ok(SupercolliderMcpServer::new()),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig {
            cancellation_token: ct.child_token(),
            ..Default::default()
        },
    );

    let router = axum::Router::new()
        .nest_service("/mcp", service)
        .layer(middleware::from_fn(log_requests))
        .layer(middleware::from_fn(normalize_mcp_accept));

    let tcp_listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind HTTP listener on {addr}"))?;

    startup::streamable_http(addr);

    axum::serve(tcp_listener, router)
        .with_graceful_shutdown(async move {
            match tokio::signal::ctrl_c().await {
                Ok(()) => {
                    ct.cancel();
                }
                Err(e) => eprintln!("[supercollider-mcp] ctrl_c handler error: {e}"),
            }
        })
        .await?;

    Ok(())
}
