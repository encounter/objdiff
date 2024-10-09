use std::{fs, path::PathBuf, sync::mpsc::Receiver};

use anyhow::{anyhow, bail, Context, Result};
use const_format::formatcp;

use crate::{
    app::AppConfig,
    jobs::{
        objdiff::{run_make, BuildConfig, BuildStatus},
        start_job, update_status, Job, JobContext, JobResult, JobState,
    },
};

#[derive(Debug, Clone)]
pub struct CreateScratchConfig {
    pub build_config: BuildConfig,
    pub context_path: Option<PathBuf>,
    pub build_context: bool,

    // Scratch fields
    pub compiler: String,
    pub platform: String,
    pub compiler_flags: String,
    pub function_name: String,
    pub target_obj: PathBuf,
}

impl CreateScratchConfig {
    pub(crate) fn from_config(config: &AppConfig, function_name: String) -> Result<Self> {
        let Some(selected_obj) = &config.selected_obj else {
            bail!("No object selected");
        };
        let Some(target_path) = &selected_obj.target_path else {
            bail!("No target path for {}", selected_obj.name);
        };
        let Some(scratch_config) = &selected_obj.scratch else {
            bail!("No scratch configuration for {}", selected_obj.name);
        };
        Ok(Self {
            build_config: BuildConfig::from_config(config),
            context_path: scratch_config.ctx_path.clone(),
            build_context: scratch_config.build_ctx.unwrap_or(false),
            compiler: scratch_config.compiler.clone().unwrap_or_default(),
            platform: scratch_config.platform.clone().unwrap_or_default(),
            compiler_flags: scratch_config.c_flags.clone().unwrap_or_default(),
            function_name,
            target_obj: target_path.to_path_buf(),
        })
    }

    pub fn is_available(config: &AppConfig) -> bool {
        let Some(selected_obj) = &config.selected_obj else {
            return false;
        };
        selected_obj.target_path.is_some() && selected_obj.scratch.is_some()
    }
}

#[derive(Default, Debug, Clone)]
pub struct CreateScratchResult {
    pub scratch_url: String,
}

#[derive(Debug, Default, Clone, serde::Deserialize)]
struct CreateScratchResponse {
    pub slug: String,
    pub claim_token: String,
}

const API_HOST: &str = "https://decomp.me";

fn run_create_scratch(
    status: &JobContext,
    cancel: Receiver<()>,
    config: CreateScratchConfig,
) -> Result<Box<CreateScratchResult>> {
    let project_dir =
        config.build_config.project_dir.as_ref().ok_or_else(|| anyhow!("Missing project dir"))?;

    let mut context = None;
    if let Some(context_path) = &config.context_path {
        if config.build_context {
            update_status(status, "Building context".to_string(), 0, 2, &cancel)?;
            match run_make(&config.build_config, context_path) {
                BuildStatus { success: true, .. } => {}
                BuildStatus { success: false, stdout, stderr, .. } => {
                    bail!("Failed to build context:\n{stdout}\n{stderr}")
                }
            }
        }
        let context_path = project_dir.join(context_path);
        context = Some(
            fs::read_to_string(&context_path)
                .map_err(|e| anyhow!("Failed to read {}: {}", context_path.display(), e))?,
        );
    }

    update_status(status, "Creating scratch".to_string(), 1, 2, &cancel)?;
    let diff_flags = [format!("--disassemble={}", config.function_name)];
    let diff_flags = serde_json::to_string(&diff_flags).unwrap();
    let obj_path = project_dir.join(&config.target_obj);
    let file = reqwest::blocking::multipart::Part::file(&obj_path)
        .with_context(|| format!("Failed to open {}", obj_path.display()))?;
    let form = reqwest::blocking::multipart::Form::new()
        .text("compiler", config.compiler.clone())
        .text("platform", config.platform.clone())
        .text("compiler_flags", config.compiler_flags.clone())
        .text("diff_label", config.function_name.clone())
        .text("diff_flags", diff_flags)
        .text("context", context.unwrap_or_default())
        .text("source_code", "// Move related code from Context tab to here")
        .part("target_obj", file);
    let client = reqwest::blocking::Client::new();
    let response = client
        .post(formatcp!("{API_HOST}/api/scratch"))
        .multipart(form)
        .send()
        .map_err(|e| anyhow!("Failed to send request: {}", e))?;
    if !response.status().is_success() {
        return Err(anyhow!("Failed to create scratch: {}", response.text()?));
    }
    let body: CreateScratchResponse = response.json().context("Failed to parse response")?;
    let scratch_url = format!("{API_HOST}/scratch/{}/claim?token={}", body.slug, body.claim_token);

    update_status(status, "Complete".to_string(), 2, 2, &cancel)?;
    Ok(Box::from(CreateScratchResult { scratch_url }))
}

pub fn start_create_scratch(ctx: &egui::Context, config: CreateScratchConfig) -> JobState {
    start_job(ctx, "Create scratch", Job::CreateScratch, move |context, cancel| {
        run_create_scratch(&context, cancel, config)
            .map(|result| JobResult::CreateScratch(Some(result)))
    })
}
