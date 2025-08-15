#![allow(clippy::too_many_arguments)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // hide console window on Windows in release

mod app;
mod app_config;
mod argp_version;
mod config;
mod fonts;
mod hotkeys;
mod jobs;
mod update;
mod views;

use std::{
    ffi::OsStr,
    fmt::Display,
    path::PathBuf,
    process::ExitCode,
    rc::Rc,
    str::FromStr,
    sync::{Arc, Mutex},
};

use anyhow::{Result, ensure};
use argp::{FromArgValue, FromArgs};
use cfg_if::cfg_if;
use objdiff_core::config::path::check_path_buf;
use time::UtcOffset;
use tracing_subscriber::{EnvFilter, filter::LevelFilter};
use typed_path::Utf8PlatformPathBuf;

use crate::views::graphics::{GraphicsBackend, GraphicsConfig, load_graphics_config};

#[derive(Debug, Eq, PartialEq, Copy, Clone)]
enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl FromStr for LogLevel {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "error" => Self::Error,
            "warn" => Self::Warn,
            "info" => Self::Info,
            "debug" => Self::Debug,
            "trace" => Self::Trace,
            _ => return Err(()),
        })
    }
}

impl Display for LogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            LogLevel::Error => "error",
            LogLevel::Warn => "warn",
            LogLevel::Info => "info",
            LogLevel::Debug => "debug",
            LogLevel::Trace => "trace",
        })
    }
}

impl FromArgValue for LogLevel {
    fn from_arg_value(value: &OsStr) -> Result<Self, String> {
        String::from_arg_value(value)
            .and_then(|s| Self::from_str(&s).map_err(|_| "Invalid log level".to_string()))
    }
}

#[derive(FromArgs, PartialEq, Debug)]
/// A local diffing tool for decompilation projects.
struct TopLevel {
    #[argp(option, short = 'L')]
    /// Minimum logging level. (Default: info)
    /// Possible values: error, warn, info, debug, trace
    log_level: Option<LogLevel>,
    #[argp(option, short = 'p')]
    /// Path to the project directory.
    project_dir: Option<PathBuf>,
    /// Print version information and exit.
    #[argp(switch, short = 'V')]
    version: bool,
}

fn load_icon() -> Result<egui::IconData> {
    let decoder = png::Decoder::new(include_bytes!("../assets/icon_64.png").as_ref());
    let mut reader = decoder.read_info()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf)?;
    ensure!(info.bit_depth == png::BitDepth::Eight);
    ensure!(info.color_type == png::ColorType::Rgba);
    buf.truncate(info.buffer_size());
    Ok(egui::IconData { rgba: buf, width: info.width, height: info.height })
}

const APP_NAME: &str = "objdiff";

fn main() -> ExitCode {
    let args: TopLevel = argp_version::from_env();
    let builder = tracing_subscriber::fmt();
    if let Some(level) = args.log_level {
        builder
            .with_max_level(match level {
                LogLevel::Error => LevelFilter::ERROR,
                LogLevel::Warn => LevelFilter::WARN,
                LogLevel::Info => LevelFilter::INFO,
                LogLevel::Debug => LevelFilter::DEBUG,
                LogLevel::Trace => LevelFilter::TRACE,
            })
            .init();
    } else {
        builder
            .with_env_filter(
                EnvFilter::builder()
                    // Default to info level
                    .with_default_directive(LevelFilter::INFO.into())
                    .from_env_lossy()
                    // This module is noisy at info level
                    .add_directive("wgpu_core::device::resource=warn".parse().unwrap()),
            )
            .init();
    }

    // Because localtime_r is unsound in multithreaded apps,
    // we must call this before initializing eframe.
    // https://github.com/time-rs/time/issues/293
    let utc_offset = UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC);

    // Resolve project directory if provided
    let project_dir = if let Some(path) = args.project_dir {
        match path.canonicalize() {
            Ok(path) => {
                // Ensure the path is a directory
                if path.is_dir() {
                    match check_path_buf(path) {
                        Ok(path) => Some(path),
                        Err(e) => {
                            log::error!("Failed to convert project directory to UTF-8 path: {}", e);
                            None
                        }
                    }
                } else {
                    log::error!("Project directory is not a directory: {}", path.display());
                    None
                }
            }
            Err(e) => {
                log::error!("Failed to canonicalize project directory: {}", e);
                None
            }
        }
    } else {
        None
    };

    let app_path = std::env::current_exe().ok();
    let exec_path: Rc<Mutex<Option<PathBuf>>> = Rc::new(Mutex::new(None));
    let mut native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_app_id(APP_NAME),
        ..Default::default()
    };
    match load_icon() {
        Ok(data) => {
            native_options.viewport.icon = Some(Arc::new(data));
        }
        Err(e) => {
            log::warn!("Failed to load application icon: {e:?}");
        }
    }
    let mut graphics_config = GraphicsConfig::default();
    let mut graphics_config_path = None;
    if let Some(storage_dir) = eframe::storage_dir(APP_NAME) {
        let config_path = storage_dir.join("graphics.ron");
        match load_graphics_config(&config_path) {
            Ok(Some(config)) => {
                graphics_config = config;
            }
            Ok(None) => {}
            Err(e) => {
                log::error!("Failed to load native config: {e:?}");
            }
        }
        graphics_config_path = Some(config_path);
    }
    #[cfg(feature = "wgpu")]
    {
        use eframe::egui_wgpu::{WgpuSetup, wgpu};
        if graphics_config.desired_backend.is_supported() {
            native_options.wgpu_options.wgpu_setup = match native_options.wgpu_options.wgpu_setup {
                WgpuSetup::CreateNew(mut setup) => {
                    setup.instance_descriptor.backends = match graphics_config.desired_backend {
                        GraphicsBackend::Auto => setup.instance_descriptor.backends,
                        GraphicsBackend::Dx12 => wgpu::Backends::DX12,
                        GraphicsBackend::Metal => wgpu::Backends::METAL,
                        GraphicsBackend::Vulkan => wgpu::Backends::VULKAN,
                        GraphicsBackend::OpenGLES => wgpu::Backends::GL,
                        GraphicsBackend::OpenGL => wgpu::Backends::empty(),
                    };
                    WgpuSetup::CreateNew(setup)
                }
                // WgpuConfiguration::Default is CreateNew until we call run_eframe()
                _ => unreachable!(),
            };
        }
    }
    let mut eframe_error = None;
    if let Err(e) = run_eframe(
        native_options.clone(),
        utc_offset,
        exec_path.clone(),
        app_path.clone(),
        graphics_config.clone(),
        graphics_config_path.clone(),
        project_dir.clone(),
    ) {
        eframe_error = Some(e);
    }
    #[cfg(feature = "wgpu")]
    if let Some(e) = eframe_error {
        use eframe::egui_wgpu::WgpuConfiguration;

        // Attempt to relaunch using wgpu auto backend if the desired backend failed
        #[allow(unused_mut)]
        let mut should_relaunch = graphics_config.desired_backend != GraphicsBackend::Auto;
        #[cfg(feature = "glow")]
        {
            // If the desired backend is OpenGL, we should try to relaunch using the glow renderer
            should_relaunch &= graphics_config.desired_backend != GraphicsBackend::OpenGL;
        }
        if should_relaunch {
            log::warn!("Failed to launch application: {e:?}");
            log::warn!("Attempting to relaunch using auto-detected backend");
            native_options.wgpu_options.wgpu_setup = WgpuConfiguration::default().wgpu_setup;
            if let Err(e) = run_eframe(
                native_options.clone(),
                utc_offset,
                exec_path.clone(),
                app_path.clone(),
                graphics_config.clone(),
                graphics_config_path.clone(),
                project_dir.clone(),
            ) {
                eframe_error = Some(e);
            } else {
                eframe_error = None;
            }
        } else {
            eframe_error = Some(e);
        }
    }
    #[cfg(all(feature = "wgpu", feature = "glow"))]
    if let Some(e) = eframe_error {
        // Attempt to relaunch using the glow renderer if the wgpu backend failed
        log::warn!("Failed to launch application: {e:?}");
        log::warn!("Attempting to relaunch using fallback OpenGL backend");
        native_options.renderer = eframe::Renderer::Glow;
        if let Err(e) = run_eframe(
            native_options,
            utc_offset,
            exec_path.clone(),
            app_path,
            graphics_config,
            graphics_config_path,
            project_dir,
        ) {
            eframe_error = Some(e);
        } else {
            eframe_error = None;
        }
    }
    if let Some(e) = eframe_error {
        log::error!("Failed to launch application: {e:?}");
        return ExitCode::FAILURE;
    }

    // Attempt to relaunch application from the updated path
    if let Ok(mut guard) = exec_path.lock()
        && let Some(path) = guard.take()
    {
        cfg_if! {
            if #[cfg(unix)] {
                let e = exec::Command::new(path)
                    .args(&std::env::args().collect::<Vec<String>>())
                    .exec();
                log::error!("Failed to relaunch: {e:?}");
                return ExitCode::FAILURE;
            } else {
                let result = std::process::Command::new(path)
                    .args(std::env::args())
                    .spawn();
                if let Err(e) = result {
                    log::error!("Failed to relaunch: {e:?}");
                    return ExitCode::FAILURE;
                }
            }
        }
    };
    ExitCode::SUCCESS
}

fn run_eframe(
    native_options: eframe::NativeOptions,
    utc_offset: UtcOffset,
    exec_path_clone: Rc<Mutex<Option<PathBuf>>>,
    app_path: Option<PathBuf>,
    graphics_config: GraphicsConfig,
    graphics_config_path: Option<PathBuf>,
    project_dir: Option<Utf8PlatformPathBuf>,
) -> Result<(), eframe::Error> {
    eframe::run_native(
        APP_NAME,
        native_options,
        Box::new(move |cc| {
            Ok(Box::new(app::App::new(
                cc,
                utc_offset,
                exec_path_clone,
                app_path,
                graphics_config,
                graphics_config_path,
                project_dir,
            )))
        }),
    )
}
