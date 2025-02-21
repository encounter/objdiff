fn main() -> Result<(), Box<dyn std::error::Error>> {
    wit_deps::lock_sync!()?;
    Ok(())
}
