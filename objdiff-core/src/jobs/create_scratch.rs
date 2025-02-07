use std::{fs, sync::mpsc::Receiver, task::Waker};

use anyhow::{anyhow, bail, Context, Result};
use typed_path::{Utf8PlatformPathBuf, Utf8UnixPathBuf};

use crate::{
    build::{run_make, BuildConfig, BuildStatus},
    jobs::{start_job, update_status, Job, JobContext, JobResult, JobState},
};

#[derive(Debug, Clone)]
pub struct CreateScratchConfig {
    pub build_config: BuildConfig,
    pub context_path: Option<Utf8UnixPathBuf>,
    pub build_context: bool,

    // Scratch fields
    pub compiler: String,
    pub platform: String,
    pub compiler_flags: String,
    pub function_name: String,
    pub target_obj: Utf8PlatformPathBuf,
    pub preset_id: Option<u32>,
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
            match run_make(&config.build_config, context_path.as_ref()) {
                BuildStatus { success: true, .. } => {}
                BuildStatus { success: false, stdout, stderr, .. } => {
                    bail!("Failed to build context:\n{stdout}\n{stderr}")
                }
            }
        }
        let context_path = project_dir.join(context_path.with_platform_encoding());
        context = Some(
            fs::read_to_string(&context_path)
                .map_err(|e| anyhow!("Failed to read {}: {}", context_path, e))?,
        );
    }

    update_status(status, "Creating scratch".to_string(), 1, 2, &cancel)?;
    let diff_flags = [format!("--disassemble={}", config.function_name)];
    let diff_flags = serde_json::to_string(&diff_flags)?;
    let file = reqwest::blocking::multipart::Part::file(&config.target_obj)
        .with_context(|| format!("Failed to open {}", config.target_obj))?;
    let mut form = reqwest::blocking::multipart::Form::new()
        .text("compiler", config.compiler.clone())
        .text("platform", config.platform.clone())
        .text("compiler_flags", config.compiler_flags.clone())
        .text("diff_label", config.function_name.clone())
        .text("diff_flags", diff_flags)
        .text("context", context.unwrap_or_default())
        .text("source_code", "// Move related code from Context tab to here");
    if let Some(preset) = config.preset_id {
        form = form.text("preset", preset.to_string());
    }
    form = form.part("target_obj", file);
    let client = reqwest::blocking::Client::new();
    let response = client
        .post(format!("{API_HOST}/api/scratch"))
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

pub fn start_create_scratch(waker: Waker, config: CreateScratchConfig) -> JobState {
    start_job(waker, "Create scratch", Job::CreateScratch, move |context, cancel| {
        run_create_scratch(&context, cancel, config)
            .map(|result| JobResult::CreateScratch(Some(result)))
    })
}
