//! Installing from local sources: unpacked trees signed with an
//! installation's local key, and release directories read from disk, go
//! through the same verification and activation as a downloaded release.

use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
};

use artisan_install::{
    InstallIntegrationOptions, InstallOptions, InstallerError, LocalRelease, LocalSigner, Platform,
    ReleaseSource, TREE_MANIFEST_NAME, TrustKey, install, local_trust,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};

fn exe(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_owned()
    }
}

/// Writes an unpacked payload whose binaries carry `tag`.
fn write_tree(tree: &Path, tag: &str) {
    std::fs::create_dir_all(tree.join("bin")).unwrap();
    std::fs::create_dir_all(tree.join("resources")).unwrap();
    for stem in ["ae", "installer", "editor", "forge"] {
        let content = if stem == "editor" {
            format!("{tag}-{stem}")
        } else {
            format!("stable-{stem}")
        };
        std::fs::write(tree.join("bin").join(exe(stem)), content).unwrap();
    }
    std::fs::write(
        tree.join("resources").join("build-info.json"),
        format!("{{\"tag\":\"{tag}\"}}"),
    )
    .unwrap();
}

fn signed_tree(root: &Path, directory: &Path, version: &str, tag: &str) -> PathBuf {
    let tree = directory.join(format!("tree-{tag}"));
    write_tree(&tree, tag);
    let signer = LocalSigner::load_or_create(root).unwrap();
    signer
        .write_tree_manifest(
            &tree,
            &LocalRelease {
                product_version: version.to_owned(),
                platform: Platform::detect().unwrap(),
            },
        )
        .unwrap();
    tree
}

fn options(root: &Path, source: ReleaseSource, trust: TrustKey) -> InstallOptions {
    InstallOptions {
        source,
        platform: Platform::detect().unwrap(),
        install_root: root.to_path_buf(),
        trust,
        expected_channel: None,
        run_setup: false,
        restore_forge: false,
        integrations: InstallIntegrationOptions {
            register_protocol: false,
            register_shortcuts: false,
            register_path: false,
        },
        retirement: None,
    }
}

fn installation(root: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(root.join("installation.json")).unwrap()).unwrap()
}

#[tokio::test]
async fn a_signed_tree_installs_and_activates_as_a_dev_release() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("Artisan Street Dev");
    let tree = signed_tree(&root, directory.path(), "0.0.0-dev.1+gaaaa", "one");
    let trust = local_trust(&root).unwrap();

    install(options(&root, ReleaseSource::Tree { path: tree }, trust))
        .await
        .unwrap();

    let state = installation(&root);
    assert_eq!(state["active_version"], "0.0.0-dev.1+gaaaa");
    assert_eq!(state["channel"], "dev");
    assert_eq!(state["artifact"]["artifact_id"], "tree");
    let release = root.join("versions").join("0.0.0-dev.1+gaaaa");
    assert_eq!(
        std::fs::read(release.join("bin").join(exe("editor"))).unwrap(),
        b"one-editor"
    );
    assert!(release.join("resources").join("build-info.json").is_file());
    assert!(
        !release.join(TREE_MANIFEST_NAME).exists(),
        "metadata is not payload"
    );
    assert!(root.join("bin").join(exe("ae")).is_file(), "permanent ae");
}

#[tokio::test]
async fn a_new_version_reuses_unchanged_files_and_reinstalling_is_idempotent() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("root");
    let first = signed_tree(&root, directory.path(), "0.0.0-dev.1+gaaaa", "one");
    let second = signed_tree(&root, directory.path(), "0.0.0-dev.2+gbbbb", "two");
    let trust = local_trust(&root).unwrap();
    install(options(
        &root,
        ReleaseSource::Tree {
            path: first.clone(),
        },
        trust.clone(),
    ))
    .await
    .unwrap();
    install(options(
        &root,
        ReleaseSource::Tree { path: second },
        trust.clone(),
    ))
    .await
    .unwrap();
    assert_eq!(installation(&root)["active_version"], "0.0.0-dev.2+gbbbb");

    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let inode = |version: &str, stem: &str| {
            std::fs::metadata(
                root.join("versions")
                    .join(version)
                    .join("bin")
                    .join(exe(stem)),
            )
            .unwrap()
            .ino()
        };
        assert_eq!(
            inode("0.0.0-dev.1+gaaaa", "forge"),
            inode("0.0.0-dev.2+gbbbb", "forge"),
            "an unchanged binary is linked from the active version"
        );
        assert_ne!(
            inode("0.0.0-dev.1+gaaaa", "editor"),
            inode("0.0.0-dev.2+gbbbb", "editor")
        );
    }

    // Re-activating an existing identical version is a rollback, not an error.
    install(options(&root, ReleaseSource::Tree { path: first }, trust))
        .await
        .unwrap();
    assert_eq!(installation(&root)["active_version"], "0.0.0-dev.1+gaaaa");
}

#[tokio::test]
async fn a_version_with_different_bytes_is_refused_as_tampered() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("root");
    let first = signed_tree(&root, directory.path(), "0.0.0-dev.1+gaaaa", "one");
    let impostor = signed_tree(&root, directory.path(), "0.0.0-dev.1+gaaaa", "two");
    let trust = local_trust(&root).unwrap();
    install(options(
        &root,
        ReleaseSource::Tree { path: first },
        trust.clone(),
    ))
    .await
    .unwrap();
    let error = install(options(
        &root,
        ReleaseSource::Tree { path: impostor },
        trust,
    ))
    .await
    .unwrap_err();
    assert!(
        matches!(error, InstallerError::TamperedRelease { .. }),
        "{error}"
    );
}

#[tokio::test]
async fn a_file_changed_after_signing_is_refused() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("root");
    let tree = signed_tree(&root, directory.path(), "0.0.0-dev.1+gaaaa", "one");
    std::fs::write(tree.join("bin").join(exe("forge")), b"swapped").unwrap();
    let error = install(options(
        &root,
        ReleaseSource::Tree { path: tree },
        local_trust(&root).unwrap(),
    ))
    .await
    .unwrap_err();
    assert!(error.to_string().contains("signed digest"), "{error}");
    assert!(!root.join("installation.json").exists());
}

#[tokio::test]
async fn another_roots_key_cannot_install_into_this_root() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("root");
    let other = directory.path().join("other");
    LocalSigner::load_or_create(&root).unwrap();
    let foreign = signed_tree(&other, directory.path(), "0.0.0-dev.1+gaaaa", "one");
    let error = install(options(
        &root,
        ReleaseSource::Tree { path: foreign },
        local_trust(&root).unwrap(),
    ))
    .await
    .unwrap_err();
    assert!(matches!(error, InstallerError::InvalidSignature), "{error}");
}

#[tokio::test]
async fn dev_releases_stay_in_dev_installations() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("root");
    let tree = signed_tree(&root, directory.path(), "0.0.0-dev.1+gaaaa", "one");
    let trust = local_trust(&root).unwrap();

    let mut expecting_stable = options(
        &root,
        ReleaseSource::Tree { path: tree.clone() },
        trust.clone(),
    );
    expecting_stable.expected_channel = Some("stable".to_owned());
    assert!(install(expecting_stable).await.is_err());
    assert!(!root.join("installation.json").exists());

    // A root that already holds a nightly installation refuses dev releases.
    let signing = SigningKey::from_bytes(&[9_u8; 32]);
    let Ok(release_trust) =
        TrustKey::resolve(Some(&hex::encode(signing.verifying_key().to_bytes())))
    else {
        return; // release test builds only trust their embedded anchor
    };
    let release = release_directory(directory.path(), &signing, "1.2.3");
    install(options(
        &root,
        ReleaseSource::Directory { path: release },
        release_trust,
    ))
    .await
    .unwrap();
    let error = install(options(&root, ReleaseSource::Tree { path: tree }, trust))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("nightly channel"), "{error}");
    assert_eq!(installation(&root)["active_version"], "1.2.3");
}

#[tokio::test]
async fn release_trust_never_verifies_a_dev_release() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("root");
    let tree = signed_tree(&root, directory.path(), "0.0.0-dev.1+gaaaa", "one");
    let public = serde_json::from_slice::<serde_json::Value>(
        &std::fs::read(root.join("trust").join("local-signing.json")).unwrap(),
    )
    .unwrap()["public_key_hex"]
        .as_str()
        .unwrap()
        .to_owned();
    // A development installer build accepts an explicit key; even the very
    // key that signed the tree does not make a dev release installable
    // without local trust.
    let Ok(explicit) = TrustKey::resolve(Some(&public)) else {
        return; // release test builds refuse explicit keys outright
    };
    let error = install(options(&root, ReleaseSource::Tree { path: tree }, explicit))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("local key"), "{error}");
}

#[test]
fn the_local_key_is_stable_per_root() {
    let directory = tempfile::tempdir().unwrap();
    let first = LocalSigner::load_or_create(directory.path()).unwrap();
    let again = LocalSigner::load_or_create(directory.path()).unwrap();
    assert_eq!(first.key_id(), again.key_id());
    assert!(first.key_id().starts_with("local-"));
    assert!(local_trust(directory.path()).unwrap().is_local());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(directory.path().join("trust").join("local-signing.key"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "the private seed is owner-only");
    }
}

#[test]
fn locations_resolve_to_their_source_kind() {
    let directory = tempfile::tempdir().unwrap();
    let tree = directory.path().join("tree");
    std::fs::create_dir_all(&tree).unwrap();
    std::fs::write(tree.join(TREE_MANIFEST_NAME), b"{}").unwrap();
    let release = directory.path().join("release");
    std::fs::create_dir_all(&release).unwrap();
    std::fs::write(release.join(artisan_install::RELEASE_MANIFEST_NAME), b"{}").unwrap();

    assert!(matches!(
        ReleaseSource::from_location(tree.to_str().unwrap()).unwrap(),
        ReleaseSource::Tree { .. }
    ));
    assert!(matches!(
        ReleaseSource::from_location(release.to_str().unwrap()).unwrap(),
        ReleaseSource::Directory { .. }
    ));
    assert!(matches!(
        ReleaseSource::from_location("https://example.invalid/release-manifest.json").unwrap(),
        ReleaseSource::Remote { signature_url, .. }
            if signature_url.as_str().ends_with("release-manifest.sig")
    ));
    assert!(ReleaseSource::from_location(directory.path().to_str().unwrap()).is_err());
}

/// Builds a release directory like `release-tool` output: a zip artifact
/// and a manifest signed by `signing`.
fn release_directory(directory: &Path, signing: &SigningKey, version: &str) -> PathBuf {
    let platform = Platform::detect().unwrap();
    let output = directory.join("release");
    std::fs::create_dir_all(&output).unwrap();
    let archive_path = output.join("payload.zip");
    let mut entries = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive_path).unwrap());
        let stored = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for stem in ["ae", "installer", "editor", "forge"] {
            let name = format!("bin/{}", exe(stem));
            zip.start_file(&name, stored).unwrap();
            zip.write_all(format!("zip-{stem}").as_bytes()).unwrap();
            entries.push(name);
        }
        zip.finish().unwrap();
    }
    let bytes = std::fs::read(&archive_path).unwrap();
    let key_id = "test-release";
    let manifest = serde_json::to_vec(&serde_json::json!({
        "format_version": 1,
        "product_version": version,
        "editor_forge_compatibility_version": version,
        "channel": "nightly",
        "signing_identity": { "key_id": key_id, "algorithm": "ed25519" },
        "minimum_installer_version": "0.0.0",
        "minimum_cli_version": version,
        "artifacts": [{
            "artifact_id": "native",
            "platform": platform.os,
            "architecture": platform.arch,
            "libc": if platform.os == "linux" { Some("glibc") } else { None },
            "archive_format": "zip",
            "file_name": "payload.zip",
            "byte_size": bytes.len(),
            "sha256": hex::encode(Sha256::digest(&bytes)),
            "archive_entries": entries,
        }],
    }))
    .unwrap();
    let signature = serde_json::to_vec(&serde_json::json!({
        "algorithm": "ed25519",
        "key_id": key_id,
        "signature": STANDARD.encode(signing.sign(&manifest).to_bytes()),
    }))
    .unwrap();
    std::fs::write(
        output.join(artisan_install::RELEASE_MANIFEST_NAME),
        manifest,
    )
    .unwrap();
    std::fs::write(
        output.join(artisan_install::RELEASE_SIGNATURE_NAME),
        signature,
    )
    .unwrap();
    output
}

#[tokio::test]
async fn a_release_directory_installs_without_a_network() {
    let directory = tempfile::tempdir().unwrap();
    let signing = SigningKey::from_bytes(&[7_u8; 32]);
    let public = hex::encode(signing.verifying_key().to_bytes());
    let Ok(trust) = TrustKey::resolve(Some(&public)) else {
        return; // release test builds only trust their embedded anchor
    };
    let release = release_directory(directory.path(), &signing, "1.2.3");
    let root = directory.path().join("root");
    install(options(
        &root,
        ReleaseSource::from_location(release.to_str().unwrap()).unwrap(),
        trust,
    ))
    .await
    .unwrap();
    let state = installation(&root);
    assert_eq!(state["active_version"], "1.2.3");
    assert_eq!(state["channel"], "nightly");
    let files: BTreeMap<String, String> = serde_json::from_slice::<serde_json::Value>(
        &std::fs::read(root.join("versions/1.2.3/payload-manifest.json")).unwrap(),
    )
    .map(|document| serde_json::from_value(document["files"].clone()).unwrap())
    .unwrap();
    assert_eq!(files.len(), 4);
}

#[tokio::test]
async fn pruning_keeps_the_active_version_and_the_most_recent_others() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("root");
    for (index, tag) in ["a", "b", "c", "d"].into_iter().enumerate() {
        let tree = signed_tree(
            &root,
            directory.path(),
            &format!("0.0.0-dev.{index}+g{tag}"),
            tag,
        );
        install(options(
            &root,
            ReleaseSource::Tree { path: tree },
            local_trust(&root).unwrap(),
        ))
        .await
        .unwrap();
        // Distinct install times on coarse-timestamp filesystems.
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    let report = artisan_install::prune(&root, 1).unwrap();
    assert_eq!(report.kept, ["0.0.0-dev.2+gc"]);
    assert_eq!(report.removed, ["0.0.0-dev.1+gb", "0.0.0-dev.0+ga"]);
    assert!(
        root.join("versions/0.0.0-dev.3+gd/bin").is_dir(),
        "active stays"
    );
    assert!(!root.join("versions/0.0.0-dev.0+ga").exists());
    assert_eq!(installation(&root)["active_version"], "0.0.0-dev.3+gd");
}
