//! objdiff-mcp: a Model Context Protocol server exposing objdiff's diffing for
//! AI-driven decompilation matching.
//!
//! Runs persistently ("prompt as you go") over either transport:
//!   * `--transport stdio` (default) for a client that spawns it (e.g. `.mcp.json`)
//!   * `--transport http`  for a long-lived shared instance reachable over the network

mod diff;
mod project;
mod server;
mod state;

use std::sync::Arc;

use anyhow::Result;
use clap::Parser;
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

use crate::{server::ObjdiffServer, state::AppState};

#[derive(Parser, Debug)]
#[command(name = "objdiff-mcp", about = "MCP server for objdiff decompilation matching")]
struct Args {
    /// Transport: "stdio" or "http".
    #[arg(long, default_value = "stdio")]
    transport: String,
    /// Bind address for the http transport.
    #[arg(long, default_value = "127.0.0.1:3001")]
    bind: String,
    /// Optionally remember a project directory on startup.
    #[arg(long)]
    project: Option<std::path::PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Logs MUST go to stderr; stdout is the MCP protocol channel on stdio.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let args = Args::parse();
    let app_state = Arc::new(AppState::new());
    if let Some(dir) = args.project.as_deref() {
        match app_state.open_project(&dir.to_string_lossy()) {
            Ok(summary) => tracing::info!("{summary}"),
            Err(e) => tracing::warn!("Failed to open project: {e:#}"),
        }
    }

    match args.transport.as_str() {
        "stdio" => {
            tracing::info!("objdiff-mcp starting on stdio");
            let service = ObjdiffServer::new(app_state).serve(stdio()).await?;
            service.waiting().await?;
        }
        "http" => {
            let factory_state = app_state.clone();
            let service = StreamableHttpService::new(
                move || Ok(ObjdiffServer::new(factory_state.clone())),
                Arc::new(LocalSessionManager::default()),
                StreamableHttpServerConfig::default(),
            );
            let router = axum::Router::new().nest_service("/mcp", service);
            let listener = tokio::net::TcpListener::bind(&args.bind).await?;
            tracing::info!("objdiff-mcp listening on http://{}/mcp", args.bind);
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = tokio::signal::ctrl_c().await;
                    tracing::info!("shutting down");
                })
                .await?;
        }
        other => {
            anyhow::bail!("Unknown transport `{other}` (expected `stdio` or `http`)");
        }
    }
    Ok(())
}
