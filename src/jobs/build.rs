use std::{
    path::Path,
    process::Command,
    str::from_utf8,
    sync::{Arc, mpsc::Receiver, RwLock},
};

use anyhow::{Context, Error, Result};

use crate::{
    app::AppConfig,
    diff::diff_objs,
    elf,
    jobs::{Job, JobResult, JobState, queue_job, Status, update_status},
    obj::ObjInfo,
};

pub struct BuildStatus {
    pub success: bool,
    pub log: String,
}
pub struct BuildResult {
    pub first_status: BuildStatus,
    pub second_status: BuildStatus,
    pub first_obj: Option<ObjInfo>,
    pub second_obj: Option<ObjInfo>,
}

fn run_make(cwd: &Path, arg: &Path, config: &AppConfig) -> BuildStatus {
    match (|| -> Result<BuildStatus> {
        let make = if config.custom_make.is_empty() { "make" } else { &config.custom_make };
        #[cfg(not(windows))]
        let mut command = Command::new(make).current_dir(cwd).arg(arg);
        #[cfg(windows)]
        let mut command = {
            use path_slash::PathExt;
            use std::os::windows::process::CommandExt;
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
                command.current_dir(cwd).arg(arg);
            }
            command.creation_flags(winapi::um::winbase::CREATE_NO_WINDOW);
            command
        };
        let output = command.output().context("Failed to execute build")?;
        let stdout = from_utf8(&output.stdout).context("Failed to process stdout")?;
        let stderr = from_utf8(&output.stderr).context("Failed to process stderr")?;
        Ok(BuildStatus {
            success: output.status.code().unwrap_or(-1) == 0,
            log: format!("{}\n{}", stdout, stderr),
        })
    })() {
        Ok(status) => status,
        Err(e) => BuildStatus { success: false, log: e.to_string() },
    }
}

fn run_build(
    status: &Status,
    cancel: Receiver<()>,
    obj_path: String,
    config: Arc<RwLock<AppConfig>>,
) -> Result<Box<BuildResult>> {
    let config = config.read().map_err(|_| Error::msg("Failed to lock app config"))?.clone();
    let project_dir =
        config.project_dir.as_ref().ok_or_else(|| Error::msg("Missing project dir"))?;
    let mut asm_path = config
        .build_asm_dir
        .as_ref()
        .ok_or_else(|| Error::msg("Missing build asm dir"))?
        .to_owned();
    asm_path.push(&obj_path);
    let mut src_path = config
        .build_src_dir
        .as_ref()
        .ok_or_else(|| Error::msg("Missing build src dir"))?
        .to_owned();
    src_path.push(&obj_path);
    let asm_path_rel =
        asm_path.strip_prefix(project_dir).context("Failed to create relative asm obj path")?;
    let src_path_rel =
        src_path.strip_prefix(project_dir).context("Failed to create relative src obj path")?;

    update_status(status, format!("Building asm {}", obj_path), 0, 5, &cancel)?;
    let first_status = run_make(project_dir, asm_path_rel, &config);

    update_status(status, format!("Building src {}", obj_path), 1, 5, &cancel)?;
    let second_status = run_make(project_dir, src_path_rel, &config);

    let mut first_obj = if first_status.success {
        update_status(status, format!("Loading asm {}", obj_path), 2, 5, &cancel)?;
        Some(elf::read(&asm_path)?)
    } else {
        None
    };

    let mut second_obj = if second_status.success {
        update_status(status, format!("Loading src {}", obj_path), 3, 5, &cancel)?;
        Some(elf::read(&src_path)?)
    } else {
        None
    };

    if let (Some(first_obj), Some(second_obj)) = (&mut first_obj, &mut second_obj) {
        update_status(status, "Performing diff".to_string(), 4, 5, &cancel)?;
        diff_objs(first_obj, second_obj)?;
    }

    update_status(status, "Complete".to_string(), 5, 5, &cancel)?;
    Ok(Box::new(BuildResult { first_status, second_status, first_obj, second_obj }))
}

pub fn queue_build(obj_path: String, config: Arc<RwLock<AppConfig>>) -> JobState {
    queue_job(Job::Build, move |status, cancel| {
        run_build(status, cancel, obj_path, config).map(JobResult::Build)
    })
}
