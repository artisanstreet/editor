//! Runner locking and startup-receipt hygiene.
//!
//! Concurrent runs on one root serialize on an OS lock that releases itself
//! when the holder dies, and every launch reads its own fresh receipt.

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use native_dev::{DevError, DevLock, DevPaths, clear_stale_receipt, fresh_receipt_path};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn scratch_root(case: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "artisan-native-dev-{case}-{}-{id}",
        std::process::id()
    ))
}

fn cleanup(path: &Path) {
    let _ = std::fs::remove_dir_all(path);
}

#[test]
fn second_lock_holder_fails_while_first_is_live() {
    let root = scratch_root("lock");
    let paths = DevPaths::new(&root).expect("absolute root");
    let _first = DevLock::acquire(&paths).expect("first holder acquires");
    let error = DevLock::acquire(&paths)
        .err()
        .expect("second holder is refused");
    assert!(
        matches!(error, DevError::StagingLocked { .. }),
        "unexpected: {error}"
    );
    cleanup(&root);
}

#[test]
fn lock_releases_when_the_holder_drops() {
    let root = scratch_root("lock-release");
    let paths = DevPaths::new(&root).expect("absolute root");
    {
        let _holder = DevLock::acquire(&paths).expect("acquires");
    }
    DevLock::acquire(&paths).expect("re-acquires after drop");
    cleanup(&root);
}

#[test]
fn each_run_uses_its_own_receipt_inside_the_runner_directory() {
    let root = scratch_root("receipt");
    let paths = DevPaths::new(&root).expect("absolute root");
    let receipt = fresh_receipt_path(&paths);
    assert!(receipt.starts_with(paths.runner_dir()));
    assert!(
        receipt
            .to_string_lossy()
            .contains(&std::process::id().to_string())
    );
    std::fs::create_dir_all(paths.runner_dir()).expect("runner dir");
    std::fs::write(&receipt, b"stale").expect("stale receipt");
    clear_stale_receipt(&receipt).expect("clears");
    assert!(!receipt.exists());
    clear_stale_receipt(&receipt).expect("missing is fine");
    cleanup(&root);
}
