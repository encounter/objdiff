#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // hide console window on Windows in release

mod app;
mod app_config;
mod config;
mod fonts;
mod hotkeys;
mod jobs;
mod update;
mod views;

use std::{
    path::PathBuf,
    process::ExitCode,
    rc::Rc,
    sync::{Arc, Mutex},
};

use anyhow::{ensure, Result};
use cfg_if::cfg_if;
use time::UtcOffset;
use tracing_subscriber::EnvFilter;

use crate::views::graphics::{load_graphics_config, GraphicsBackend, GraphicsConfig};

fn load_icon() -> Result<egui::IconData> {
    use bytes::Buf;
    let decoder = png::Decoder::new(include_bytes!("../assets/icon_64.png").reader());
    let mut reader = decoder.read_info()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf)?;
    ensure!(info.bit_depth == png::BitDepth::Eight);
    ensure!(info.color_type == png::ColorType::Rgba);
    buf.truncate(info.buffer_size());
    Ok(egui::IconData { rgba: buf, width: info.width, height: info.height })
}

const APP_NAME: &str = "objdiff";

// When compiling natively:
#[cfg(not(target_arch = "wasm32"))]
fn main() -> ExitCode {
    // Log to stdout (if you run with `RUST_LOG=debug`).
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                // Default to info level
                .with_default_directive(tracing_subscriber::filter::LevelFilter::INFO.into())
                .from_env_lossy()
                // This module is noisy at info level
                .add_directive("wgpu_core::device::resource=warn".parse().unwrap()),
        )
        .init();

    // Because localtime_r is unsound in multithreaded apps,
    // we must call this before initializing eframe.
    // https://github.com/time-rs/time/issues/293
    let utc_offset = UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC);

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
        use eframe::egui_wgpu::wgpu::Backends;
        if graphics_config.desired_backend.is_supported() {
            native_options.wgpu_options.supported_backends = match graphics_config.desired_backend {
                GraphicsBackend::Auto => native_options.wgpu_options.supported_backends,
                GraphicsBackend::Dx12 => Backends::DX12,
                GraphicsBackend::Metal => Backends::METAL,
                GraphicsBackend::Vulkan => Backends::VULKAN,
                GraphicsBackend::OpenGL => Backends::GL,
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
    ) {
        eframe_error = Some(e);
    }
    #[cfg(feature = "wgpu")]
    if let Some(e) = eframe_error {
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
            native_options.wgpu_options.supported_backends = Default::default();
            if let Err(e) = run_eframe(
                native_options.clone(),
                utc_offset,
                exec_path.clone(),
                app_path.clone(),
                graphics_config.clone(),
                graphics_config_path.clone(),
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
    if let Ok(mut guard) = exec_path.lock() {
        if let Some(path) = guard.take() {
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
            )))
        }),
    )
}

// when compiling to web using trunk.
#[cfg(target_arch = "wasm32")]
fn main() {
    // Make sure panics are logged using `console.error`.
    console_error_panic_hook::set_once();

    // Redirect tracing to console.log and friends:
    tracing_wasm::set_as_global_default();

    let web_options = eframe::WebOptions::default();
    eframe::start_web(
        "the_canvas_id", // hardcode it
        web_options,
        Box::new(|cc| Box::new(eframe_template::TemplateApp::new(cc))),
    )
    .expect("failed to start eframe");
}
