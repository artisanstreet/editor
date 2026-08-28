//! Focused tests for exact open-descriptor file identity.

#[path = "../../modules/backend/src/file_identity_policy.rs"]
mod file_identity_policy;

use std::fs::{self, File, OpenOptions};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use file_identity_policy::{
    FileIdentity, normalize_uint64, read_file_identity, same_file_identity,
};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A bounded test fixture whose open file remains available until its identity
/// observations have completed.
struct TemporaryFile {
    file: File,
    path: PathBuf,
}

impl TemporaryFile {
    fn new(label: &str) -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "artisan-file-identity-{label}-{}-{sequence}",
            std::process::id()
        ));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .expect("temporary identity fixture should be created");
        Self { file, path }
    }

    fn replacement_path(&self) -> PathBuf {
        self.path.with_extension("replacement")
    }
}

impl Drop for TemporaryFile {
    fn drop(&mut self) {
        let _cleanup_result = fs::remove_file(&self.path);
        let _cleanup_result = fs::remove_file(self.replacement_path());
    }
}

#[test]
fn normalization_covers_zero_max_and_signed_modulo_boundaries() {
    assert_eq!(normalize_uint64(0_i128), 0);
    assert_eq!(normalize_uint64(u64::MAX), u64::MAX);
    assert_eq!(normalize_uint64(-1_i128), u64::MAX);
    assert_eq!(normalize_uint64(i64::MAX), i64::MAX as u64);
    assert_eq!(normalize_uint64(i64::MIN), 1_u64 << 63);
    assert_eq!(normalize_uint64(1_i128 << 64), 0);
    assert_eq!(normalize_uint64(-(1_i128 << 64)), 0);
    assert_eq!(normalize_uint64(-(1_i128 << 64) - 1), u64::MAX);
}

#[test]
fn equality_requires_both_device_and_inode_fields() {
    let identity = FileIdentity::new(0, 0);
    let max = FileIdentity::new(u64::MAX, u64::MAX);

    assert!(same_file_identity(identity, identity));
    assert!(same_file_identity(max, max));
    assert!(!same_file_identity(identity, max));
    assert!(!same_file_identity(
        FileIdentity::new(1, identity.inode),
        identity
    ));
    assert!(!same_file_identity(
        FileIdentity::new(identity.device, 1),
        identity
    ));
}

#[test]
fn duplicate_observations_of_one_descriptor_are_equal() {
    let fixture = TemporaryFile::new("duplicate");
    let first = read_file_identity(&fixture.file).expect("first descriptor identity");
    let second = read_file_identity(&fixture.file).expect("second descriptor identity");
    let duplicate_descriptor = fixture.file.try_clone().expect("descriptor clone");
    let third = read_file_identity(&duplicate_descriptor).expect("cloned descriptor identity");

    assert!(same_file_identity(first, second));
    assert!(same_file_identity(first, third));
}

#[test]
fn identity_stays_bound_to_the_open_descriptor_when_the_path_is_replaced() {
    let fixture = TemporaryFile::new("replacement");
    let before = read_file_identity(&fixture.file).expect("identity before replacement");
    let moved_path = fixture.replacement_path();

    fs::rename(&fixture.path, &moved_path).expect("open fixture should be renameable");
    fs::write(&fixture.path, b"replacement").expect("replacement should be created");

    let from_original_descriptor =
        read_file_identity(&fixture.file).expect("identity from original descriptor");
    let replacement = File::open(&fixture.path).expect("replacement should be openable");
    let after_path_replacement =
        read_file_identity(&replacement).expect("replacement descriptor identity");

    assert!(same_file_identity(before, from_original_descriptor));
    assert!(!same_file_identity(before, after_path_replacement));
}

#[cfg(windows)]
#[test]
fn descriptor_identity_failure_is_returned_without_a_path_fallback() {
    let descriptor = File::open("NUL").expect("Windows NUL descriptor should open");
    let error = read_file_identity(&descriptor).expect_err("NUL has no file identity");

    assert_eq!(error.kind(), std::io::ErrorKind::Other);
    assert!(error.to_string().contains("file identity helper"));
}

#[cfg(not(any(unix, windows)))]
#[test]
fn unsupported_platform_identity_is_an_explicit_error() {
    let fixture = TemporaryFile::new("unsupported");
    assert_eq!(
        read_file_identity(&fixture.file).expect_err("unsupported platform"),
        std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "exact file identity is unsupported on this platform"
        )
    );
}
