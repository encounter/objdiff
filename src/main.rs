#![warn(clippy::all, rust_2018_idioms)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // hide console window on Windows in release

use time::UtcOffset;

// When compiling natively:
#[cfg(not(target_arch = "wasm32"))]
fn main() {
    // Log to stdout (if you run with `RUST_LOG=debug`).
    tracing_subscriber::fmt::init();

    // Because localtime_r is unsound in multithreaded apps,
    // we must call this before initializing eframe.
    // https://github.com/time-rs/time/issues/293
    let utc_offset = UtcOffset::current_local_offset().unwrap();

    let native_options = eframe::NativeOptions::default();
    // native_options.renderer = eframe::Renderer::Wgpu;
    eframe::run_native(
        "objdiff",
        native_options,
        Box::new(move |cc| Box::new(objdiff::App::new(cc, utc_offset))),
    );
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
