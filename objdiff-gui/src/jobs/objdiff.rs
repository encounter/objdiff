use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc::Receiver,
};

use anyhow::{anyhow, Error, Result};
use objdiff_core::{
    diff::{diff_objs, DiffObjConfig, MappingConfig, ObjDiff},
    obj::{read, ObjInfo},
};
use time::OffsetDateTime;

use crate::{
    app::{AppConfig, AppState, ObjectConfig},
    jobs::{start_job, update_status, Job, JobContext, JobResult, JobState},
};

pub struct BuildStatus {
    pub success: bool,
    pub cmdline: String,
    pub stdout: String,
    pub stderr: String,
}

impl Default for BuildStatus {
    fn default() -> Self {
        BuildStatus {
            success: true,
            cmdline: String::new(),
            stdout: String::new(),
            stderr: String::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct BuildConfig {
    pub project_dir: Option<PathBuf>,
    pub custom_make: Option<String>,
    pub custom_args: Option<Vec<String>>,
    #[allow(unused)]
    pub selected_wsl_distro: Option<String>,
}

impl BuildConfig {
    pub(crate) fn from_config(config: &AppConfig) -> Self {
        Self {
            project_dir: config.project_dir.clone(),
            custom_make: config.custom_make.clone(),
            custom_args: config.custom_args.clone(),
            selected_wsl_distro: config.selected_wsl_distro.clone(),
        }
    }
}

pub struct ObjDiffConfig {
    pub build_config: BuildConfig,
    pub build_base: bool,
    pub build_target: bool,
    pub selected_obj: Option<ObjectConfig>,
    pub diff_obj_config: DiffObjConfig,
    pub selecting_left: Option<String>,
    pub selecting_right: Option<String>,
}

impl ObjDiffConfig {
    pub(crate) fn from_state(state: &AppState) -> Self {
        Self {
            build_config: BuildConfig::from_config(&state.config),
            build_base: state.config.build_base,
            build_target: state.config.build_target,
            selected_obj: state.config.selected_obj.clone(),
            diff_obj_config: state.config.diff_obj_config.clone(),
            selecting_left: state.selecting_left.clone(),
            selecting_right: state.selecting_right.clone(),
        }
    }
}

pub struct ObjDiffResult {
    pub first_status: BuildStatus,
    pub second_status: BuildStatus,
    pub first_obj: Option<(ObjInfo, ObjDiff)>,
    pub second_obj: Option<(ObjInfo, ObjDiff)>,
    pub time: OffsetDateTime,
}

pub(crate) fn run_make(config: &BuildConfig, arg: &Path) -> BuildStatus {
    let Some(cwd) = &config.project_dir else {
        return BuildStatus {
            success: false,
            stderr: "Missing project dir".to_string(),
            ..Default::default()
        };
    };
    let make = config.custom_make.as_deref().unwrap_or("make");
    let make_args = config.custom_args.as_deref().unwrap_or(&[]);
    #[cfg(not(windows))]
    let mut command = {
        let mut command = Command::new(make);
        command.current_dir(cwd).args(make_args).arg(arg);
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
            // Strip distro root prefix \\wsl.localhost\{distro}
            let wsl_path_prefix = format!("\\\\wsl.localhost\\{}", distro);
            let cwd = match cwd.strip_prefix(wsl_path_prefix) {
                Ok(new_cwd) => format!("/{}", new_cwd.to_slash_lossy().as_ref()),
                Err(_) => cwd.to_string_lossy().to_string(),
            };

            command
                .arg("--cd")
                .arg(cwd)
                .arg("-d")
                .arg(distro)
                .arg("--")
                .arg(make)
                .args(make_args)
                .arg(arg.to_slash_lossy().as_ref());
        } else {
            command.current_dir(cwd).args(make_args).arg(arg.to_slash_lossy().as_ref());
        }
        command.creation_flags(winapi::um::winbase::CREATE_NO_WINDOW);
        command
    };
    let mut cmdline = shell_escape::escape(command.get_program().to_string_lossy()).into_owned();
    for arg in command.get_args() {
        cmdline.push(' ');
        cmdline.push_str(shell_escape::escape(arg.to_string_lossy()).as_ref());
    }
    let output = match command.output() {
        Ok(output) => output,
        Err(e) => {
            return BuildStatus {
                success: false,
                cmdline,
                stdout: Default::default(),
                stderr: e.to_string(),
            };
        }
    };
    // Try from_utf8 first to avoid copying the buffer if it's valid, then fall back to from_utf8_lossy
    let stdout = String::from_utf8(output.stdout)
        .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
    let stderr = String::from_utf8(output.stderr)
        .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());
    BuildStatus { success: output.status.success(), cmdline, stdout, stderr }
}

fn run_build(
    context: &JobContext,
    cancel: Receiver<()>,
    mut config: ObjDiffConfig,
) -> Result<Box<ObjDiffResult>> {
    let obj_config = config.selected_obj.ok_or_else(|| Error::msg("Missing obj path"))?;
    // Use the per-object symbol mappings, we don't set mappings globally
    config.diff_obj_config.symbol_mappings = MappingConfig {
        mappings: obj_config.symbol_mappings,
        selecting_left: config.selecting_left,
        selecting_right: config.selecting_right,
    };

    let project_dir = config
        .build_config
        .project_dir
        .as_ref()
        .ok_or_else(|| Error::msg("Missing project dir"))?;
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

    let mut total = 1;
    if config.build_target && target_path_rel.is_some() {
        total += 1;
    }
    if config.build_base && base_path_rel.is_some() {
        total += 1;
    }
    if target_path_rel.is_some() {
        total += 1;
    }
    if base_path_rel.is_some() {
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

    let first_obj = match &obj_config.target_path {
        Some(target_path) if first_status.success => {
            update_status(
                context,
                format!("Loading target {}", target_path_rel.unwrap().display()),
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

    let second_obj = match &obj_config.base_path {
        Some(base_path) if second_status.success => {
            update_status(
                context,
                format!("Loading base {}", base_path_rel.unwrap().display()),
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

pub fn start_build(ctx: &egui::Context, config: ObjDiffConfig) -> JobState {
    start_job(ctx, "Build", Job::ObjDiff, move |context, cancel| {
        run_build(&context, cancel, config).map(|result| JobResult::ObjDiff(Some(result)))
    })
}
