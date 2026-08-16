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

use std::{path::PathBuf, sync::Arc};

use anyhow::{Result, bail};
use argp::FromArgs;
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

use self::{server::ObjdiffServer, state::AppState};

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
