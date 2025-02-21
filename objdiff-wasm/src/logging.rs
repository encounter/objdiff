wit_bindgen::generate!({
    world: "imports",
    path: "wit/deps/logging",
});

use alloc::format;

pub use wasi::logging::logging as wasi_logging;

struct WasiLogger;

impl log::Log for WasiLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool { metadata.level() <= log::max_level() }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let level = match record.level() {
            log::Level::Error => wasi_logging::Level::Error,
            log::Level::Warn => wasi_logging::Level::Warn,
            log::Level::Info => wasi_logging::Level::Info,
            log::Level::Debug => wasi_logging::Level::Debug,
            log::Level::Trace => wasi_logging::Level::Trace,
        };
        wasi_logging::log(level, record.target(), &format!("{}", record.args()));
    }

    fn flush(&self) {}
}

static LOGGER: WasiLogger = WasiLogger;

pub fn init(level: wasi_logging::Level) {
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(match level {
        wasi_logging::Level::Error => log::LevelFilter::Error,
        wasi_logging::Level::Warn => log::LevelFilter::Warn,
        wasi_logging::Level::Info => log::LevelFilter::Info,
        wasi_logging::Level::Debug => log::LevelFilter::Debug,
        wasi_logging::Level::Trace => log::LevelFilter::Trace,
        wasi_logging::Level::Critical => log::LevelFilter::Off,
    });
}

#[cfg(not(feature = "std"))]
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    use alloc::string::ToString;
    wasi_logging::log(wasi_logging::Level::Critical, "objdiff_core::panic", &info.to_string());
    core::arch::wasm32::unreachable();
}
