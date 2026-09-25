//! Build identity of locally staged payloads: derived from the checkout,
//! staged under the payload manifest, and readable by the staged binaries.

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use artisan_build_info::{BuildIdentity, BuildInfo, Channel};
use artisan_editor_cli::payload;
use native_dev::{
    BinarySet, DevPaths, GitState, dev_build_info, dev_version, profile_for_bin_dir, stage_payload,
};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn scratch_dev_dir(case: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "artisan-native-dev-identity-{case}-{}-{id}",
        std::process::id()
    ))
}

fn fixture_set(root: &Path) -> BinarySet {
    std::fs::create_dir_all(root).expect("sources");
    let get = |stem: &str| {
        let path = root.join(native_dev::exe_name(stem));
        std::fs::write(&path, format!("fixture-{stem}")).expect("fixture binary");
        path
    };
    BinarySet {
        ae: get("ae"),
        editor: get("editor"),
        forge: get("forge"),
        installer: get("installer"),
    }
}

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
    let outside = std::env::temp_dir();
    let state = GitState::read(&outside.join("definitely-not-a-checkout"));
    assert_eq!(state, GitState::default());

    let checkout = GitState::read(Path::new(env!("CARGO_MANIFEST_DIR")));
    if let Some(commit) = &checkout.commit {
        assert_eq!(commit.len(), 40, "full hash expected: {commit}");
        assert!(checkout.commit_count.is_some());
    }
}

#[test]
fn staged_identity_is_covered_by_the_payload_and_readable_by_binaries() {
    let dev_dir = scratch_dev_dir("stage");
    let paths = DevPaths::new(&dev_dir).expect("absolute dev dir");
    let sources = dev_dir.join("target").join("debug");
    let set = fixture_set(&sources);
    let identity = dev_build_info(&git(Some("feedfacecafebeef00"), true, Some(3)), &sources);
    assert_eq!(identity.channel, Channel::Dev);
    assert_eq!(identity.profile, "dev");

    stage_payload(&set, Some(&identity), &paths).expect("stages");
    assert_eq!(
        payload::verify(&paths.version_root),
        payload::PayloadHealth::Verified
    );
    assert_eq!(
        BuildInfo::read(&paths.version_root).expect("reads"),
        identity
    );
    assert_eq!(
        BuildIdentity::for_executable(&paths.version_bin.join(native_dev::exe_name("editor"))),
        BuildIdentity::Installed(identity.clone())
    );

    let again = stage_payload(&set, Some(&identity), &paths).expect("restages");
    assert_eq!(again.rewritten, 0, "identical payload must not reactivate");

    let next = dev_build_info(&git(Some("0123456789abcdef00"), false, Some(4)), &sources);
    let changed = stage_payload(&set, Some(&next), &paths).expect("new identity");
    assert_eq!(changed.rewritten, 1, "only the identity changed");
    assert_eq!(BuildInfo::read(&paths.version_root).expect("reads"), next);
    assert_eq!(
        payload::verify(&paths.version_root),
        payload::PayloadHealth::Verified
    );

    let _ = std::fs::remove_dir_all(&dev_dir);
}
