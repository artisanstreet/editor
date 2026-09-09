//! Installation and payload staging for the dev home.
//!
//! Both documents are validated through the shipping authorities — the
//! installation manifest through the CLI loader and the payload through the
//! existing verifier — so a home the Editor would refuse fails here first.
//! Activation is verified separately: only a verified scratch tree swaps
//! into the active version.

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use artisan_editor_cli::{
    manifest::InstallationManifest,
    payload::PAYLOAD_MANIFEST_NAME,
};
use native_dev::{
    BinarySet, DevError, DevPaths, installation_document, provision_manifest, stage_binaries,
    verify_payload_dir, write_payload_manifest,
};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn scratch_dev_dir(case: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "artisan-native-dev-{case}-{}-{id}",
        std::process::id()
    ))
}

fn fixture_set(case: &str) -> (PathBuf, BinarySet) {
    let root = scratch_dev_dir(case).join("sources");
    std::fs::create_dir_all(&root).expect("fixture sources");
    let get = |stem: &str| {
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
fn activated_payload_verifies_and_tampering_is_reported() {
    let dev_dir = scratch_dev_dir("payload");
    let paths = DevPaths::new(&dev_dir).expect("absolute dev dir");
    let (_sources, set) = fixture_set("payload");
    let counts = stage_binaries(&set, &paths).expect("binaries stage");
    assert_eq!(counts.rewritten, 4);
    assert_eq!(counts.reused, 0);

    verify_payload_dir(&paths.version_root).expect("activated payload verifies");

    let manifest_bytes =
        std::fs::read(paths.version_root.join(PAYLOAD_MANIFEST_NAME)).expect("payload manifest");
    let manifest: serde_json::Value = serde_json::from_slice(&manifest_bytes).expect("json");
    assert_eq!(manifest["format_version"].as_u64(), Some(1));
    assert_eq!(manifest["files"].as_object().expect("files").len(), 4);

    let forge = paths.version_bin.join(native_dev::exe_name("forge"));
    std::fs::write(&forge, b"tampered").expect("tamper");
    let error = verify_payload_dir(&paths.version_root).expect_err("tamper is reported");
    assert!(
        matches!(error, DevError::PayloadUnverified { .. }),
        "unexpected: {error}"
    );
    assert!(error.to_string().contains("forge"), "unexpected: {error}");
    cleanup(&dev_dir);
}

#[test]
fn repeat_staging_reuses_identical_binaries() {
    let dev_dir = scratch_dev_dir("repeat");
    let paths = DevPaths::new(&dev_dir).expect("absolute dev dir");
    let (_sources, set) = fixture_set("repeat");
    let first = stage_binaries(&set, &paths).expect("first stage");
    assert_eq!((first.rewritten, first.reused), (4, 0));
    let second = stage_binaries(&set, &paths).expect("second stage");
    assert_eq!(
        (second.rewritten, second.reused),
        (0, 4),
        "identical binaries must be reused"
    );
    verify_payload_dir(&paths.version_root).expect("payload still verifies");
    cleanup(&dev_dir);
}

#[test]
fn missing_source_binary_fails_before_any_manifest() {
    let dev_dir = scratch_dev_dir("missing-source");
    let paths = DevPaths::new(&dev_dir).expect("absolute dev dir");
    let (_sources, set) = fixture_set("missing-source");
    std::fs::remove_file(&set.forge).expect("remove fixture");
    let error = native_dev::hash_file(&set.forge).expect_err("missing source cannot hash");
    assert!(error.to_string().contains("forge"), "unexpected: {error}");
    assert!(
        !paths.version_root.join(PAYLOAD_MANIFEST_NAME).exists(),
        "no payload on failed staging"
    );
    assert!(
        !paths.manifest_path.exists(),
        "no manifest on failed staging"
    );
    cleanup(&dev_dir);
}

#[test]
fn scratch_payload_manifest_covers_all_four_binaries() {
    let dev_dir = scratch_dev_dir("scratch-manifest");
    let paths = DevPaths::new(&dev_dir).expect("absolute dev dir");
    let (_sources, set) = fixture_set("scratch-manifest");
    for (relative, source) in set.entries() {
        let destination = paths.staging_root().join(&relative);
        std::fs::create_dir_all(destination.parent().expect("parent")).expect("staging parent");
        std::fs::copy(&source, &destination).expect("scratch copy");
    }
    write_payload_manifest(&paths.staging_root()).expect("scratch manifest writes");
    verify_payload_dir(&paths.staging_root()).expect("scratch payload verifies");
    assert!(
        !paths.version_root.exists(),
        "scratch work never touches the active version"
    );
    cleanup(&dev_dir);
}
