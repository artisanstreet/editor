use std::path::PathBuf;

fn main() -> std::io::Result<()> {
    artisan_payload::embed("ARTISAN_FORGE_PAYLOAD", "forge.gz")?;
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let files =
        |names: &[&str]| -> Vec<PathBuf> { names.iter().map(|name| root.join(name)).collect() };
    artisan_payload::embed_notices(
        "forge-notices.gz",
        &root,
        &files(&["LICENSE.md", "notices/forge.txt", "notices/fff-MIT.txt"]),
    )?;
    artisan_payload::embed_notices(
        "ae-notices.gz",
        &root,
        &files(&[
            "LICENSE.md",
            "notices/ae.txt",
            "notices/forge.txt",
            "notices/fff-MIT.txt",
        ]),
    )?;
    build_info::emit();
    Ok(())
}
