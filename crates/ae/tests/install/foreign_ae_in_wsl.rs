use super::FOREIGN_AE_IN_WSL;
use std::os::unix::fs::symlink;
use std::path::Path;

const FOREIGN: i32 = 0;
const ARTISANS: i32 = 1;

fn check(home: &Path) -> i32 {
    std::process::Command::new("sh")
        .args(["-c", FOREIGN_AE_IN_WSL])
        .env("HOME", home)
        .env_remove("XDG_DATA_HOME")
        .status()
        .expect("run the check")
        .code()
        .expect("the check exits with a code")
}

fn link_to(home: &Path, target: &Path) {
    let bin = home.join(".local/bin");
    std::fs::create_dir_all(&bin).expect("create ~/.local/bin");
    symlink(target, bin.join("ae")).expect("link ae");
}

#[test]
fn only_a_link_to_artisans_ae_counts_as_artisans_in_wsl() {
    let empty = tempfile::tempdir().expect("a home");
    assert_eq!(check(empty.path()), ARTISANS, "nothing is in the way");

    let ours = tempfile::tempdir().expect("a home");
    link_to(
        ours.path(),
        &ours.path().join(".local/share/artisan-street/ae/ae"),
    );
    assert_eq!(check(ours.path()), ARTISANS, "Artisan's own link");

    let moved = tempfile::tempdir().expect("a home");
    let default = moved.path().join(".local/share/artisan-street");
    std::fs::create_dir_all(&default).expect("create the default root");
    let chosen = moved.path().join("elsewhere/Artisan Street");
    std::fs::write(default.join("location"), format!("{}\n", chosen.display()))
        .expect("point at the chosen root");
    link_to(moved.path(), &chosen.join("ae/ae"));
    assert_eq!(check(moved.path()), ARTISANS, "a link into a chosen root");

    let stowed = tempfile::tempdir().expect("a home");
    let dotfiles = stowed.path().join("dotfiles/bin/ae");
    std::fs::create_dir_all(dotfiles.parent().expect("a folder")).expect("create dotfiles");
    std::fs::write(&dotfiles, "#!/bin/sh\n").expect("write their ae");
    link_to(stowed.path(), &dotfiles);
    assert_eq!(check(stowed.path()), FOREIGN, "a dotfile manager's link");

    let dangling = tempfile::tempdir().expect("a home");
    link_to(dangling.path(), &dangling.path().join("gone/ae"));
    assert_eq!(check(dangling.path()), FOREIGN, "a dangling link elsewhere");

    let file = tempfile::tempdir().expect("a home");
    std::fs::create_dir_all(file.path().join(".local/bin")).expect("create ~/.local/bin");
    std::fs::write(file.path().join(".local/bin/ae"), "#!/bin/sh\n").expect("write their ae");
    assert_eq!(check(file.path()), FOREIGN, "a plain file");
}
