//! Development layout: one side-by-side installation root, never the real
//! installation, on local disk.

use std::path::{Path, PathBuf};

use native_dev::{
    DEV_ROOT_NAME, DevError, DevPaths, default_dev_root, exe_name, resolve_dev_root, stage_line,
};

fn absolute_root(name: &str) -> PathBuf {
    let base = if cfg!(windows) {
        PathBuf::from(r"C:\artisan-native-dev-tests")
    } else {
        std::env::temp_dir().join("artisan-native-dev-tests")
    };
    base.join(name)
}

#[test]
fn every_owned_path_lives_under_the_root() {
    let root = absolute_root("layout");
    let paths = DevPaths::new(&root).expect("absolute root");
    assert_eq!(paths.home, root);
    assert_eq!(paths.manifest_path, root.join("installation.json"));
    assert_eq!(paths.permanent_ae, root.join("bin").join(exe_name("ae")));
    for owned in [
        paths.runner_dir(),
        paths.lock_path(),
        paths.database_path(),
        paths.custody_path(),
        paths.readiness_path(),
    ] {
        assert!(owned.starts_with(&root), "{}", owned.display());
    }
    assert!(paths.lock_path().starts_with(paths.runner_dir()));
}

#[test]
fn the_default_root_is_beside_the_real_installation() {
    let Ok(root) = default_dev_root() else {
        return; // no user data directory in this environment
    };
    assert!(root.ends_with(DEV_ROOT_NAME), "{}", root.display());
    assert_ne!(
        root.file_name().and_then(|name| name.to_str()),
        Some("Artisan Street"),
        "the dev root must never be the installed root"
    );
}

#[test]
fn relative_roots_are_rejected() {
    assert!(matches!(
        DevPaths::new(Path::new("relative/root")),
        Err(DevError::NotAbsolute { .. })
    ));
    assert!(matches!(
        resolve_dev_root(Some(Path::new("relative"))),
        Err(DevError::NotAbsolute { .. })
    ));
}

#[test]
fn an_explicit_root_wins() {
    let root = absolute_root("explicit");
    assert_eq!(resolve_dev_root(Some(&root)).expect("explicit root"), root);
}

#[test]
fn only_local_roots_are_accepted() {
    assert!(!native_dev::is_network_share(&absolute_root("local")));
    if cfg!(windows) {
        for shared in [
            r"\\wsl.localhost\Ubuntu\home\ada\Artisan Street Dev",
            r"\\?\UNC\server\share\dev",
        ] {
            assert!(native_dev::is_network_share(Path::new(shared)), "{shared}");
            assert!(matches!(
                resolve_dev_root(Some(Path::new(shared))),
                Err(DevError::NetworkShare { .. })
            ));
        }
    }
}

#[test]
fn binary_names_follow_the_platform() {
    let name = exe_name("editor");
    if cfg!(windows) {
        assert_eq!(name, "editor.exe");
    } else {
        assert_eq!(name, "editor");
    }
}

#[test]
fn stage_lines_are_plain_and_numbered() {
    assert_eq!(
        stage_line(2, 7, "assemble", ""),
        "dev: stage 2/7 assemble ... ok"
    );
    assert_eq!(
        stage_line(3, 7, "install", "dev channel"),
        "dev: stage 3/7 install ... ok (dev channel)"
    );
}
