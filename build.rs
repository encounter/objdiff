use anyhow::Result;
use vergen::{vergen, Config};

fn main() -> Result<()> {
    #[cfg(windows)]
    {
        winres::WindowsResource::new().set_icon("assets/icon.ico").compile()?;
    }
    vergen(Config::default())
}
