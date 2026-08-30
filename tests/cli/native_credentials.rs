use artisan_editor_cli::credentials::{provision_or_load, ForgeCredentialPaths};
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
    {
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let target_home = parent.clone();
        let err = provision_or_load(&target_home);
        assert!(err.is_err(), "nested symlink must be rejected");
    }
    #[cfg(windows)]
    {
        match std::os::windows::fs::symlink_dir(&real, &link) {
            Ok(()) => {
                let target_home = parent.clone();
                let err = provision_or_load(&target_home);
                assert!(err.is_err(), "nested reparse must be rejected");
                let _ = fs::remove_file(&link);
            }
            Err(_) => {
                eprintln!("SKIP: nested reparse not supported on this Windows host");
            }
        }
    }
    let _ = fs::remove_file(&link);
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn no_overwrite_existing_capability() {
    let home = temp_home("nooverwrite");
    let cred_dir = home.join("credentials");
    fs::create_dir_all(&cred_dir).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&cred_dir, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let cap_path = cred_dir.join("bootstrap-capability.bin");
    fs::write(&cap_path, vec![0xAA; 32]).unwrap();
    let err = provision_or_load(&home);
    assert!(
        err.is_err(),
        "partial bundle must not be silently overwritten"
    );
    assert_eq!(fs::read(&cap_path).unwrap(), vec![0xAA; 32]);
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn manifest_conflict_cleans_prior_atomic_links() {
    let home = temp_home("manifestconflict");
    let cred_dir = home.join("credentials");
    fs::create_dir_all(&cred_dir).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&cred_dir, fs::Permissions::from_mode(0o700)).unwrap();
    }
    fs::create_dir(cred_dir.join("manifest.json")).unwrap();
    let err = provision_or_load(&home);
    assert!(err.is_err());
    assert!(!cred_dir.join("bootstrap-capability.bin").exists());
    assert!(!cred_dir.join("localhost-leaf.der").exists());
    assert!(!cred_dir.join("localhost-key.pkcs8.der").exists());
    let entries: Vec<_> = fs::read_dir(&cred_dir).unwrap().collect();
    for e in entries {
        let name = e.unwrap().file_name().to_string_lossy().to_string();
        assert!(!name.ends_with(".tmp"), "private temp must be removed");
    }
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn leaf_conflict_cleans_capability_and_no_extra_temp() {
    let home = temp_home("leafconflict");
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
    let temps: Vec<_> = fs::read_dir(&cred_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(temps.is_empty());
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn existing_replacement_not_deleted_when_new_bundle_fails() {
    let home = temp_home("keep");
    let cred_dir = home.join("credentials");
    fs::create_dir_all(&cred_dir).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&cred_dir, fs::Permissions::from_mode(0o700)).unwrap();
    }
    // Pre-existing unrelated file that must survive failed provisioning
    let keep = cred_dir.join("keep.txt");
    fs::write(&keep, b"keep").unwrap();
    fs::create_dir(cred_dir.join("manifest.json")).unwrap();
    let err = provision_or_load(&home);
    assert!(err.is_err());
    assert_eq!(fs::read(&keep).unwrap(), b"keep");
    fs::remove_dir_all(home).unwrap();
}

#[test]
fn handle_read_fencing_rejects_symlink_drift() {
    let home = temp_home("drift");
    let paths = provision_or_load(&home).unwrap();
    let cert_path = paths.certificate_paths()[0].to_path_buf();
    let backup = cert_path.with_extension("bak");
    fs::rename(&cert_path, &backup).unwrap();
    let mut symlink_created = false;
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&backup, &cert_path).unwrap();
        symlink_created = true;
    }
    #[cfg(windows)]
    {
        if std::os::windows::fs::symlink_file(&backup, &cert_path).is_ok() {
            symlink_created = true;
        } else {
            eprintln!("SKIP: symlink drift not supported on this Windows host");
            fs::rename(&backup, &cert_path).unwrap();
            fs::remove_dir_all(home).unwrap();
            return;
        }
    }
    if symlink_created {
        let err = provision_or_load(&home);
        assert!(
            err.is_err(),
            "symlink drift must be rejected via handle fencing"
        );
        let _ = fs::remove_file(&cert_path);
        fs::rename(&backup, &cert_path).unwrap();
    }
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
fn forge_paths_require_absolute_home() {
    let home = std::env::temp_dir().join("abs-test-home");
    let paths = ForgeCredentialPaths::new(&home).unwrap();
    assert!(paths.manifest_path().is_absolute());
    assert!(paths.capability_path().is_absolute());
    let err = ForgeCredentialPaths::new(&PathBuf::from("relative"));
    assert!(err.is_err());
}
