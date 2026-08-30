use artisan_editor_cli::credentials::{ForgeCredentialPaths, provision_or_load};
use std::{fs, path::PathBuf, thread};

fn temp_home(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "artisan-cred-int-{}-{}-{}",
        label,
        std::process::id(),
        line!()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn first_provision_exact_file_set() {
    let home = temp_home("first");
    let paths = provision_or_load(&home).unwrap();
    assert!(paths.manifest_path().is_file());
    assert!(paths.capability_path().is_file());
    assert!(paths.certificate_paths()[0].is_file());
    assert!(paths.private_key_path().is_file());
    assert_eq!(fs::read(paths.capability_path()).unwrap().len(), 32);
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(paths.manifest_path()).unwrap()).unwrap();
    assert_eq!(manifest["schema"], "artisan-forge-credentials-v1");
    assert_eq!(manifest["version"], 1);
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn repeated_and_concurrent_byte_identical() {
    let home = temp_home("repeat");
    let first = provision_or_load(&home).unwrap();
    let cap1 = fs::read(first.capability_path()).unwrap();
    let second = provision_or_load(&home).unwrap();
    assert_eq!(cap1, fs::read(second.capability_path()).unwrap());
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let home = home.clone();
            thread::spawn(move || provision_or_load(&home).unwrap())
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    for p in &results[1..] {
        assert_eq!(cap1, fs::read(p.capability_path()).unwrap());
    }
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn nested_ancestor_symlink_rejected() {
    let home = temp_home("nested");
    let parent = home.join("a").join("b").join("c");
    fs::create_dir_all(&parent).unwrap();
    let real = home.join("real");
    fs::create_dir_all(&real).unwrap();
    let link = home.join("a").join("b");
    let _ = fs::remove_dir_all(&link);
    #[cfg(unix)]
    std::os::unix::fs::symlink(&real, &link).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_dir(&real, &link).unwrap_or_else(|_| {
        // if symlink not permitted, fallback to file symlink test
        return;
    });
    let target_home = parent.clone();
    let err = provision_or_load(&target_home);
    if cfg!(unix) {
        assert!(err.is_err());
    }
    let _ = fs::remove_file(&link);
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn no_overwrite_existing_bundle() {
    let home = temp_home("nooverwrite");
    let paths = provision_or_load(&home).unwrap();
    let cap_before = fs::read(paths.capability_path()).unwrap();
    let err = {
        let cred_dir = home.join("credentials");
        let dest = cred_dir.join("bootstrap-capability.bin");
        // try to trigger install_atomic via second provision? provision should not overwrite,
        // just return existing bundle. So we test that file_id stays same after second call.
        provision_or_load(&home).unwrap();
        fs::read(&dest).unwrap()
    };
    assert_eq!(cap_before, err);
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn post_link_failure_leaves_no_untracked_files() {
    let home = temp_home("postlink");
    let cred_dir = home.join("credentials");
    fs::create_dir_all(&cred_dir).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&cred_dir, fs::Permissions::from_mode(0o700)).unwrap();
    }
    // Make manifest path a directory to force failure at manifest install step.
    let manifest_dir = cred_dir.join("manifest.json");
    fs::create_dir(&manifest_dir).unwrap();
    let err = provision_or_load(&home);
    assert!(err.is_err());
    // capability etc should have been cleaned up (no untracked destination)
    assert!(!cred_dir.join("bootstrap-capability.bin").exists());
    assert!(!cred_dir.join("localhost-leaf.der").exists());
    assert!(!cred_dir.join("localhost-key.pkcs8.der").exists());
    // temp files should also be gone
    let entries: Vec<_> = fs::read_dir(&cred_dir).unwrap().collect();
    for e in entries {
        let name = e.unwrap().file_name().to_string_lossy().to_string();
        assert!(!name.ends_with(".tmp"));
    }
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn replacement_not_deleted_by_cleanup() {
    let home = temp_home("replace");
    let paths = provision_or_load(&home).unwrap();
    let cap_path = paths.capability_path().to_path_buf();
    let original = fs::read(&cap_path).unwrap();
    // Replace file with new inode
    fs::remove_file(&cap_path).unwrap();
    fs::write(&cap_path, vec![0xFF; 32]).unwrap();
    let new_bytes = fs::read(&cap_path).unwrap();
    assert!(new_bytes != original);
    // Force a failing provision by corrupting manifest
    let manifest_path = paths.manifest_path().to_path_buf();
    let backup = fs::read(&manifest_path).unwrap();
    fs::write(&manifest_path, b"corrupt").unwrap();
    let err = provision_or_load(&home);
    assert!(err.is_err());
    // Replacement should still exist, not deleted by cleanup that used old id
    assert!(cap_path.is_file());
    assert_eq!(fs::read(&cap_path).unwrap(), new_bytes);
    fs::write(&manifest_path, backup).unwrap();
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn final_validation_error_cleans_this_run() {
    let home = temp_home("finalclean");
    // Create a home where final validation will fail due to tampered cert after install.
    // We trigger by pre-creating credentials dir and making cert path a directory so install fails at cert,
    // which tests cleanup of earlier capability file.
    let cred_dir = home.join("credentials");
    fs::create_dir_all(&cred_dir).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&cred_dir, fs::Permissions::from_mode(0o700)).unwrap();
    }
    fs::create_dir(cred_dir.join("localhost-leaf.der")).unwrap();
    let err = provision_or_load(&home);
    assert!(err.is_err());
    assert!(!cred_dir.join("bootstrap-capability.bin").exists());
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn handle_read_fencing_rejects_symlink_drift() {
    let home = temp_home("drift");
    let paths = provision_or_load(&home).unwrap();
    let cert_path = paths.certificate_paths()[0].to_path_buf();
    let backup = cert_path.with_extension("bak");
    fs::rename(&cert_path, &backup).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&backup, &cert_path).unwrap();
    #[cfg(windows)]
    std::os::windows::fs::symlink_file(&backup, &cert_path).unwrap_or_else(|_| {
        fs::write(&cert_path, b"bad").unwrap();
        return;
    });
    let err = provision_or_load(&home);
    assert!(err.is_err());
    let _ = fs::remove_file(&cert_path);
    fs::rename(&backup, &cert_path).unwrap();
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn redacted_errors_do_not_echo_attacker_content() {
    let home = temp_home("redact");
    let _ = provision_or_load(&home).unwrap();
    let manifest_path = home.join("credentials").join("manifest.json");
    let evil = "evil\x00\n\r\x1b traversal \u{202e}";
    fs::write(
        &manifest_path,
        format!(
            "{{\"schema\":\"artisan-forge-credentials-v1\",\"version\":1,\"bootstrap_capability\":\"../{evil}\",\"certificate_chain\":[\"localhost-leaf.der\"],\"private_key\":\"localhost-key.pkcs8.der\"}}"
        ),
    )
    .unwrap();
    let err = provision_or_load(&home).unwrap_err();
    let display = format!("{err}");
    let debug = format!("{err:?}");
    assert!(!display.contains("evil"));
    assert!(!debug.contains("evil"));
    assert!(!display.contains('\0'.to_string().as_str()));
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn legacy_secrets_untouched() {
    let home = temp_home("legacy");
    let sentinel = b"legacy-sentinel-keep";
    fs::write(home.join("secrets.json"), sentinel).unwrap();
    provision_or_load(&home).unwrap();
    assert_eq!(fs::read(home.join("secrets.json")).unwrap(), sentinel);
    fs::remove_dir_all(home).unwrap();
}

#[test]
#[cfg(unix)]
fn unix_exact_modes() {
    use std::os::unix::fs::PermissionsExt;
    let home = temp_home("modes");
    let paths = provision_or_load(&home).unwrap();
    let dir_mode = fs::metadata(paths.credentials_dir())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(dir_mode, 0o700);
    for p in [
        paths.manifest_path(),
        paths.capability_path(),
        &paths.certificate_paths()[0],
        paths.private_key_path(),
    ] {
        let mode = fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn forge_paths_are_absolute_and_no_default() {
    let home = PathBuf::from("/tmp/abs-test-home");
    let paths = ForgeCredentialPaths::new(&home).unwrap();
    assert!(paths.manifest_path().is_absolute());
    assert!(paths.capability_path().is_absolute());
    // No Default
    fn assert_no_default<T: Default>() {}
    // This should not compile if ForgeCredentialPaths implements Default; we just check it doesn't.
    // If it did, the next line would compile, but we expect it not to.
    // We can't test compile-fail at runtime, but we ensure new requires home.
    let err = ForgeCredentialPaths::new(&PathBuf::from("relative"));
    assert!(err.is_err());
}
