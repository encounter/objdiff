pub mod watcher;

use std::process::Command;

use typed_path::Utf8PlatformPathBuf;

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
    pub project_dir: Option<Utf8PlatformPathBuf>,
    pub custom_make: Option<String>,
    pub custom_args: Option<Vec<String>>,
    #[allow(unused)]
    pub selected_wsl_distro: Option<String>,
}

pub fn run_make(config: &BuildConfig, arg: &str) -> BuildStatus {
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
