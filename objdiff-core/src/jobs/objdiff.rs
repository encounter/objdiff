use std::{sync::mpsc::Receiver, task::Waker};

use anyhow::{Error, Result, bail};
use time::OffsetDateTime;
use typed_path::Utf8PlatformPathBuf;

use crate::{
    build::{BuildConfig, BuildStatus, run_make},
    diff::{DiffObjConfig, MappingConfig, ObjectDiff, diff_objs},
    jobs::{Job, JobContext, JobResult, JobState, start_job, update_status},
    obj::{Object, read},
};

pub struct ObjDiffConfig {
    pub build_config: BuildConfig,
    pub build_base: bool,
    pub build_target: bool,
    pub target_path: Option<Utf8PlatformPathBuf>,
    pub base_path: Option<Utf8PlatformPathBuf>,
    pub diff_obj_config: DiffObjConfig,
    pub mapping_config: MappingConfig,
}

pub struct ObjDiffResult {
    pub first_status: BuildStatus,
    pub second_status: BuildStatus,
    pub first_obj: Option<(Object, ObjectDiff)>,
    pub second_obj: Option<(Object, ObjectDiff)>,
    pub time: OffsetDateTime,
}

fn run_build(
    context: &JobContext,
    cancel: Receiver<()>,
    config: ObjDiffConfig,
) -> Result<Box<ObjDiffResult>> {
    let mut target_path_rel = None;
    let mut base_path_rel = None;
    if config.build_target || config.build_base {
        let project_dir = config
            .build_config
            .project_dir
            .as_ref()
            .ok_or_else(|| Error::msg("Missing project dir"))?;
        if let Some(target_path) = &config.target_path {
            target_path_rel = match target_path.strip_prefix(project_dir) {
                Ok(p) => Some(p.with_unix_encoding()),
                Err(_) => {
                    bail!("Target path '{}' doesn't begin with '{}'", target_path, project_dir);
                }
            };
        }
        if let Some(base_path) = &config.base_path {
            base_path_rel = match base_path.strip_prefix(project_dir) {
                Ok(p) => Some(p.with_unix_encoding()),
                Err(_) => {
                    bail!("Base path '{}' doesn't begin with '{}'", base_path, project_dir);
                }
            };
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
                format!("Building target {}", target_path_rel),
                step_idx,
                total,
                &cancel,
            )?;
            step_idx += 1;
            run_make(&config.build_config, target_path_rel.as_ref())
        }
        _ => BuildStatus::default(),
    };

    let mut second_status = match base_path_rel {
        Some(base_path_rel) if config.build_base => {
            update_status(
                context,
                format!("Building base {}", base_path_rel),
                step_idx,
                total,
                &cancel,
            )?;
            step_idx += 1;
            run_make(&config.build_config, base_path_rel.as_ref())
        }
        _ => BuildStatus::default(),
    };

    let time = OffsetDateTime::now_utc();

    let first_obj = match &config.target_path {
        Some(target_path) if first_status.success => {
            update_status(
                context,
                format!("Loading target {}", target_path),
                step_idx,
                total,
                &cancel,
            )?;
            step_idx += 1;
            match read::read(target_path.as_ref(), &config.diff_obj_config) {
                Ok(obj) => Some(obj),
                Err(e) => {
                    first_status = BuildStatus {
                        success: false,
                        stdout: format!("Loading object '{}'", target_path),
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
                format!("Loading base {}", base_path),
                step_idx,
                total,
                &cancel,
            )?;
            step_idx += 1;
            match read::read(base_path.as_ref(), &config.diff_obj_config) {
                Ok(obj) => Some(obj),
                Err(e) => {
                    second_status = BuildStatus {
                        success: false,
                        stdout: format!("Loading object '{}'", base_path),
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
    let result = diff_objs(
        first_obj.as_ref(),
        second_obj.as_ref(),
        None,
        &config.diff_obj_config,
        &config.mapping_config,
    )?;

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
