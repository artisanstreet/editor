//! A local build becomes an installed dev release through the shipping
//! installer: assembled and signed by the runner, verified and activated by
//! `artisan-install`, and loadable by the Editor's own discovery.

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use artisan_build_info::{BuildIdentity, BuildInfo, Channel};
use artisan_editor_cli::{manifest::InstallationManifest, payload};
use artisan_install::LocalSigner;
use native_dev::{BinarySet, DevPaths, GitState, assemble, install_tree, staged_editor};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn scratch(case: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "artisan-native-dev-payload-{case}-{}-{id}",
        std::process::id()
    ))
}

fn fixture_set(directory: &Path, editor: &str) -> BinarySet {
    std::fs::create_dir_all(directory).expect("sources");
    let write = |stem: &str, content: &str| {
        let path = directory.join(native_dev::exe_name(stem));
        std::fs::write(&path, content).expect("fixture binary");
        path
    };
    BinarySet {
        ae: write("ae", "fixture-ae"),
        editor: write("editor", editor),
        forge: write("forge", "fixture-forge"),
        installer: write("installer", "fixture-installer"),
    }
}

fn git(commit: &str) -> GitState {
    GitState {
        commit: Some(commit.to_owned()),
        dirty: false,
        commit_count: Some(42),
    }
}

fn install(paths: &DevPaths, work: &Path, binaries: &BinarySet, commit: &str) -> BuildInfo {
    let signer = LocalSigner::load_or_create(&paths.home).expect("local key");
    let tree = work.join("payload");
    let info = assemble(binaries, &git(commit), "dev", &tree, &signer).expect("assembles");
    install_tree(paths, &tree, &signer).expect("installs");
    info
}

#[test]
fn an_installed_build_is_a_verified_dev_release_the_editor_can_load() {
    let work = scratch("install");
    let paths = DevPaths::new(&work.join("Artisan Street Dev")).expect("absolute root");
    let binaries = fixture_set(&work.join("target/debug"), "editor-one");
    let info = install(
        &paths,
        &work,
        &binaries,
        "1111111111111111111111111111111111111111",
    );
    assert_eq!(info.channel, Channel::Dev);
    assert!(
        info.version.starts_with("0.0.0-dev.42+g1111111111.b"),
        "{}",
        info.version
    );

    let manifest = InstallationManifest::load(&paths.manifest_path).expect("shipping loader");
    assert_eq!(
        manifest.active_version.as_deref(),
        Some(info.version.as_str())
    );
    let version_root = paths.active_version_root().expect("active version");
    assert_eq!(
        payload::verify(&version_root),
        payload::PayloadHealth::Verified
    );
    assert_eq!(
        BuildIdentity::for_executable(&staged_editor(&version_root)),
        BuildIdentity::Installed(info)
    );
    assert!(paths.permanent_ae.is_file());
    let _ = std::fs::remove_dir_all(&work);
}

#[test]
fn identical_builds_share_a_version_and_new_builds_get_their_own() {
    let work = scratch("versions");
    let paths = DevPaths::new(&work.join("root")).expect("absolute root");
    let first = fixture_set(&work.join("one"), "editor-one");
    let commit = "2222222222222222222222222222222222222222";
    let installed = install(&paths, &work, &first, commit);
    let again = install(&paths, &work, &first, commit);
    assert_eq!(installed.version, again.version, "same bytes, same version");

    let second = fixture_set(&work.join("two"), "editor-two");
    let rebuilt = install(&paths, &work, &second, commit);
    assert_ne!(rebuilt.version, installed.version, "new bytes, new version");
    assert_eq!(
        std::fs::read(staged_editor(&paths.active_version_root().expect("active")))
            .expect("installed editor"),
        b"editor-two"
    );
    assert!(
        paths
            .home
            .join("versions")
            .join(&installed.version)
            .is_dir(),
        "the previous version stays for rollback"
    );
    let _ = std::fs::remove_dir_all(&work);
}
