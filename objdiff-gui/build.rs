use anyhow::Result;
use vergen_gitcl::{BuildBuilder, CargoBuilder, Emitter, GitclBuilder};

fn main() -> Result<()> {
    #[cfg(windows)]
    {
        winres::WindowsResource::new().set_icon("assets/icon.ico").compile()?;
    }
    Emitter::default()
        .add_instructions(&BuildBuilder::all_build()?)?
        .add_instructions(&CargoBuilder::all_cargo()?)?
        .add_instructions(&GitclBuilder::all_git()?)?
        .emit()
}
