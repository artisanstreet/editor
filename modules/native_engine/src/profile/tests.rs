//! Unit tests for profile registry decoding, registration policy, and launch
//! capability verification.
//!
//! Split out of `profile.rs` during the module split.

#![forbid(unsafe_code)]

use std::fs;
use std::path::PathBuf;

use artisan_domain::EngineProfileId;

use crate::engine_core::NativeOpenCode2Authority;
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
use crate::engine_core::{ManagedInstallLock, ManagedInstallLockError, ManagedInstallPaths};
#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
use crate::io::AtomicReplaceOutcome;

use super::registry::{decode_profile_registry, encode_profile_registry, register_in_registry};
use super::types::{EngineProfileRegistration, ProfileRegistry};
use super::*;

#[test]
fn profile_registry_decoding_is_strict_sorted_and_exact() {
    let input = br#"{"engine_id":"opencode2","format_version":1,"profiles":[{"profile_id":"zeta","home":"named"},{"profile_id":"alpha","home":"primary"}]}"#;
    let registry = decode_profile_registry(input).unwrap();
    assert_eq!(registry.profiles.len(), 2);
    assert_eq!(registry.profiles[0].profile_id.as_str(), "alpha");
    assert_eq!(registry.profiles[1].profile_id.as_str(), "zeta");
    assert_eq!(
        encode_profile_registry(&registry).unwrap(),
        br#"{"engine_id":"opencode2","format_version":1,"profiles":[{"profile_id":"alpha","home":"primary"},{"profile_id":"zeta","home":"named"}]}"#
    );
}

#[test]
fn profile_registry_rejects_missing_duplicate_malformed_and_ambiguous_entries() {
    for bytes in [
        br"{}".as_slice(),
        br#"{"engine_id":"opencode2","format_version":1,"profiles":[],"extra":true}"#,
        br#"{"engine_id":"opencode2","engine_id":"opencode2","format_version":1,"profiles":[]}"#,
        br#"{"engine_id":"opencode2","format_version":1,"profiles":[{"profile_id":"a","home":"named","extra":true}]}"#,
        br#"{"engine_id":"opencode2","format_version":1,"profiles":[{"profile_id":"a","profile_id":"b","home":"named"}]}"#,
        br#"{"engine_id":"opencode2","format_version":1,"profiles":[{"profile_id":"a"}]}"#,
        br#"{"engine_id":"opencode2","format_version":1,"profiles":[]} trailing"#,
        br#"{"engine_id":"wrong","format_version":1,"profiles":[]}"#,
        br#"{"engine_id":"opencode2","format_version":2,"profiles":[]}"#,
        br#"{"engine_id":"opencode2","format_version":1,"profiles":[{"profile_id":"a","home":"other"}]}"#,
        br#"{"engine_id":"opencode2","format_version":1,"profiles":[{"profile_id":"a/b","home":"named"}]}"#,
    ] {
        assert!(matches!(
            decode_profile_registry(bytes),
            Err(NativeOpenCode2ProfileError::ProfileRegistryMalformed
                | NativeOpenCode2ProfileError::ProfileRegistryUnsupportedEngine
                | NativeOpenCode2ProfileError::ProfileRegistryUnsupportedVersion)
        ));
    }

    let duplicate_ids = br#"{"engine_id":"opencode2","format_version":1,"profiles":[{"profile_id":"a","home":"named"},{"profile_id":"a","home":"named"}]}"#;
    assert_eq!(
        decode_profile_registry(duplicate_ids),
        Err(NativeOpenCode2ProfileError::DuplicateProfile)
    );
    let multiple_primary = br#"{"engine_id":"opencode2","format_version":1,"profiles":[{"profile_id":"a","home":"primary"},{"profile_id":"b","home":"primary"}]}"#;
    assert_eq!(
        decode_profile_registry(multiple_primary),
        Err(NativeOpenCode2ProfileError::MultiplePrimaryProfiles)
    );
    assert_eq!(
        decode_profile_registry(&vec![b'x'; MAX_PROFILE_REGISTRY_BYTES + 1]),
        Err(NativeOpenCode2ProfileError::ProfileRegistryTooLarge)
    );
}

#[test]
fn profile_registration_policy_is_idempotent_conflict_safe_and_bounded() {
    let id = EngineProfileId::parse("work").unwrap();
    let mut registry = ProfileRegistry {
        profiles: vec![EngineProfileRegistration {
            profile_id: id.clone(),
            home: ProfileHomeKind::Named,
        }],
    };
    let before = encode_profile_registry(&registry).unwrap();
    assert_eq!(
        register_in_registry(&mut registry, &id, ProfileHomeKind::Named),
        Ok(ProfileRegistrationOutcome::AlreadyRegistered)
    );
    assert_eq!(encode_profile_registry(&registry).unwrap(), before);
    assert_eq!(
        register_in_registry(&mut registry, &id, ProfileHomeKind::Primary),
        Err(NativeOpenCode2ProfileError::ProfileConflict)
    );

    let primary = EngineProfileId::parse("primary").unwrap();
    assert_eq!(
        register_in_registry(&mut registry, &primary, ProfileHomeKind::Primary),
        Ok(ProfileRegistrationOutcome::Registered)
    );
    let second_primary = EngineProfileId::parse("second").unwrap();
    assert_eq!(
        register_in_registry(&mut registry, &second_primary, ProfileHomeKind::Primary),
        Err(NativeOpenCode2ProfileError::PrimaryAlreadyRegistered)
    );

    let mut full = ProfileRegistry {
        profiles: Vec::new(),
    };
    for index in 0..MAX_PROFILES {
        full.profiles.push(EngineProfileRegistration {
            profile_id: EngineProfileId::parse(format!("profile-{index:02}")).unwrap(),
            home: ProfileHomeKind::Named,
        });
    }
    let extra = EngineProfileId::parse("extra").unwrap();
    assert_eq!(
        register_in_registry(&mut full, &extra, ProfileHomeKind::Named),
        Err(NativeOpenCode2ProfileError::ProfileLimit)
    );
}

#[test]
fn profile_home_derivation_uses_only_the_certified_engine_root() {
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("data").join("artisan.sqlite");
    std::fs::create_dir(database.parent().unwrap()).unwrap();
    let authority = NativeOpenCode2Authority::new();
    let profile_id = EngineProfileId::parse("work.profile").unwrap();
    assert_eq!(
        authority
            .profile_home_path(&database, &profile_id, ProfileHomeKind::Primary)
            .unwrap(),
        root.path()
            .join("data")
            .join("toolchain")
            .join("opencode2")
            .join("home")
    );
    assert_eq!(
        authority
            .profile_home_path(&database, &profile_id, ProfileHomeKind::Named)
            .unwrap(),
        root.path()
            .join("data")
            .join("toolchain")
            .join("opencode2")
            .join("homes")
            .join("work.profile")
    );
}

#[test]
fn missing_registry_and_default_are_never_discovered_by_read_authority() {
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("artisan.sqlite");
    let authority = NativeOpenCode2Authority::new();
    let registry = authority.profile_registry_path(&database).unwrap();
    assert_eq!(authority.list_profiles(&database).unwrap(), None);
    assert_eq!(
        authority.read_profile(&database, &EngineProfileId::parse("default").unwrap()),
        Err(NativeOpenCode2ProfileError::ProfileNotFound)
    );
    assert!(!registry.exists());
    assert!(!root.path().join("toolchain").exists());
}

#[cfg(unix)]
#[test]
fn profile_home_validation_is_private_and_rejects_unsafe_targets() {
    use crate::io as files;
    use crate::io::NativeFileError;
    use std::os::unix::fs::{PermissionsExt, symlink};

    let root = tempfile::tempdir().unwrap();
    let private = root.path().join("private");
    files::ensure_private_directory(&private).unwrap();
    assert_eq!(
        fs::symlink_metadata(&private).unwrap().permissions().mode() & 0o777,
        0o700
    );

    fs::set_permissions(&private, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        files::validate_private_directory(&private),
        Err(NativeFileError::PrivatePermissions)
    );

    let missing = root.path().join("missing");
    assert_eq!(
        files::validate_private_directory(&missing),
        Err(NativeFileError::NotFound)
    );
    assert!(!missing.exists());

    let target = root.path().join("target");
    fs::create_dir(&target).unwrap();
    let link = root.path().join("link");
    symlink(&target, &link).unwrap();
    assert_eq!(
        files::ensure_private_directory(&link),
        Err(NativeFileError::UnsafePath)
    );

    let file = root.path().join("file");
    fs::write(&file, b"not a directory").unwrap();
    assert_eq!(
        files::ensure_private_directory(&file),
        Err(NativeFileError::UnsafePath)
    );
}

#[test]
fn profile_errors_and_launch_errors_are_path_and_secret_free() {
    let profile_errors = [
        NativeOpenCode2ProfileError::ProfileRegistryTooLarge,
        NativeOpenCode2ProfileError::ProfileRegistryMalformed,
        NativeOpenCode2ProfileError::ProfileRegistryUnsupportedVersion,
        NativeOpenCode2ProfileError::ProfileRegistryUnsupportedEngine,
        NativeOpenCode2ProfileError::ProfileRegistryUnsafe,
        NativeOpenCode2ProfileError::ProfileRegistryUnavailable,
        NativeOpenCode2ProfileError::DuplicateProfile,
        NativeOpenCode2ProfileError::MultiplePrimaryProfiles,
        NativeOpenCode2ProfileError::ProfileNotFound,
        NativeOpenCode2ProfileError::ProfileConflict,
        NativeOpenCode2ProfileError::PrimaryAlreadyRegistered,
        NativeOpenCode2ProfileError::ProfileLimit,
        NativeOpenCode2ProfileError::ProfileHomeUnsafe,
        NativeOpenCode2ProfileError::ProfileHomeUnavailable,
        NativeOpenCode2ProfileError::ProfileAtomicPublishFailed,
        NativeOpenCode2ProfileError::ProfileLockUnavailable,
        NativeOpenCode2ProfileError::CertifiedEngineUnavailable,
    ];
    for error in profile_errors {
        assert!(!error.to_string().contains("C:\\secret"));
        assert!(!format!("{error:?}").contains("profiles.json"));
        assert!(!format!("{error:?}").contains("credential"));
    }
    let launch_errors = [
        NativeOpenCode2ProfileLaunchError::UnsupportedPlatform,
        NativeOpenCode2ProfileLaunchError::ProfileRegistryTooLarge,
        NativeOpenCode2ProfileLaunchError::ProfileRegistryMalformed,
        NativeOpenCode2ProfileLaunchError::ProfileRegistryUnsupportedVersion,
        NativeOpenCode2ProfileLaunchError::ProfileRegistryUnsupportedEngine,
        NativeOpenCode2ProfileLaunchError::ProfileRegistryUnsafe,
        NativeOpenCode2ProfileLaunchError::ProfileRegistryUnavailable,
        NativeOpenCode2ProfileLaunchError::DuplicateProfile,
        NativeOpenCode2ProfileLaunchError::MultiplePrimaryProfiles,
        NativeOpenCode2ProfileLaunchError::ProfileNotFound,
        NativeOpenCode2ProfileLaunchError::ProfileHomeUnsafe,
        NativeOpenCode2ProfileLaunchError::ProfileHomeUnavailable,
        NativeOpenCode2ProfileLaunchError::LockUnavailable,
        NativeOpenCode2ProfileLaunchError::InstallStateMissing,
        NativeOpenCode2ProfileLaunchError::InstallStateInvalid,
        NativeOpenCode2ProfileLaunchError::GenerationUnsafe,
        NativeOpenCode2ProfileLaunchError::GenerationUntrusted,
        NativeOpenCode2ProfileLaunchError::ExecutableUnavailable,
        NativeOpenCode2ProfileLaunchError::ExecutableChanged,
        NativeOpenCode2ProfileLaunchError::ExecutableSizeMismatch,
        NativeOpenCode2ProfileLaunchError::ExecutableHashMismatch,
        NativeOpenCode2ProfileLaunchError::ProfileChanged,
    ];
    for error in launch_errors {
        assert!(!error.to_string().contains("C:\\secret"));
        assert!(!format!("{error:?}").contains("profiles.json"));
        assert!(!format!("{error:?}").contains("OPENCODE_PASSWORD"));
    }
}

#[test]
fn launch_error_cli_reason_taxonomy_is_exhaustive() {
    let reasons = [
        (
            NativeOpenCode2ProfileLaunchError::UnsupportedPlatform,
            "unsupported_platform",
        ),
        (
            NativeOpenCode2ProfileLaunchError::ProfileRegistryTooLarge,
            "profile_registry_invalid",
        ),
        (
            NativeOpenCode2ProfileLaunchError::ProfileRegistryMalformed,
            "profile_registry_invalid",
        ),
        (
            NativeOpenCode2ProfileLaunchError::ProfileRegistryUnsupportedVersion,
            "profile_registry_invalid",
        ),
        (
            NativeOpenCode2ProfileLaunchError::ProfileRegistryUnsupportedEngine,
            "profile_registry_invalid",
        ),
        (
            NativeOpenCode2ProfileLaunchError::ProfileRegistryUnsafe,
            "profile_registry_invalid",
        ),
        (
            NativeOpenCode2ProfileLaunchError::ProfileRegistryUnavailable,
            "profile_registry_invalid",
        ),
        (
            NativeOpenCode2ProfileLaunchError::DuplicateProfile,
            "profile_registry_invalid",
        ),
        (
            NativeOpenCode2ProfileLaunchError::MultiplePrimaryProfiles,
            "profile_registry_invalid",
        ),
        (
            NativeOpenCode2ProfileLaunchError::ProfileNotFound,
            "profile_not_found",
        ),
        (
            NativeOpenCode2ProfileLaunchError::ProfileHomeUnsafe,
            "profile_home_unsafe",
        ),
        (
            NativeOpenCode2ProfileLaunchError::ProfileHomeUnavailable,
            "profile_home_unavailable",
        ),
        (
            NativeOpenCode2ProfileLaunchError::LockUnavailable,
            "profile_lock_unavailable",
        ),
        (
            NativeOpenCode2ProfileLaunchError::InstallStateMissing,
            "install_state_missing",
        ),
        (
            NativeOpenCode2ProfileLaunchError::InstallStateInvalid,
            "install_state_invalid",
        ),
        (
            NativeOpenCode2ProfileLaunchError::GenerationUnsafe,
            "generation_unsafe",
        ),
        (
            NativeOpenCode2ProfileLaunchError::GenerationUntrusted,
            "generation_untrusted",
        ),
        (
            NativeOpenCode2ProfileLaunchError::ExecutableUnavailable,
            "executable_unavailable",
        ),
        (
            NativeOpenCode2ProfileLaunchError::ExecutableChanged,
            "executable_changed",
        ),
        (
            NativeOpenCode2ProfileLaunchError::ExecutableSizeMismatch,
            "executable_size_mismatch",
        ),
        (
            NativeOpenCode2ProfileLaunchError::ExecutableHashMismatch,
            "executable_hash_mismatch",
        ),
        (
            NativeOpenCode2ProfileLaunchError::ProfileChanged,
            "profile_changed",
        ),
    ];
    for (error, expected) in reasons {
        assert_eq!(
            error.cli_reason(),
            expected,
            "unexpected reason for {error:?}"
        );
    }
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
#[test]
fn exact_default_and_primary_named_profile_resolution() {
    let (_root, database, authority, paths) = installed_fixture();
    let default_id = EngineProfileId::parse("default").unwrap();
    let primary_id = EngineProfileId::parse("work").unwrap();
    assert_eq!(
        authority.register_profile(&database, &default_id, ProfileHomeKind::Named),
        Ok(ProfileRegistrationOutcome::Registered)
    );
    assert_eq!(
        authority.register_profile(&database, &primary_id, ProfileHomeKind::Primary),
        Ok(ProfileRegistrationOutcome::Registered)
    );

    let named = authority
        .resolve_profile_launch(&database, &default_id)
        .unwrap();
    assert_eq!(named.profile_id(), &default_id);
    assert_eq!(named.home(), ProfileHomeKind::Named);
    assert_eq!(
        named.profile_home(),
        paths.engine_root().join("homes").join("default")
    );
    assert_eq!(named.generation_id(), generation_id());
    assert_eq!(named.version(), "0.0.0-beta-17778");
    assert_eq!(named.executable_path(), test_executable(&paths));
    drop(named);
    assert!(matches!(
        authority.resolve_profile_launch(&database, &EngineProfileId::parse("missing").unwrap()),
        Err(NativeOpenCode2ProfileLaunchError::ProfileNotFound)
    ));

    let primary = authority
        .resolve_profile_launch(&database, &primary_id)
        .unwrap();
    assert_eq!(primary.home(), ProfileHomeKind::Primary);
    assert_eq!(primary.profile_home(), paths.engine_root().join("home"));
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
#[test]
fn launch_rejects_missing_duplicate_and_malformed_registries() {
    let (_root, database, authority, paths) = installed_fixture();
    let id = EngineProfileId::parse("default").unwrap();
    assert!(matches!(
        authority.resolve_profile_launch(&database, &id),
        Err(NativeOpenCode2ProfileLaunchError::ProfileNotFound)
    ));
    let registry = paths.engine_root().join("profiles.json");
    fs::write(
        &registry,
        br#"{"engine_id":"opencode2","format_version":1,"profiles":[{"profile_id":"default","home":"named"},{"profile_id":"default","home":"named"}]}"#,
    )
    .unwrap();
    assert!(matches!(
        authority.resolve_profile_launch(&database, &id),
        Err(NativeOpenCode2ProfileLaunchError::DuplicateProfile)
    ));
    fs::write(
        &registry,
        br#"{"engine_id":"opencode2","format_version":1,"profiles":[{"profile_id":"default","home":"other"}]}"#,
    )
    .unwrap();
    assert!(matches!(
        authority.resolve_profile_launch(&database, &id),
        Err(NativeOpenCode2ProfileLaunchError::ProfileRegistryMalformed)
    ));
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
#[test]
fn generation_replacement_and_executable_identity_drift_fail_revalidation() {
    let (_root, database, authority, paths) = installed_fixture();
    let id = EngineProfileId::parse("default").unwrap();
    authority
        .register_profile(&database, &id, ProfileHomeKind::Named)
        .unwrap();
    let launch = authority.resolve_profile_launch(&database, &id).unwrap();
    let replacement_id = "generation-fedcba9876543210fedcba9876543210";
    write_generation(&paths, replacement_id, b"test executable");
    let state = fixture_state(replacement_id);
    assert_eq!(
        authority
            .managed()
            .write_install_state(paths.engine_root(), &state),
        Ok(AtomicReplaceOutcome::Committed)
    );
    assert_eq!(
        launch.revalidate(),
        Err(NativeOpenCode2ProfileLaunchError::ProfileChanged)
    );
    drop(launch);
    let replacement = authority.resolve_profile_launch(&database, &id).unwrap();
    assert_eq!(replacement.generation_id(), replacement_id);

    let executable = executable_for(&paths, replacement_id);
    fs::remove_file(&executable).unwrap();
    fs::write(&executable, b"test executable").unwrap();
    assert_eq!(
        replacement.revalidate(),
        Err(NativeOpenCode2ProfileLaunchError::ProfileChanged)
    );
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
#[test]
fn executable_size_and_hash_failures_are_distinct_and_lock_is_retained() {
    let (_root, database, authority, paths) = installed_fixture();
    let id = EngineProfileId::parse("default").unwrap();
    authority
        .register_profile(&database, &id, ProfileHomeKind::Named)
        .unwrap();
    let executable = test_executable(&paths);
    fs::write(&executable, b"wrong").unwrap();
    assert!(matches!(
        authority.resolve_profile_launch(&database, &id),
        Err(NativeOpenCode2ProfileLaunchError::ExecutableSizeMismatch)
    ));
    fs::write(&executable, b"wrong content!!").unwrap();
    assert!(matches!(
        authority.resolve_profile_launch(&database, &id),
        Err(NativeOpenCode2ProfileLaunchError::ExecutableHashMismatch)
    ));
    fs::write(&executable, b"test executable").unwrap();
    let launch = authority.resolve_profile_launch(&database, &id).unwrap();
    assert!(matches!(
        ManagedInstallLock::try_acquire(&paths),
        Err(ManagedInstallLockError::Busy)
    ));
    assert!(!format!("{launch:?}").contains(&database.to_string_lossy().to_string()));
    assert_eq!(
        launch.to_string(),
        "verified OpenCode2 profile launch capability"
    );
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
fn installed_fixture() -> (
    tempfile::TempDir,
    PathBuf,
    NativeOpenCode2Authority,
    ManagedInstallPaths,
) {
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("artisan.sqlite");
    let authority = NativeOpenCode2Authority::new();
    let paths = authority.install_paths(&database).unwrap();
    paths.prepare().unwrap();
    write_generation(&paths, generation_id(), b"test executable");
    let state = fixture_state(generation_id());
    assert_eq!(
        authority
            .managed()
            .write_install_state(paths.engine_root(), &state),
        Ok(AtomicReplaceOutcome::Committed)
    );
    (root, database, authority, paths)
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
fn generation_id() -> &'static str {
    "generation-0123456789abcdef0123456789abcdef"
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
fn test_executable(paths: &ManagedInstallPaths) -> PathBuf {
    executable_for(paths, generation_id())
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
fn executable_for(paths: &ManagedInstallPaths, id: &str) -> PathBuf {
    paths.versions_root().join(id).join("opencode2.exe")
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
fn write_generation(paths: &ManagedInstallPaths, id: &str, bytes: &[u8]) {
    let generation = paths.versions_root().join(id);
    fs::create_dir_all(&generation).unwrap();
    fs::write(generation.join("opencode2.exe"), bytes).unwrap();
}

#[cfg(all(target_os = "windows", target_arch = "x86_64"))]
fn fixture_state(directory: &str) -> crate::engine_core::ManagedToolchainState {
    use sha2::{Digest, Sha256};

    crate::engine_core::ManagedToolchainState::new(crate::engine_core::ManagedGeneration {
        binary: "opencode2.exe".into(),
        directory: directory.into(),
        sha256: Sha256::digest(b"test executable")
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        size: Some(15),
        version: "0.0.0-beta-17778".into(),
    })
}
