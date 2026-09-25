//! Development layout: every path stays under the isolated dev directory.
//!
//! The central isolation invariant is that the dev home is derived from the
//! dev directory alone. No platform default and no real installation path
//! may appear in it.

use std::path::{Path, PathBuf};

use native_dev::{DevPaths, default_base_dir, exe_name, resolve_dev_dir, stage_line};

fn absolute_dev_dir(name: &str) -> PathBuf {
    let base = if cfg!(windows) {
        PathBuf::from(r"C:\artisan-native-dev-tests")
    } else {
        std::env::temp_dir().join("artisan-native-dev-tests")
    };
    base.join(name)
}

#[test]
fn dev_paths_live_under_the_dev_directory() {
    let dev_dir = absolute_dev_dir("layout");
    let paths = DevPaths::new(&dev_dir).expect("absolute dev dir");
    assert_eq!(paths.dev_dir, dev_dir);
    assert!(paths.home.starts_with(&dev_dir));
    assert!(paths.version_root.starts_with(&paths.home));
    assert!(paths.version_bin.starts_with(&paths.version_root));
    assert!(paths.manifest_path.starts_with(&paths.home));
    assert!(paths.permanent_ae.starts_with(&paths.home));
    assert_eq!(paths.version_root.join("bin"), paths.version_bin);
}

#[test]
fn dev_home_never_resolves_to_a_platform_default() {
    let dev_dir = absolute_dev_dir("no-default");
    let paths = DevPaths::new(&dev_dir).expect("absolute dev dir");
    let home = paths.home.to_string_lossy().to_lowercase();
    assert!(
        !home.ends_with("artisan street"),
        "must not be the installed home: {home}"
    );
    assert!(
        !home.ends_with("artisan"),
        "must not be a legacy root: {home}"
    );
}

#[test]
fn relative_dev_directories_are_rejected() {
    let error = DevPaths::new(Path::new(".dist/dev")).expect_err("relative dir is rejected");
    assert!(error.to_string().contains(".dist"), "unexpected: {error}");
}

#[test]
fn default_base_prefers_the_workspace_directory() {
    let workspace = if cfg!(windows) {
        PathBuf::from(r"C:\repo")
    } else {
        PathBuf::from("/repo")
    };
    let current = if cfg!(windows) {
        PathBuf::from(r"C:\other")
    } else {
        PathBuf::from("/other")
    };
    assert_eq!(
        default_base_dir(Some(&workspace), &current),
        workspace.join(".dist/dev")
    );
    assert_eq!(default_base_dir(None, &current), current.join(".dist/dev"));
}

#[test]
fn explicit_resolution_keeps_absolute_and_rejects_relative() {
    let absolute = absolute_dev_dir("explicit");
    assert_eq!(
        resolve_dev_dir(Some(&absolute)).expect("absolute resolves"),
        absolute
    );
    let error = resolve_dev_dir(Some(Path::new("relative/dev"))).expect_err("relative is rejected");
    assert!(
        error.to_string().contains("relative"),
        "unexpected: {error}"
    );
}

#[test]
fn binary_names_follow_the_platform() {
    if cfg!(windows) {
        assert_eq!(exe_name("forge"), "forge.exe");
    } else {
        assert_eq!(exe_name("forge"), "forge");
    }
}

#[test]
fn forge_sidecar_paths_stay_inside_the_home() {
    let paths = DevPaths::new(&absolute_dev_dir("sidecars")).expect("absolute dev dir");
    for path in [
        paths.database_path(),
        paths.custody_path(),
        paths.readiness_path(),
    ] {
        assert!(path.starts_with(&paths.home), "leaks: {}", path.display());
    }
    assert_ne!(paths.database_path(), paths.custody_path());
    assert_ne!(paths.custody_path(), paths.readiness_path());
}

#[test]
fn stage_lines_are_plain_and_numbered() {
    let line = stage_line(3, 7, "stage", "2 rewritten, 3 reused");
    assert_eq!(line, "dev: stage 3/7 stage ... ok (2 rewritten, 3 reused)");
    let bare = stage_line(1, 7, "resolve", "");
    assert_eq!(bare, "dev: stage 1/7 resolve ... ok");
    assert!(!line.contains('\x1b'), "no TTY escape codes");
}

#[test]
fn staging_and_lock_paths_stay_inside_the_dev_tree() {
    let dev_dir = absolute_dev_dir("staging-layout");
    let paths = DevPaths::new(&dev_dir).expect("absolute dev dir");
    assert!(paths.staging_root().starts_with(&dev_dir));
    assert!(paths.previous_root().starts_with(&dev_dir));
    assert!(paths.lock_path().starts_with(&dev_dir));
    assert!(paths.receipt_path().starts_with(&dev_dir));
    assert_ne!(paths.staging_root(), paths.version_root);
    assert_ne!(paths.previous_root(), paths.version_root);
    assert_ne!(paths.staging_root(), paths.previous_root());
    assert!(
        !paths
            .staging_root()
            .to_string_lossy()
            .contains("artisan street"),
        "staging must not resemble the installed home"
    );
}

#[test]
fn only_local_dev_directories_are_accepted() {
    assert!(!native_dev::is_network_share(&absolute_dev_dir("local")));
    if cfg!(windows) {
        for shared in [
            r"\wsl.localhost\Ubuntu\home\ada\editor\.dist\dev",
            r"\?\UNC\server\share\dev",
        ] {
            assert!(native_dev::is_network_share(Path::new(shared)), "{shared}");
            assert!(matches!(
                resolve_dev_dir(Some(Path::new(shared))),
                Err(native_dev::DevError::NetworkShare { .. })
            ));
        }
    }
}
