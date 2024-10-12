use std::{path::PathBuf, sync::mpsc::Receiver, task::Waker};

use anyhow::{anyhow, Error, Result};
use time::OffsetDateTime;

use crate::{
    build::{run_make, BuildConfig, BuildStatus},
    config::SymbolMappings,
    diff::{diff_objs, DiffObjConfig, MappingConfig, ObjDiff},
    jobs::{start_job, update_status, Job, JobContext, JobResult, JobState},
    obj::{read, ObjInfo},
};

pub struct ObjDiffConfig {
    pub build_config: BuildConfig,
    pub build_base: bool,
    pub build_target: bool,
    pub target_path: Option<PathBuf>,
    pub base_path: Option<PathBuf>,
    pub diff_obj_config: DiffObjConfig,
    pub symbol_mappings: SymbolMappings,
    pub selecting_left: Option<String>,
    pub selecting_right: Option<String>,
}

pub struct ObjDiffResult {
    pub first_status: BuildStatus,
    pub second_status: BuildStatus,
    pub first_obj: Option<(ObjInfo, ObjDiff)>,
    pub second_obj: Option<(ObjInfo, ObjDiff)>,
    pub time: OffsetDateTime,
}

fn run_build(
    context: &JobContext,
    cancel: Receiver<()>,
    mut config: ObjDiffConfig,
) -> Result<Box<ObjDiffResult>> {
    // Use the per-object symbol mappings, we don't set mappings globally
    config.diff_obj_config.symbol_mappings = MappingConfig {
        mappings: config.symbol_mappings,
        selecting_left: config.selecting_left,
        selecting_right: config.selecting_right,
    };

    let mut target_path_rel = None;
    let mut base_path_rel = None;
    if config.build_target || config.build_base {
        let project_dir = config
            .build_config
            .project_dir
            .as_ref()
            .ok_or_else(|| Error::msg("Missing project dir"))?;
        if let Some(target_path) = &config.target_path {
            target_path_rel = Some(target_path.strip_prefix(project_dir).map_err(|_| {
                anyhow!(
                    "Target path '{}' doesn't begin with '{}'",
                    target_path.display(),
                    project_dir.display()
                )
            })?);
        }
        if let Some(base_path) = &config.base_path {
            base_path_rel = Some(base_path.strip_prefix(project_dir).map_err(|_| {
                anyhow!(
                    "Base path '{}' doesn't begin with '{}'",
                    base_path.display(),
                    project_dir.display()
                )
            })?);
        };
    }

    let mut total = 1;
    if config.build_target && target_path_rel.is_some() {
        total += 1;
    }
    if config.build_base && base_path_rel.is_some() {
        total += 1;
    }
    if config.target_path.is_some() {
        total += 1;
    }
    if config.base_path.is_some() {
        total += 1;
    }

    let mut step_idx = 0;
    let mut first_status = match target_path_rel {
        Some(target_path_rel) if config.build_target => {
            update_status(
                context,
                format!("Building target {}", target_path_rel.display()),
                step_idx,
                total,
                &cancel,
            )?;
            step_idx += 1;
            run_make(&config.build_config, target_path_rel)
        }
        _ => BuildStatus::default(),
    };

    let mut second_status = match base_path_rel {
        Some(base_path_rel) if config.build_base => {
            update_status(
                context,
                format!("Building base {}", base_path_rel.display()),
                step_idx,
                total,
                &cancel,
            )?;
            step_idx += 1;
            run_make(&config.build_config, base_path_rel)
        }
        _ => BuildStatus::default(),
    };

    let time = OffsetDateTime::now_utc();

    let first_obj = match &config.target_path {
        Some(target_path) if first_status.success => {
            update_status(
                context,
                format!("Loading target {}", target_path.display()),
                step_idx,
                total,
                &cancel,
            )?;
            step_idx += 1;
            match read::read(target_path, &config.diff_obj_config) {
                Ok(obj) => Some(obj),
                Err(e) => {
                    first_status = BuildStatus {
                        success: false,
                        stdout: format!("Loading object '{}'", target_path.display()),
                        stderr: format!("{:#}", e),
                        ..Default::default()
                    };
                    None
                }
            }
        }
        Some(_) => {
            step_idx += 1;
            None
        }
        _ => None,
    };

    let second_obj = match &config.base_path {
        Some(base_path) if second_status.success => {
            update_status(
                context,
                format!("Loading base {}", base_path.display()),
                step_idx,
                total,
                &cancel,
            )?;
            step_idx += 1;
            match read::read(base_path, &config.diff_obj_config) {
                Ok(obj) => Some(obj),
                Err(e) => {
                    second_status = BuildStatus {
                        success: false,
                        stdout: format!("Loading object '{}'", base_path.display()),
                        stderr: format!("{:#}", e),
                        ..Default::default()
                    };
                    None
                }
            }
        }
        Some(_) => {
            step_idx += 1;
            None
        }
        _ => None,
    };

    update_status(context, "Performing diff".to_string(), step_idx, total, &cancel)?;
    step_idx += 1;
    let result = diff_objs(&config.diff_obj_config, first_obj.as_ref(), second_obj.as_ref(), None)?;

    update_status(context, "Complete".to_string(), step_idx, total, &cancel)?;
    Ok(Box::new(ObjDiffResult {
        first_status,
        second_status,
        first_obj: first_obj.and_then(|o| result.left.map(|d| (o, d))),
        second_obj: second_obj.and_then(|o| result.right.map(|d| (o, d))),
        time,
    }))
}

pub fn start_build(waker: Waker, config: ObjDiffConfig) -> JobState {
    start_job(waker, "Build", Job::ObjDiff, move |context, cancel| {
        run_build(&context, cancel, config).map(|result| JobResult::ObjDiff(Some(result)))
    })
}
