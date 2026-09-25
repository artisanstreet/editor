//! Build identity of local builds: derived from the checkout, never guessed.

use std::path::Path;

use native_dev::{GitState, dev_version, payload_version, profile_for_bin_dir};

fn git(commit: Option<&str>, dirty: bool, count: Option<u64>) -> GitState {
    GitState {
        commit: commit.map(str::to_owned),
        dirty,
        commit_count: count,
    }
}

#[test]
fn dev_versions_are_semver_prereleases_naming_the_commit() {
    assert_eq!(
        dev_version("0.4.0", &git(Some("1a2b3c4d5e6f7a8b"), false, Some(1284))),
        "0.4.0-dev.1284+g1a2b3c4d5e"
    );
    assert_eq!(
        dev_version("0.4.0", &git(Some("1a2b3c4d5e6f7a8b"), true, Some(7))),
        "0.4.0-dev.7+g1a2b3c4d5e.dirty"
    );
    assert_eq!(dev_version("0.4.0", &GitState::default()), "0.4.0-dev.0");
}

#[test]
fn payload_versions_append_the_binaries_digest() {
    let digest = "0123456789abcdef0123";
    let clean = payload_version(&git(Some("feedfacecafe"), false, Some(3)), digest);
    assert!(clean.ends_with("+gfeedfaceca.b0123456789"), "{clean}");
    let unknown = payload_version(&GitState::default(), digest);
    assert!(unknown.ends_with("-dev.0+b0123456789"), "{unknown}");
}

#[test]
fn cargo_output_directories_name_their_profile() {
    assert_eq!(profile_for_bin_dir(Path::new("/t/debug")), "dev");
    assert_eq!(profile_for_bin_dir(Path::new("/t/release")), "release");
    assert_eq!(
        profile_for_bin_dir(Path::new("/t/performance")),
        "performance"
    );
}

#[test]
fn a_checkout_is_read_from_git_and_absence_is_unknown() {
    let state = GitState::read(&std::env::temp_dir().join("definitely-not-a-checkout"));
    assert_eq!(state, GitState::default());

    let checkout = GitState::read(Path::new(env!("CARGO_MANIFEST_DIR")));
    if let Some(commit) = &checkout.commit {
        assert_eq!(commit.len(), 40, "full hash expected: {commit}");
        assert!(checkout.commit_count.is_some());
    }
}
