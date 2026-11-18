use super::*;

fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn credentials_written_under_a_permissive_umask_stay_private() {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().join("engines").join("claude");
    let file = folder.join("mcp.json");
    write_file(&file, b"{\"token\":\"t\"}").unwrap();
    assert_eq!(mode(&folder), 0o700);
    assert_eq!(mode(&file), 0o600);
    assert_eq!(std::fs::read(&file).unwrap(), b"{\"token\":\"t\"}");
}

#[test]
fn rewriting_a_world_readable_file_makes_it_private() {
    use std::os::unix::fs::PermissionsExt as _;
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("config.toml");
    std::fs::write(&file, b"old").unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
    write_file(&file, b"new").unwrap();
    assert_eq!(mode(&file), 0o600);
    assert_eq!(std::fs::read(&file).unwrap(), b"new");
    let leftovers: Vec<_> = std::fs::read_dir(root.path()).unwrap().collect();
    assert_eq!(leftovers.len(), 1, "no temporary file is left behind");
}
