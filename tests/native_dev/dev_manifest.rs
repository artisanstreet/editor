//! Installation and payload staging for the dev home.
//!
//! Both documents are validated through the shipping authorities — the
//! installation manifest through the CLI loader and the payload through the
//! existing verifier — so a home the Editor would refuse fails here first.

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use artisan_editor_cli::{
    manifest::InstallationManifest,
    payload::{self, PAYLOAD_MANIFEST_NAME},
};
use native_dev::{
    BinarySet, DevError, DevPaths, installation_document, provision_manifest, provision_payload,
    stage_binaries,
};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn scratch_dev_dir(case: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "artisan-native-dev-{case}-{}-{id}",
        std::process::id()
    ))
}

fn fixture_sources(case: &str) -> (PathBuf, BinarySet) {
    let root = scratch_dev_dir(case).join("sources");
    std::fs::create_dir_all(&root).expect("fixture sources");
    let mut get = |stem: &str| {
        let path = root.join(native_dev::exe_name(stem));
        std::fs::write(&path, format!("fixture-binary-{stem}")).expect("fixture binary");
        path
    };
    let set = BinarySet {
        ae: get("ae"),
        editor: get("editor"),
        forge: get("forge"),
        installer: get("installer"),
    };
    (root, set)
}

fn cleanup(path: &Path) {
    let _ = std::fs::remove_dir_all(path);
}

#[test]
fn manifest_document_matches_the_shipping_schema() {
    let home = PathBuf::from(if cfg!(windows) {
        r"C:\artisan\dev\home"
    } else {
        "/artisan/dev/home"
    });
    let permanent = home.join("bin").join(native_dev::exe_name("ae"));
    let document = installation_document(&home, &permanent);
    assert_eq!(document["activation_state"].as_str(), Some("active"));
    assert_eq!(document["finalization_state"].as_str(), Some("complete"));
    assert_eq!(document["active_version"].as_str(), Some("dev"));
    assert_eq!(
        document["install_root"].as_str(),
        Some(home.to_string_lossy().as_ref())
    );
}

#[test]
fn provisioned_manifest_passes_the_shipping_loader() {
    let dev_dir = scratch_dev_dir("manifest-ok");
    let paths = DevPaths::new(&dev_dir).expect("absolute dev dir");
    provision_manifest(&paths).expect("manifest provisions");
    let manifest =
        InstallationManifest::load(&paths.manifest_path).expect("shipping loader accepts");
    assert_eq!(manifest.install_root, paths.home);
    assert_eq!(
        manifest.active_version.as_deref(),
        Some(native_dev::DEV_VERSION)
    );
    assert_eq!(
        manifest.forge_executable(),
        paths.version_bin.join(native_dev::exe_name("forge"))
    );
    assert_eq!(
        manifest.editor_executable(),
        paths.version_bin.join(native_dev::exe_name("editor"))
    );
    cleanup(&dev_dir);
}

#[test]
fn staged_payload_verifies_and_tampering_is_reported() {
    let dev_dir = scratch_dev_dir("payload");
    let paths = DevPaths::new(&dev_dir).expect("absolute dev dir");
    let (_sources, set) = fixture_sources("payload");
    let staged = stage_binaries(&set, &paths).expect("binaries stage");
    assert_eq!(staged.len(), 5);
    assert!(staged.iter().all(|(_, written)| *written));

    provision_payload(&paths).expect("payload provisions");
    assert_eq!(
        payload::verify(&paths.version_root),
        payload::PayloadHealth::Verified
    );

    let manifest_bytes =
        std::fs::read(paths.version_root.join(PAYLOAD_MANIFEST_NAME)).expect("payload manifest");
    let manifest: serde_json::Value = serde_json::from_slice(&manifest_bytes).expect("json");
    assert_eq!(manifest["format_version"].as_u64(), Some(1));
    assert_eq!(manifest["files"].as_object().expect("files").len(), 4);

    let forge = paths.version_bin.join(native_dev::exe_name("forge"));
    std::fs::write(&forge, b"tampered").expect("tamper");
    let payload::PayloadHealth::Modified(issues) = payload::verify(&paths.version_root) else {
        panic!("tampered payload verified");
    };
    assert!(
        issues.iter().any(|issue| issue.contains("forge")),
        "issues: {issues:?}"
    );
    let error = provision_payload(&paths).expect_err("re-provision reports drift");
    assert!(
        matches!(error, DevError::PayloadUnverified { .. }),
        "unexpected: {error}"
    );
    cleanup(&dev_dir);
}

#[test]
fn repeat_staging_reuses_identical_binaries() {
    let dev_dir = scratch_dev_dir("repeat");
    let paths = DevPaths::new(&dev_dir).expect("absolute dev dir");
    let (_sources, set) = fixture_sources("repeat");
    let first = stage_binaries(&set, &paths).expect("first stage");
    assert!(first.iter().all(|(_, written)| *written));
    let second = stage_binaries(&set, &paths).expect("second stage");
    assert!(
        second.iter().all(|(_, written)| !written),
        "identical binaries must be reused"
    );
    provision_payload(&paths).expect("payload still verifies");
    cleanup(&dev_dir);
}

#[test]
fn missing_source_binary_fails_before_any_manifest() {
    let dev_dir = scratch_dev_dir("missing-source");
    let paths = DevPaths::new(&dev_dir).expect("absolute dev dir");
    let (_sources, set) = fixture_sources("missing-source");
    std::fs::remove_file(&set.forge).expect("remove fixture");
    let error = native_dev::hash_file(&set.forge).expect_err("missing source cannot hash");
    assert!(error.to_string().contains("forge"), "unexpected: {error}");
    assert!(
        !paths.manifest_path.exists(),
        "no manifest on failed staging"
    );
    cleanup(&dev_dir);
}
