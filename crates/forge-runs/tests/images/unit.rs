#![cfg(unix)]

use super::*;
use std::os::unix::fs::PermissionsExt as _;

fn mode(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[test]
fn a_live_folder_that_cannot_be_checked_is_logged_not_skipped_silently() {
    let data = tempfile::tempdir().unwrap();
    let root = data.path().join("run-images");
    let folder = root.join("live");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join(".image.png.k3Yx9a.partial"), b"half").unwrap();
    mode(&root, 0o600);
    let (logs, _capturing) = test_support::logs::capture();

    let found = partials(&folder);
    mode(&root, 0o700);

    assert!(found.is_empty());
    let logs = logs.text();
    assert!(
        logs.contains("check a live thread's image folder")
            && logs.contains(&*folder.to_string_lossy()),
        "the unchecked folder went unlogged: {logs}"
    );
}

#[test]
fn an_unreadable_copy_is_logged_and_copied_again() {
    let data = tempfile::tempdir().unwrap();
    let thread = ThreadId::new();
    let stored = data.path().join("blob.png");
    std::fs::write(&stored, b"pristine").unwrap();
    let dir = thread_folder(data.path(), &thread);
    std::fs::create_dir_all(&dir).unwrap();
    let copy = dir.join("blob.png");
    std::fs::write(&copy, b"pristine").unwrap();
    mode(&copy, 0o000);
    let (logs, _capturing) = test_support::logs::capture();

    let copied = copy_one(&thread, data.path(), &stored).unwrap();

    assert_eq!(copied, copy);
    assert_eq!(std::fs::read(&copy).unwrap(), b"pristine");
    let logs = logs.text();
    assert!(
        logs.contains("read an image copy; copying it again")
            && logs.contains(&*copy.to_string_lossy()),
        "the unreadable copy went unlogged: {logs}"
    );
}
