use std::{
    path::{Path, PathBuf},
    process::Command,
    str::from_utf8,
    sync::mpsc::Receiver,
};

use anyhow::{anyhow, Context, Error, Result};
use time::OffsetDateTime;

use crate::{
    app::{AppConfig, ObjectConfig},
    diff::{diff_objs, DiffAlg, DiffObjConfig},
    jobs::{start_job, update_status, Job, JobResult, JobState, JobStatusRef},
    obj::{elf, ObjInfo},
};

pub struct BuildStatus {
    pub success: bool,
    pub log: String,
}

pub struct ObjDiffConfig {
    pub build_base: bool,
    pub build_target: bool,
    pub custom_make: Option<String>,
    pub project_dir: Option<PathBuf>,
    pub selected_obj: Option<ObjectConfig>,
    pub selected_wsl_distro: Option<String>,
    pub code_alg: DiffAlg,
    pub data_alg: DiffAlg,
}

impl ObjDiffConfig {
    pub(crate) fn from_config(config: &AppConfig) -> Self {
        ObjDiffConfig {
            build_base: config.build_base,
            build_target: config.build_target,
            custom_make: config.custom_make.clone(),
            project_dir: config.project_dir.clone(),
            selected_obj: config.selected_obj.clone(),
            selected_wsl_distro: config.selected_wsl_distro.clone(),
            code_alg: config.code_alg,
            data_alg: config.data_alg,
        }
    }
}

pub struct ObjDiffResult {
    pub first_status: BuildStatus,
    pub second_status: BuildStatus,
    pub first_obj: Option<ObjInfo>,
    pub second_obj: Option<ObjInfo>,
    pub time: OffsetDateTime,
}

fn run_make(cwd: &Path, arg: &Path, config: &ObjDiffConfig) -> BuildStatus {
    match (|| -> Result<BuildStatus> {
        let make = config.custom_make.as_deref().unwrap_or("make");
        #[cfg(not(windows))]
        let mut command = {
            let mut command = Command::new(make);
            command.current_dir(cwd).arg(arg);
            command
        };
        #[cfg(windows)]
        let mut command = {
            use std::os::windows::process::CommandExt;

            use path_slash::PathExt;
            let mut command = if config.selected_wsl_distro.is_some() {
                Command::new("wsl")
            } else {
                Command::new(make)
            };
            if let Some(distro) = &config.selected_wsl_distro {
                command
                    .arg("--cd")
                    .arg(cwd)
                    .arg("-d")
                    .arg(distro)
                    .arg("--")
                    .arg(make)
                    .arg(arg.to_slash_lossy().as_ref());
            } else {
                command.current_dir(cwd).arg(arg.to_slash_lossy().as_ref());
            }
            command.creation_flags(winapi::um::winbase::CREATE_NO_WINDOW);
            command
        };
        let output = command.output().context("Failed to execute build")?;
        let stdout = from_utf8(&output.stdout).context("Failed to process stdout")?;
        let stderr = from_utf8(&output.stderr).context("Failed to process stderr")?;
        Ok(BuildStatus {
            success: output.status.code().unwrap_or(-1) == 0,
            log: format!("{stdout}\n{stderr}"),
        })
    })() {
        Ok(status) => status,
        Err(e) => BuildStatus { success: false, log: e.to_string() },
    }
}

fn run_build(
    status: &JobStatusRef,
    cancel: Receiver<()>,
    config: ObjDiffConfig,
) -> Result<Box<ObjDiffResult>> {
    let obj_config = config.selected_obj.as_ref().ok_or_else(|| Error::msg("Missing obj path"))?;
    let project_dir =
        config.project_dir.as_ref().ok_or_else(|| Error::msg("Missing project dir"))?;
    let target_path_rel = if let Some(target_path) = &obj_config.target_path {
        Some(target_path.strip_prefix(project_dir).map_err(|_| {
            anyhow!(
                "Target path '{}' doesn't begin with '{}'",
                target_path.display(),
                project_dir.display()
            )
        })?)
    } else {
        None
    };
    let base_path_rel = if let Some(base_path) = &obj_config.base_path {
        Some(base_path.strip_prefix(project_dir).map_err(|_| {
            anyhow!(
                "Base path '{}' doesn't begin with '{}'",
                base_path.display(),
                project_dir.display()
            )
        })?)
    } else {
        None
    };

    let mut total = 3;
    if config.build_target && target_path_rel.is_some() {
        total += 1;
    }
    if config.build_base && base_path_rel.is_some() {
        total += 1;
    }
    let first_status = match target_path_rel {
        Some(target_path_rel) if config.build_target => {
            update_status(
                status,
                format!("Building target {}", target_path_rel.display()),
                0,
                total,
                &cancel,
            )?;
            run_make(project_dir, target_path_rel, &config)
        }
        _ => BuildStatus { success: true, log: String::new() },
    };

    let second_status = match base_path_rel {
        Some(base_path_rel) if config.build_base => {
            update_status(
                status,
                format!("Building base {}", base_path_rel.display()),
                0,
                total,
                &cancel,
            )?;
            run_make(project_dir, base_path_rel, &config)
        }
        _ => BuildStatus { success: true, log: String::new() },
    };

    let time = OffsetDateTime::now_utc();

    let mut first_obj =
        match &obj_config.target_path {
            Some(target_path) if first_status.success => {
                update_status(
                    status,
                    format!("Loading target {}", target_path_rel.unwrap().display()),
                    2,
                    total,
                    &cancel,
                )?;
                Some(elf::read(target_path).with_context(|| {
                    format!("Failed to read object '{}'", target_path.display())
                })?)
            }
            _ => None,
        };

    let mut second_obj = match &obj_config.base_path {
        Some(base_path) if second_status.success => {
            update_status(
                status,
                format!("Loading base {}", base_path_rel.unwrap().display()),
                3,
                total,
                &cancel,
            )?;
            Some(
                elf::read(base_path)
                    .with_context(|| format!("Failed to read object '{}'", base_path.display()))?,
            )
        }
        _ => None,
    };

    update_status(status, "Performing diff".to_string(), 4, total, &cancel)?;
    let diff_config = DiffObjConfig { code_alg: config.code_alg, data_alg: config.data_alg };
    diff_objs(&diff_config, first_obj.as_mut(), second_obj.as_mut())?;

    update_status(status, "Complete".to_string(), total, total, &cancel)?;
    Ok(Box::new(ObjDiffResult { first_status, second_status, first_obj, second_obj, time }))
}

pub fn start_build(config: ObjDiffConfig) -> JobState {
    start_job("Object diff", Job::ObjDiff, move |status, cancel| {
        run_build(status, cancel, config).map(|result| JobResult::ObjDiff(Some(result)))
    })
}
