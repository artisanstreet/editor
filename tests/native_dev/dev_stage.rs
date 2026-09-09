//! Locking and stage/activate coherence for dev updates.
//!
//! The exact-coherence contract: a failed update leaves the active version,
//! the manifest, and dev data exactly as they were. Binaries stage into a
//! scratch directory, verify there, and only then swap into the active
//! version; concurrent runs serialize on an OS lock that releases itself
//! when the holder dies.

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use artisan_editor_cli::payload;
use native_dev::{BinarySet, DevError, DevLock, DevPaths, stage_binaries, verify_payload_dir};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn scratch_dev_dir(case: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "artisan-native-dev-{case}-{}-{id}",
        std::process::id()
    ))
}

fn fixture_set(tag: &str, root: &Path) -> BinarySet {
    let mut get = |stem: &str| {
        let path = root.join(native_dev::exe_name(stem));
        std::fs::write(&path, format!("fixture-{tag}-{stem}")).expect("fixture binary");
        path
    };
    BinarySet {
        ae: get("ae"),
        editor: get("editor"),
        forge: get("forge"),
        installer: get("installer"),
    }
}

fn cleanup(path: &Path) {
    let _ = std::fs::remove_dir_all(path);
}

#[test]
fn second_lock_holder_fails_while_first_is_live() {
    let dev_dir = scratch_dev_dir("lock");
    let paths = DevPaths::new(&dev_dir).expect("absolute dev dir");
    let _first = DevLock::acquire(&paths).expect("first holder acquires");
    let error = DevLock::acquire(&paths).expect_err("second holder is refused");
    assert!(
        matches!(error, DevError::StagingLocked { .. }),
        "unexpected: {error}"
    );
    assert!(error.to_string().contains("locked"), "unexpected: {error}");
    cleanup(&dev_dir);
}

#[test]
fn lock_releases_when_the_holder_drops() {
    let dev_dir = scratch_dev_dir("lock-release");
    let paths = DevPaths::new(&dev_dir).expect("absolute dev dir");
    {
        let _holder = DevLock::acquire(&paths).expect("acquires");
    }
    DevLock::acquire(&paths).expect("re-acquires after drop");
    cleanup(&dev_dir);
}

#[test]
fn failed_update_leaves_the_active_version_untouched() {
    let dev_dir = scratch_dev_dir("coherence");
    let paths = DevPaths::new(&dev_dir).expect("absolute dev dir");
    let sources = dev_dir.join("sources");
    std::fs::create_dir_all(&sources).expect("sources");
    let good = fixture_set("good", &sources);
    stage_binaries(&good, &paths).expect("good version activates");
    let active_forge = paths.version_bin.join(native_dev::exe_name("forge"));
    assert_eq!(
        std::fs::read(&active_forge).expect("reads"),
        b"fixture-good-forge"
    );

    let broken_root = dev_dir.join("broken");
    std::fs::create_dir_all(&broken_root).expect("broken sources");
    let broken = fixture_set("broken", &broken_root);
    std::fs::remove_file(&broken.editor).expect("remove editor");
    let error = stage_binaries(&broken, &paths).expect_err("broken update fails");
    assert!(error.to_string().contains("stage"), "unexpected: {error}");

    assert_eq!(
        std::fs::read(&active_forge).expect("reads"),
        b"fixture-good-forge",
        "failed update must not touch the active version"
    );
    assert_eq!(
        payload::verify(&paths.version_root),
        payload::PayloadHealth::Verified
    );
    assert!(
        !paths.previous_root().exists(),
        "no backup leaks after a failed update"
    );
    assert!(
        !paths.staging_root().exists(),
        "no scratch leaks after a failed update"
    );
    cleanup(&dev_dir);
}

#[test]
fn successful_update_replaces_and_cleans_up() {
    let dev_dir = scratch_dev_dir("replace");
    let paths = DevPaths::new(&dev_dir).expect("absolute dev dir");
    let first_root = dev_dir.join("first");
    std::fs::create_dir_all(&first_root).expect("first sources");
    stage_binaries(&fixture_set("v1", &first_root), &paths).expect("v1 activates");

    let second_root = dev_dir.join("second");
    std::fs::create_dir_all(&second_root).expect("second sources");
    let counts = stage_binaries(&fixture_set("v2", &second_root), &paths).expect("v2 activates");
    assert_eq!((counts.rewritten, counts.reused), (4, 0));

    let active_forge = paths.version_bin.join(native_dev::exe_name("forge"));
    assert_eq!(
        std::fs::read(&active_forge).expect("reads"),
        b"fixture-v2-forge"
    );
    verify_payload_dir(&paths.version_root).expect("new payload verifies");
    assert!(
        !paths.previous_root().exists(),
        "backup is removed after activation"
    );
    assert!(
        !paths.staging_root().exists(),
        "scratch is removed after activation"
    );
    cleanup(&dev_dir);
}

#[test]
fn partially_changed_update_stages_only_differences() {
    let dev_dir = scratch_dev_dir("partial");
    let paths = DevPaths::new(&dev_dir).expect("absolute dev dir");
    let first_root = dev_dir.join("first");
    std::fs::create_dir_all(&first_root).expect("first sources");
    let first = fixture_set("same", &first_root);
    stage_binaries(&first, &paths).expect("first activates");

    let second_root = dev_dir.join("second");
    std::fs::create_dir_all(&second_root).expect("second sources");
    let second = fixture_set("same", &second_root);
    std::fs::write(
        &second_root.join(native_dev::exe_name("forge")),
        b"new-forge-bytes",
    )
    .expect("change forge");
    let counts = stage_binaries(&second, &paths).expect("partial update activates");
    assert_eq!((counts.rewritten, counts.reused), (1, 3));
    let active_forge = paths.version_bin.join(native_dev::exe_name("forge"));
    assert_eq!(
        std::fs::read(&active_forge).expect("reads"),
        b"new-forge-bytes"
    );
    cleanup(&dev_dir);
}
