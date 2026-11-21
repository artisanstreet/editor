use super::*;

#[test]
fn a_folder_where_a_file_belongs_is_refused_and_kept() {
    let home = tempfile::tempdir().expect("a home");
    let folder = home.path().join("settings.json");
    fs::create_dir_all(folder.join("inside")).expect("a folder in the file's place");
    fs::write(folder.join("inside/keep"), "the user's").expect("a file in it");

    let written = write(home.path(), Path::new("settings.json"), b"{}", 0o600);

    assert!(
        matches!(&written, Err(WriteError::Folder { path }) if *path == folder),
        "{written:?}"
    );
    assert_eq!(
        fs::read_to_string(folder.join("inside/keep")).expect("the folder survived"),
        "the user's"
    );
}

#[cfg(unix)]
#[test]
fn a_place_that_cannot_be_inspected_says_so() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().expect("a home");
    let locked = home.path().join("locked");
    fs::create_dir(&locked).expect("a folder");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).expect("lock it");
    let modes_are_enforced = fs::read_dir(&locked).is_err();

    let written = write(home.path(), Path::new("locked/config"), b"x", 0o600);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).expect("unlock it");

    assert!(
        !modes_are_enforced
            || matches!(&written, Err(WriteError::Inspect { source, .. }) if source.kind() == io::ErrorKind::PermissionDenied),
        "{written:?}"
    );
}

#[test]
fn a_missing_file_is_written() {
    let home = tempfile::tempdir().expect("a home");

    write(home.path(), Path::new("nested/config"), b"x", 0o600).expect("written");

    assert_eq!(
        fs::read(home.path().join("nested/config")).expect("the file"),
        b"x"
    );
}

#[test]
fn removing_a_config_where_a_folder_sits_refuses_and_keeps_it() {
    let home = tempfile::tempdir().expect("a home");
    let folder = home.path().join("mcp.json");
    fs::create_dir_all(folder.join("inside")).expect("a folder in the file's place");
    fs::write(folder.join("inside/keep"), "the user's").expect("a file in it");

    let removed = remove_config(EngineId::Claude, home.path(), Path::new("mcp.json"));

    assert!(
        matches!(&removed, Err(ConfigError::Remove { source: RemoveError::Folder { path }, .. }) if *path == folder),
        "{removed:?}"
    );
    assert_eq!(
        fs::read_to_string(folder.join("inside/keep")).expect("the folder survived"),
        "the user's"
    );
}
