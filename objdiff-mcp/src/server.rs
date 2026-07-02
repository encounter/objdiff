//! rmcp server exposing objdiff's diffing as MCP tools.

use std::{collections::BTreeMap, sync::Arc};

use objdiff_core::build::run_make;
use rmcp::{
    ErrorData, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::{diff, project, state::AppState};

#[derive(Clone)]
pub struct ObjdiffServer {
    state: Arc<AppState>,
    // Read by the `#[tool_handler]`-generated dispatch code.
    #[allow(dead_code)]
    tool_router: ToolRouter<ObjdiffServer>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct OpenProjectArgs {
    /// Directory containing an `objdiff.json` (or `.yml`/`.yaml`).
    pub dir: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListUnitsArgs {
    /// Optional substring filter on unit names.
    #[serde(default)]
    pub filter: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BuildArgs {
    /// Unit (translation-unit) name to build, from the loaded project.
    pub unit: String,
    /// Build the target/expected object instead of your base build (default false).
    #[serde(default)]
    pub target: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DiffFunctionArgs {
    /// Unit name from the loaded project (alternative to target/base paths).
    #[serde(default)]
    pub unit: Option<String>,
    /// Path to the target/expected object file (the baseline). Use with `base`.
    #[serde(default)]
    pub target: Option<String>,
    /// Path to the base/current object file produced by your build. Use with `target`.
    #[serde(default)]
    pub base: Option<String>,
    /// Name of the function symbol to diff.
    pub symbol: String,
    /// Optional objdiff config overrides, e.g. `{"x86.formatter": "intel"}`.
    #[serde(default)]
    pub config: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DiffOverviewArgs {
    /// Unit name from the loaded project (alternative to target/base paths).
    #[serde(default)]
    pub unit: Option<String>,
    /// Path to the target/expected object file (the baseline). Use with `base`.
    #[serde(default)]
    pub target: Option<String>,
    /// Path to the base/current object file produced by your build. Use with `target`.
    #[serde(default)]
    pub base: Option<String>,
    /// Only list functions that are not already 100% matched.
    #[serde(default)]
    pub only_mismatches: bool,
    /// Maximum number of functions to list (default 200).
    #[serde(default)]
    pub limit: Option<usize>,
    /// Optional objdiff config overrides.
    #[serde(default)]
    pub config: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetConfigArgs {
    /// Config property id, e.g. `x86.formatter` or `spaceBetweenArgs`.
    pub key: String,
    /// New value for the property.
    pub value: String,
}

fn err(e: anyhow::Error) -> ErrorData { ErrorData::internal_error(format!("{e:#}"), None) }

fn join_err(e: tokio::task::JoinError) -> ErrorData {
    ErrorData::internal_error(format!("task join error: {e}"), None)
}

fn text(s: String) -> CallToolResult { CallToolResult::success(vec![ContentBlock::text(s)]) }

#[tool_router]
impl ObjdiffServer {
    pub fn new(state: Arc<AppState>) -> Self { Self { state, tool_router: Self::tool_router() } }

    #[tool(description = "Report the objdiff-mcp / objdiff-core version.")]
    async fn version(&self) -> Result<CallToolResult, ErrorData> {
        Ok(text(format!("objdiff-mcp {} (objdiff-core embedded)", env!("CARGO_PKG_VERSION"))))
    }

    #[tool(
        description = "Load an objdiff project (objdiff.json) so later tools can refer to units \
        by name instead of file paths."
    )]
    async fn open_project(
        &self,
        Parameters(args): Parameters<OpenProjectArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        Ok(text(self.state.open_project(&args.dir).map_err(err)?))
    }

    #[tool(description = "List units in the loaded project with their resolved target/base paths.")]
    async fn list_units(
        &self,
        Parameters(args): Parameters<ListUnitsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        Ok(text(self.state.list_units(args.filter.as_deref()).map_err(err)?))
    }

    #[tool(description = "Diff a single function between the target (expected/baseline) and base \
        (current/your build) objects. Specify either a project `unit` or explicit `target`+`base` \
        paths. Returns the overall match percent and a side-by-side, per-instruction diff with \
        mismatch markers. The primary tool for iterating C/C++ toward a 100% match.")]
    async fn diff_function(
        &self,
        Parameters(args): Parameters<DiffFunctionArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let inputs = self
            .state
            .prepare_diff(
                args.unit.as_deref(),
                args.target.as_deref(),
                args.base.as_deref(),
                &args.config,
            )
            .map_err(err)?;
        let symbol = args.symbol;
        let out = tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
            let result = diff::run_diff(&inputs.target, &inputs.base, &inputs.config)?;
            diff::function_diff(&result, &symbol)
        })
        .await
        .map_err(join_err)?
        .map_err(err)?;
        Ok(text(out))
    }

    #[tool(description = "List every function in the object pair with its match percent, worst \
        matches first. Specify either a project `unit` or explicit `target`+`base` paths.")]
    async fn diff_overview(
        &self,
        Parameters(args): Parameters<DiffOverviewArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let inputs = self
            .state
            .prepare_diff(
                args.unit.as_deref(),
                args.target.as_deref(),
                args.base.as_deref(),
                &args.config,
            )
            .map_err(err)?;
        let only = args.only_mismatches;
        let limit = args.limit.unwrap_or(200);
        let out = tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
            let result = diff::run_diff(&inputs.target, &inputs.base, &inputs.config)?;
            Ok(diff::overview(&result, only, limit))
        })
        .await
        .map_err(join_err)?
        .map_err(err)?;
        Ok(text(out))
    }

    #[tool(description = "Run the loaded project's build command for a unit's base (or target) \
        object. Returns the command line, exit status, and compiler output.")]
    async fn build(
        &self,
        Parameters(args): Parameters<BuildArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let (build_config, rel) =
            self.state.build_invocation(&args.unit, args.target).map_err(err)?;
        let status = tokio::task::spawn_blocking(move || run_make(&build_config, &rel))
            .await
            .map_err(join_err)?;
        Ok(text(project::build_status_text(&status)))
    }

    #[tool(description = "Set a persistent objdiff diff/disassembly config option (applies to \
        subsequent diffs). Example keys: `x86.formatter`, `spaceBetweenArgs`, `demangler`.")]
    async fn set_config(
        &self,
        Parameters(args): Parameters<SetConfigArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.state.set_config(&args.key, &args.value).map_err(err)?;
        Ok(text(format!("Set `{}` = `{}`", args.key, args.value)))
    }
}

#[tool_handler]
impl ServerHandler for ObjdiffServer {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.instructions = Some(
            "objdiff MCP server for decompilation matching. Load a project with open_project, \
            build a unit, then use diff_function to compare your build against the baseline and \
            iterate toward 100%."
                .to_string(),
        );
        info
    }
}
