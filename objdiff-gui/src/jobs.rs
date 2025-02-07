use std::{
    sync::Arc,
    task::{Wake, Waker},
};

use anyhow::{bail, Result};
use jobs::create_scratch;
use objdiff_core::{
    build::BuildConfig,
    diff::MappingConfig,
    jobs,
    jobs::{check_update::CheckUpdateConfig, objdiff, update::UpdateConfig, Job, JobQueue},
};

use crate::{
    app::{AppConfig, AppState},
    update::{build_updater, BIN_NAME_NEW, BIN_NAME_OLD},
};

struct EguiWaker(egui::Context);

impl Wake for EguiWaker {
    fn wake(self: Arc<Self>) { self.0.request_repaint(); }

    fn wake_by_ref(self: &Arc<Self>) { self.0.request_repaint(); }
}

pub fn egui_waker(ctx: &egui::Context) -> Waker { Waker::from(Arc::new(EguiWaker(ctx.clone()))) }

pub fn is_create_scratch_available(config: &AppConfig) -> bool {
    let Some(selected_obj) = &config.selected_obj else {
        return false;
    };
    selected_obj.target_path.is_some() && selected_obj.scratch.is_some()
}

pub fn start_create_scratch(
    ctx: &egui::Context,
    jobs: &mut JobQueue,
    state: &AppState,
    function_name: String,
) {
    match create_scratch_config(state, function_name) {
        Ok(config) => {
            jobs.push_once(Job::CreateScratch, || {
                create_scratch::start_create_scratch(egui_waker(ctx), config)
            });
        }
        Err(err) => {
            log::error!("Failed to create scratch config: {err}");
        }
    }
}

fn create_scratch_config(
    state: &AppState,
    function_name: String,
) -> Result<create_scratch::CreateScratchConfig> {
    let Some(selected_obj) = &state.config.selected_obj else {
        bail!("No object selected");
    };
    let Some(target_path) = &selected_obj.target_path else {
        bail!("No target path for {}", selected_obj.name);
    };
    let Some(scratch_config) = &selected_obj.scratch else {
        bail!("No scratch configuration for {}", selected_obj.name);
    };
    Ok(create_scratch::CreateScratchConfig {
        build_config: BuildConfig::from(&state.config),
        context_path: scratch_config.ctx_path.clone(),
        build_context: scratch_config.build_ctx.unwrap_or(false),
        compiler: scratch_config.compiler.clone().unwrap_or_default(),
        platform: scratch_config.platform.clone().unwrap_or_default(),
        compiler_flags: scratch_config.c_flags.clone().unwrap_or_default(),
        function_name,
        target_obj: target_path.clone(),
        preset_id: scratch_config.preset_id,
    })
}

impl From<&AppConfig> for BuildConfig {
    fn from(config: &AppConfig) -> Self {
        Self {
            project_dir: config.project_dir.clone(),
            custom_make: config.custom_make.clone(),
            custom_args: config.custom_args.clone(),
            selected_wsl_distro: config.selected_wsl_distro.clone(),
        }
    }
}

pub fn create_objdiff_config(state: &AppState) -> objdiff::ObjDiffConfig {
    objdiff::ObjDiffConfig {
        build_config: BuildConfig::from(&state.config),
        build_base: state.config.build_base,
        build_target: state.config.build_target,
        target_path: state
            .config
            .selected_obj
            .as_ref()
            .and_then(|obj| obj.target_path.as_ref())
            .cloned(),
        base_path: state
            .config
            .selected_obj
            .as_ref()
            .and_then(|obj| obj.base_path.as_ref())
            .cloned(),
        diff_obj_config: state.config.diff_obj_config.clone(),
        mapping_config: MappingConfig {
            mappings: state
                .config
                .selected_obj
                .as_ref()
                .map(|obj| &obj.symbol_mappings)
                .cloned()
                .unwrap_or_default(),
            selecting_left: state.selecting_left.clone(),
            selecting_right: state.selecting_right.clone(),
        },
    }
}

pub fn start_build(ctx: &egui::Context, jobs: &mut JobQueue, config: objdiff::ObjDiffConfig) {
    jobs.push_once(Job::ObjDiff, || objdiff::start_build(egui_waker(ctx), config));
}

pub fn start_check_update(ctx: &egui::Context, jobs: &mut JobQueue) {
    jobs.push_once(Job::Update, || {
        jobs::check_update::start_check_update(egui_waker(ctx), CheckUpdateConfig {
            build_updater,
            bin_names: vec![BIN_NAME_NEW.to_string(), BIN_NAME_OLD.to_string()],
        })
    });
}

pub fn start_update(ctx: &egui::Context, jobs: &mut JobQueue, bin_name: String) {
    jobs.push_once(Job::Update, || {
        jobs::update::start_update(egui_waker(ctx), UpdateConfig { build_updater, bin_name })
    });
}
