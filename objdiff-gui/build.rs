use anyhow::Result;
use vergen::EmitBuilder;

fn main() -> Result<()> {
    #[cfg(windows)]
    {
        winres::WindowsResource::new().set_icon("assets/icon.ico").compile()?;
    }
    EmitBuilder::builder().fail_on_error().all_build().all_cargo().all_git().emit()
}
