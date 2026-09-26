//! A Nix-built payload becomes an installed dev release through the shipping
//! installer: signed by the runner beside the read-only payload, verified
//! and activated by `artisan-install`, and loadable by the Editor's own
//! discovery.

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use artisan_build_info::{BuildIdentity, BuildInfo, Channel, FORMAT_VERSION};
use artisan_editor_cli::{manifest::InstallationManifest, payload};
use artisan_install::LocalSigner;
use native_dev::{DevPaths, install_payload, payload_identity, sign_payload, staged_editor};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn scratch(case: &str) -> PathBuf {
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "artisan-native-dev-payload-{case}-{}-{id}",
        std::process::id()
    ))
}

/// A payload shaped exactly like a Nix stage output.
fn nix_payload(directory: &Path, version: &str, editor: &str) -> (PathBuf, BuildInfo) {
    let payload = directory.join(format!("payload-{version}"));
    std::fs::create_dir_all(payload.join("bin")).expect("bin");
    for (stem, content) in [
        ("ae", "fixture-ae"),
        ("editor", editor),
        ("forge", "fixture-forge"),
        ("installer", "fixture-installer"),
    ] {
        std::fs::write(
            payload.join("bin").join(native_dev::exe_name(stem)),
            content,
        )
        .expect("fixture binary");
    }
    let identity = BuildInfo {
        format_version: FORMAT_VERSION,
        version: version.to_owned(),
        channel: Channel::Dev,
        commit: Some("1111111111111111111111111111111111111111".to_owned()),
        dirty: false,
        profile: "production-debug".to_owned(),
        target: "x86_64-unknown-linux-gnu".to_owned(),
        built_at: None,
    };
    std::fs::create_dir_all(payload.join("resources")).expect("resources");
    std::fs::write(
        payload.join(artisan_build_info::RESOURCE_PATH),
        identity.to_json(),
    )
    .expect("identity");
    (payload, identity)
}

fn install(paths: &DevPaths, payload: &Path) -> BuildInfo {
    let identity = payload_identity(payload).expect("payload identity");
    let signer = LocalSigner::load_or_create(&paths.home).expect("local key");
    let manifests = paths.runner_dir().join("manifest");
    sign_payload(payload, &manifests, &identity, &signer).expect("signs");
    install_payload(paths, payload, &manifests, &signer, false).expect("installs");
    identity
}

#[test]
fn a_nix_payload_installs_as_a_verified_dev_release_the_editor_can_load() {
    let work = scratch("install");
    let paths = DevPaths::new(&work.join("Artisan Street Dev")).expect("absolute root");
    let (payload, identity) = nix_payload(&work, "0.0.0-dev.42+g1111111111.naaaaaaaaaa", "one");
    assert_eq!(install(&paths, &payload), identity);
    assert!(
        !payload.join(artisan_install::TREE_MANIFEST_NAME).exists(),
        "the payload itself stays untouched"
    );

    let manifest = InstallationManifest::load(&paths.manifest_path).expect("shipping loader");
    assert_eq!(
        manifest.active_version.as_deref(),
        Some(identity.version.as_str())
    );
    let version_root = paths.active_version_root().expect("active version");
    assert_eq!(
        payload::verify(&version_root),
        payload::PayloadHealth::Verified
    );
    assert_eq!(
        BuildIdentity::for_executable(&staged_editor(&version_root)),
        BuildIdentity::Installed(identity)
    );
    assert!(paths.permanent_ae.is_file());
    let _ = std::fs::remove_dir_all(&work);
}

#[test]
fn reinstalling_a_payload_is_idempotent_and_new_payloads_keep_the_old_for_rollback() {
    let work = scratch("versions");
    let paths = DevPaths::new(&work.join("root")).expect("absolute root");
    let (first, first_identity) = nix_payload(&work, "0.0.0-dev.1+naaaaaaaaaa", "one");
    install(&paths, &first);
    install(&paths, &first);

    let (second, second_identity) = nix_payload(&work, "0.0.0-dev.2+nbbbbbbbbbb", "two");
    install(&paths, &second);
    assert_eq!(
        std::fs::read(staged_editor(&paths.active_version_root().expect("active")))
            .expect("installed editor"),
        b"two"
    );
    assert_ne!(first_identity.version, second_identity.version);
    assert!(
        paths
            .home
            .join("versions")
            .join(&first_identity.version)
            .is_dir(),
        "the previous version stays for rollback"
    );
    let _ = std::fs::remove_dir_all(&work);
}

#[test]
fn a_directory_without_nix_identity_is_not_a_payload() {
    let work = scratch("not-a-payload");
    std::fs::create_dir_all(work.join("bin")).expect("bin");
    let error = payload_identity(&work).expect_err("no identity");
    assert!(error.to_string().contains("nix build"), "{error}");
    let _ = std::fs::remove_dir_all(&work);
}
