#![warn(clippy::all, rust_2018_idioms)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // hide console window on Windows in release

use std::{
    path::PathBuf,
    rc::Rc,
    sync::{Arc, Mutex},
};

use anyhow::{Error, Result};
use cfg_if::cfg_if;
use time::UtcOffset;

fn load_icon() -> Result<egui::IconData> {
    use bytes::Buf;
    let decoder = png::Decoder::new(include_bytes!("../assets/icon_64.png").reader());
    let mut reader = decoder.read_info()?;
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf)?;
    if info.bit_depth != png::BitDepth::Eight {
        return Err(Error::msg("Invalid bit depth"));
    }
    if info.color_type != png::ColorType::Rgba {
        return Err(Error::msg("Invalid color type"));
    }
    buf.truncate(info.buffer_size());
    Ok(egui::IconData { rgba: buf, width: info.width, height: info.height })
}

// When compiling natively:
#[cfg(not(target_arch = "wasm32"))]
fn main() {
    // Log to stdout (if you run with `RUST_LOG=debug`).
    tracing_subscriber::fmt::init();

    // Because localtime_r is unsound in multithreaded apps,
    // we must call this before initializing eframe.
    // https://github.com/time-rs/time/issues/293
    let utc_offset = UtcOffset::current_local_offset().unwrap_or(UtcOffset::UTC);

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
    #[cfg(feature = "wgpu")]
    {
        native_options.renderer = eframe::Renderer::Wgpu;
    }
    eframe::run_native(
        "objdiff",
        native_options,
        Box::new(move |cc| Box::new(objdiff::App::new(cc, utc_offset, exec_path_clone))),
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
                        .spawn()
                        .unwrap()
                        .wait();
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
