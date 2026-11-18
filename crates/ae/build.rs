fn main() -> std::io::Result<()> {
    artisan_payload::embed("ARTISAN_FORGE_PAYLOAD", "forge.gz")?;
    build_info::emit();
    Ok(())
}
