use std::{path::Path, process::Command, str::from_utf8, sync::mpsc::Receiver};

use anyhow::{anyhow, Context, Error, Result};
use time::OffsetDateTime;

use crate::{
    app::{AppConfig, AppConfigRef},
    diff::diff_objs,
    jobs::{start_job, update_status, Job, JobResult, JobState, JobStatusRef},
    obj::{elf, ObjInfo},
};

pub struct BuildStatus {
    pub success: bool,
    pub log: String,
}

pub struct ObjDiffResult {
    pub first_status: BuildStatus,
    pub second_status: BuildStatus,
    pub first_obj: Option<ObjInfo>,
    pub second_obj: Option<ObjInfo>,
    pub time: OffsetDateTime,
}

fn run_make(cwd: &Path, arg: &Path, config: &AppConfig) -> BuildStatus {
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
    config: AppConfigRef,
) -> Result<Box<ObjDiffResult>> {
    let config = config.read().map_err(|_| Error::msg("Failed to lock app config"))?.clone();
    let obj_config = config.selected_obj.as_ref().ok_or_else(|| Error::msg("Missing obj path"))?;
    let project_dir =
        config.project_dir.as_ref().ok_or_else(|| Error::msg("Missing project dir"))?;
    let target_path_rel = obj_config.target_path.strip_prefix(project_dir).map_err(|_| {
        anyhow!(
            "Target path '{}' doesn't begin with '{}'",
            obj_config.target_path.display(),
            project_dir.display()
        )
    })?;
    let base_path_rel = obj_config.base_path.strip_prefix(project_dir).map_err(|_| {
        anyhow!(
            "Base path '{}' doesn't begin with '{}'",
            obj_config.base_path.display(),
            project_dir.display()
        )
    })?;

    let total = if config.build_target { 5 } else { 4 };
    let first_status = if config.build_target {
        update_status(status, format!("Building target {}", target_path_rel.display()), 0, total, &cancel)?;
        run_make(project_dir, target_path_rel, &config)
    } else {
        BuildStatus { success: true, log: String::new() }
    };

    update_status(status, format!("Building base {}", base_path_rel.display()), 1, total, &cancel)?;
    let second_status = run_make(project_dir, base_path_rel, &config);

    let time = OffsetDateTime::now_utc();

    let mut first_obj = if first_status.success {
        update_status(status, format!("Loading target {}", target_path_rel.display()), 2, total, &cancel)?;
        Some(elf::read(&obj_config.target_path).with_context(|| {
            format!("Failed to read object '{}'", obj_config.target_path.display())
        })?)
    } else {
        None
    };

    let mut second_obj = if second_status.success {
        update_status(status, format!("Loading base {}", base_path_rel.display()), 3, total, &cancel)?;
        Some(elf::read(&obj_config.base_path).with_context(|| {
            format!("Failed to read object '{}'", obj_config.base_path.display())
        })?)
    } else {
        None
    };

    if let (Some(first_obj), Some(second_obj)) = (&mut first_obj, &mut second_obj) {
        update_status(status, "Performing diff".to_string(), 4, total, &cancel)?;
        diff_objs(first_obj, second_obj)?;
    }

    update_status(status, "Complete".to_string(), total, total, &cancel)?;
    Ok(Box::new(ObjDiffResult { first_status, second_status, first_obj, second_obj, time }))
}

pub fn start_build(config: AppConfigRef) -> JobState {
    start_job("Object diff", Job::ObjDiff, move |status, cancel| {
        run_build(status, cancel, config).map(|result| JobResult::ObjDiff(Some(result)))
    })
}
