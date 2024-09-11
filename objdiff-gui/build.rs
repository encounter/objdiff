use anyhow::Result;

fn main() -> Result<()> {
    #[cfg(windows)]
    {
        let mut res = tauri_winres::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set_language(0x0409); // US English
        res.compile()?;
    }
    Ok(())
}
