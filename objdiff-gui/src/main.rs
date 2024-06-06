#![warn(clippy::all, rust_2018_idioms)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // hide console window on Windows in release

mod app;
mod app_config;
mod config;
mod fonts;
mod jobs;
mod update;
mod views;

use std::{
    path::PathBuf,
    rc::Rc,
    sync::{Arc, Mutex},
};

use anyhow::{ensure, Result};
use cfg_if::cfg_if;
use time::UtcOffset;

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
fn main() {
    // Log to stdout (if you run with `RUST_LOG=debug`).
    tracing_subscriber::fmt::init();

    // Because localtime_r is unsound in multithreaded apps,
    // we must call this before initializing eframe.
    // https://github.com/time-rs/time/issues/293
    let utc_offset = UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC);

    let app_path = std::env::current_exe().ok();
    let exec_path: Rc<Mutex<Option<PathBuf>>> = Rc::new(Mutex::new(None));
    let exec_path_clone = exec_path.clone();
    let mut native_options =
        eframe::NativeOptions { follow_system_theme: false, ..Default::default() };
    match load_icon() {
        Ok(data) => {
            native_options.viewport.icon = Some(Arc::new(data));
        }
        Err(e) => {
            log::warn!("Failed to load application icon: {}", e);
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
                log::error!("Failed to load native config: {:?}", e);
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
    eframe::run_native(
        APP_NAME,
        native_options,
        Box::new(move |cc| {
            Box::new(app::App::new(
                cc,
                utc_offset,
                exec_path_clone,
                app_path,
                graphics_config,
                graphics_config_path,
            ))
        }),
    )
    .expect("Failed to run eframe application");

    // Attempt to relaunch application from the updated path
    if let Ok(mut guard) = exec_path.lock() {
        if let Some(path) = guard.take() {
            cfg_if! {
                if #[cfg(unix)] {
                    let result = exec::Command::new(path)
                        .args(&std::env::args().collect::<Vec<String>>())
                        .exec();
                    log::error!("Failed to relaunch: {result:?}");
                } else {
                    let result = std::process::Command::new(path)
                        .args(std::env::args())
                        .spawn();
                    if let Err(e) = result {
                        log::error!("Failed to relaunch: {:?}", e);
                    }
                }
            }
        }
    };
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
