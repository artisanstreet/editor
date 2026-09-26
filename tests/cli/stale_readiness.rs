//! Reconciling a readiness receipt left by a Forge that did not shut down
//! gracefully, before the next start of the same home.
//!
//! Only a proven-stale receipt is ever removed: it must parse as Forge
//! readiness, its process must not be a live Forge, the home's custody lock
//! must be free, and its parents must be plain directories. Everything else
//! is preserved and refused.

use std::path::{Path, PathBuf};

use artisan_editor_cli::{
    CliError,
    process::{ReadinessReconcile, reconcile_stale_readiness},
};

/// A syntactically valid receipt naming a pid no live process can have.
fn dead_forge_receipt() -> Vec<u8> {
    br#"{"schema":"artisan-forge-ready-v1","endpoint":"127.0.0.1:9","certificate_sha256":"abababababababababababababababababababababababababababababababab","pid":4294967295}"#.to_vec()
}

struct Home {
    _scratch: tempfile::TempDir,
    readiness: PathBuf,
    custody: PathBuf,
    forge: PathBuf,
}

/// A home whose readiness and custody directories exist, as `ae setup`
/// creates them.
fn home() -> Home {
    let scratch = tempfile::tempdir().expect("scratch");
    let root = scratch.path().join("Artisan Street Dev");
    let readiness = root.join("readiness").join("forge.json");
    let custody = root.join("custody").join("forge.lock");
    for path in [&readiness, &custody] {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("runtime dir");
    }
    Home {
        forge: root.join("versions/0.0.0-dev.1/bin/forge"),
        _scratch: scratch,
        readiness,
        custody,
    }
}

fn reconcile(home: &Home) -> artisan_editor_cli::Result<ReadinessReconcile> {
    reconcile_stale_readiness(&home.readiness, &home.custody, &home.forge)
}

#[test]
fn missing_readiness_reconciles_to_absent() {
    let home = home();
    assert_eq!(
        reconcile(&home).expect("absent"),
        ReadinessReconcile::Absent
    );
}

#[test]
fn a_stale_receipt_is_removed_and_its_siblings_preserved() {
    let home = home();
    std::fs::write(&home.readiness, dead_forge_receipt()).expect("stale receipt");
    let parent = home.readiness.parent().expect("parent");
    let temporary = parent.join(".artisan-forge-ready-19808-0.tmp");
    std::fs::write(&temporary, b"orphan publish temporary").expect("temporary");
    let notes = parent.join("notes.txt");
    std::fs::write(&notes, b"operator notes").expect("notes");

    assert_eq!(
        reconcile(&home).expect("stale"),
        ReadinessReconcile::CleanedStale { pid: u32::MAX }
    );
    assert!(!home.readiness.exists());
    assert!(temporary.exists(), "publish temporaries are never swept");
    assert!(notes.exists());
}

#[test]
fn malformed_oversized_and_non_file_receipts_are_preserved() {
    for (contents, expected) in [
        (b"not a receipt".to_vec(), "malformed"),
        (vec![b'x'; 5000], "size bound"),
    ] {
        let home = home();
        std::fs::write(&home.readiness, contents).expect("receipt");
        let error = reconcile(&home).expect_err("refused");
        assert!(error.to_string().contains(expected), "{error}");
        assert!(home.readiness.exists());
    }
    let home = home();
    std::fs::create_dir_all(&home.readiness).expect("directory receipt");
    let error = reconcile(&home).expect_err("refused");
    assert!(error.to_string().contains("not a regular file"), "{error}");
    assert!(home.readiness.is_dir());
}

#[test]
fn held_custody_refuses_until_it_is_released() {
    let home = home();
    std::fs::write(&home.readiness, dead_forge_receipt()).expect("stale receipt");
    std::fs::write(&home.custody, b"custody carrier").expect("custody");
    let held = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&home.custody)
        .expect("custody opens");
    fs2::FileExt::try_lock_exclusive(&held).expect("test holds custody");
    assert!(matches!(
        reconcile(&home),
        Err(CliError::ForgeCustodyHeld { .. })
    ));
    assert!(home.readiness.exists());
    drop(held);
    assert_eq!(
        reconcile(&home).expect("stale"),
        ReadinessReconcile::CleanedStale { pid: u32::MAX }
    );
}

#[test]
fn a_missing_custody_directory_fails_closed() {
    let home = home();
    std::fs::write(&home.readiness, dead_forge_receipt()).expect("stale receipt");
    std::fs::remove_dir_all(home.custody.parent().expect("custody dir")).expect("remove");
    let error = reconcile(&home).expect_err("refused");
    assert!(error.to_string().contains("custody"), "{error}");
    assert!(home.readiness.exists());
}

#[cfg(unix)]
#[test]
fn a_symlinked_parent_refuses_and_preserves() {
    use std::os::unix::fs::symlink;

    let scratch = tempfile::tempdir().expect("scratch");
    let real = scratch.path().join("real");
    std::fs::create_dir_all(real.join("readiness")).expect("readiness");
    std::fs::create_dir_all(real.join("custody")).expect("custody");
    let linked = scratch.path().join("linked");
    symlink(&real, &linked).expect("symlink");
    let readiness = linked.join("readiness/forge.json");
    std::fs::write(&readiness, dead_forge_receipt()).expect("stale receipt");
    let error = reconcile_stale_readiness(
        &readiness,
        &linked.join("custody/forge.lock"),
        Path::new("/nonexistent/forge"),
    )
    .expect_err("refused");
    assert!(error.to_string().contains("symbolic link"), "{error}");
    assert!(readiness.exists());
}
