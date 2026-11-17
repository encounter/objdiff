//! `objdiff-cli mcp`: a Model Context Protocol server exposing objdiff's
//! diffing for AI-driven decompilation matching.
//!
//! Runs persistently ("prompt as you go") over either transport:
//!   * `--transport stdio` (default) for a client that spawns it (e.g. `.mcp.json`)
//!   * `--transport http`  for a long-lived shared instance reachable over the network

mod diff;
mod project;
mod server;
mod state;
mod wire;

use std::{path::PathBuf, sync::Arc};

use anyhow::{Result, bail};
use argp::FromArgs;
use axum::{
    body::{Body, to_bytes},
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use futures_util::StreamExt;
use rmcp::{
    ServiceExt,
    transport::{
        stdio,
        streamable_http_server::{
            StreamableHttpService, session::local::LocalSessionManager,
            tower::StreamableHttpServerConfig,
        },
    },
};

use self::{
    server::ObjdiffServer,
    state::AppState,
    wire::{LoggingRead, LoggingWrite},
};

#[derive(FromArgs, PartialEq, Debug)]
/// Starts an MCP server exposing objdiff's diffing for decompilation matching.
#[argp(subcommand, name = "mcp")]
pub struct Args {
    #[argp(option, default = "String::from(\"stdio\")")]
    /// Transport: "stdio" or "http". (Default: stdio)
    transport: String,
    #[argp(option, default = "String::from(\"127.0.0.1:3001\")")]
    /// Bind address for the http transport. (Default: 127.0.0.1:3001)
    bind: String,
    #[argp(option)]
    /// Optionally remember a project directory on startup.
    project: Option<PathBuf>,
}

/// Largest request body buffered for logging on the http transport.
const HTTP_BODY_LIMIT: usize = 8 * 1024 * 1024;

/// Log both sides of an http request. Responses are usually SSE streams, so
/// their body is logged chunk by chunk as it is produced rather than buffered.
async fn log_http(request: Request, next: Next) -> Response {
    let (parts, body) = request.into_parts();
    let bytes = match to_bytes(body, HTTP_BODY_LIMIT).await {
        Ok(bytes) => bytes,
        Err(e) => {
            tracing::warn!(
                "{} {} {}: failed to read body: {e}",
                wire::RECV,
                parts.method,
                parts.uri
            );
            return StatusCode::PAYLOAD_TOO_LARGE.into_response();
        }
    };
    tracing::info!("{} {} {}", wire::RECV, parts.method, parts.uri);
    wire::log_payload(wire::RECV, &bytes);

    let response = next.run(Request::from_parts(parts, Body::from(bytes))).await;

    let (parts, body) = response.into_parts();
    tracing::info!("{} {}", wire::SEND, parts.status);
    let body = Body::from_stream(body.into_data_stream().map(|chunk| {
        match &chunk {
            Ok(bytes) => wire::log_payload(wire::SEND, bytes),
            Err(e) => tracing::warn!("{} response body error: {e}", wire::SEND),
        }
        chunk
    }));
    Response::from_parts(parts, body)
}

pub fn run(args: Args) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    runtime.block_on(run_async(args))
}

async fn run_async(args: Args) -> Result<()> {
    let app_state = Arc::new(AppState::new());
    if let Some(dir) = args.project.as_deref() {
        match app_state.open_project(&dir.to_string_lossy()) {
            Ok(summary) => tracing::info!("{summary}"),
            Err(e) => tracing::warn!("Failed to open project: {e:#}"),
        }
    }

    match args.transport.as_str() {
        "stdio" => {
            // Logs go to stderr (configured in main); stdout is the MCP protocol channel.
            tracing::info!("objdiff mcp server starting on stdio");
            let (stdin, stdout) = stdio();
            let transport = (LoggingRead::new(stdin), LoggingWrite::new(stdout));
            let service = ObjdiffServer::new(app_state).serve(transport).await?;
            service.waiting().await?;
        }
        "http" => {
            let factory_state = app_state.clone();
            let service = StreamableHttpService::new(
                move || Ok(ObjdiffServer::new(factory_state.clone())),
                Arc::new(LocalSessionManager::default()),
                StreamableHttpServerConfig::default(),
            );
            let router = axum::Router::new()
                .nest_service("/mcp", service)
                .layer(axum::middleware::from_fn(log_http));
            let listener = tokio::net::TcpListener::bind(&args.bind).await?;
            tracing::info!("objdiff mcp server listening on http://{}/mcp", args.bind);
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = tokio::signal::ctrl_c().await;
                    tracing::info!("shutting down");
                })
                .await?;
        }
        other => {
            bail!("Unknown transport `{other}` (expected `stdio` or `http`)");
        }
    }
    Ok(())
}
