use std::{
    fs, io,
    path::{Path, PathBuf},
};

use super::state::{
    MAX_BINARY_PATH_BYTES, MAX_GENERATION_ID_BYTES, MAX_STATE_BYTES, decode_state,
    is_safe_basename, is_safe_relative_path, is_safe_sha256, validate_generation,
    validate_state_version,
};
use super::*;

const DIGEST: &str = "452794a764e1033e629c4cd40bde6433c10c6bd32433fb3be279bf03969a6edf";

fn opencode2() -> ManagedEngineAuthority {
    ManagedEngineAuthority::for_platform(ManagedEngine::OpenCode2, HostPlatform::WindowsX64)
}

fn generation(directory: &str, version: &str) -> ManagedGeneration {
    ManagedGeneration {
        binary: "opencode2.exe".into(),
        directory: directory.into(),
        sha256: DIGEST.into(),
        size: Some(144_313_344),
        version: version.into(),
    }
}

fn database_path(root: &Path) -> PathBuf {
    root.join("artisan.sqlite")
}

#[test]
fn lock_contention_predicate_uses_canonical_identity_and_fails_closed() {
    assert!(spec::is_lock_contended(&fs2::lock_contended_error()));
    assert!(spec::is_lock_contended(&io::Error::from(
        io::ErrorKind::WouldBlock
    )));
    assert!(!spec::is_lock_contended(&io::Error::from(
        io::ErrorKind::Other
    )));
}

#[test]
fn format_1_state_from_the_original_opencode2_install_is_still_accepted() {
    let v1 = format!(
        r#"{{"active":{{"binary":"opencode2.exe","directory":"generation-a","sha256":"{DIGEST}","version":"0.0.0-beta-17778"}},"format_version":1}}"#
    );
    let state = decode_state(v1.as_bytes()).unwrap();
    assert_eq!(state.format_version, 1);
    assert_eq!(state.active.size, None);
    assert!(state.previous.is_empty());
    let root = tempfile::tempdir().unwrap();
    let engine_root = root.path().join("toolchain").join("opencode2");
    fs::create_dir_all(&engine_root).unwrap();
    fs::write(engine_root.join("state.json"), &v1).unwrap();
    let read = opencode2()
        .read_install_state(&engine_root)
        .unwrap()
        .unwrap();
    let upgraded = String::from_utf8(opencode2().encode_install_state(&read).unwrap()).unwrap();
    assert!(upgraded.contains(r#""format_version":1"#));
    let next = read.activated(generation("generation-b", "0.0.0-beta-19271"));
    let encoded = String::from_utf8(opencode2().encode_install_state(&next).unwrap()).unwrap();
    assert!(encoded.contains(r#""format_version":2"#));
    assert!(encoded.contains(r#""previous":[{"#));
}

#[test]
fn state_decoder_rejects_malformed_duplicate_unknown_trailing_and_null_values() {
    let active = format!(
        r#"{{"binary":"opencode2.exe","directory":"generation-a","sha256":"{DIGEST}","version":"0.0.0-beta-17778"}}"#
    );
    for malformed in [
        r#"{"active":{}}"#.to_owned(),
        format!(r#"{{"active":{active},"format_version":2,"extra":true}}"#),
        format!(r#"{{"active":{active},"active":{active},"format_version":2}}"#),
        format!(r#"{{"active":{active},"format_version":2}} trailing"#),
        format!(r#"{{"active":{active},"format_version":1,"previous":null}}"#),
        format!(r#"{{"active":{active},"format_version":2,"pending":null}}"#),
        format!(r#"{{"active":{active},"format_version":2,"previous":{active}}}"#),
    ] {
        assert!(
            matches!(
                decode_state(malformed.as_bytes()),
                Err(ManagedEngineError::StateMalformed)
            ),
            "{malformed}"
        );
    }
    assert!(matches!(
        decode_state(&[0xff, 0xfe]),
        Err(ManagedEngineError::StateMalformed)
    ));
    assert!(matches!(
        decode_state(&vec![b' '; MAX_STATE_BYTES + 1]),
        Err(ManagedEngineError::StateTooLarge)
    ));
    let future = format!(r#"{{"active":{active},"format_version":3}}"#);
    let state = decode_state(future.as_bytes()).unwrap();
    assert_eq!(
        validate_state_version(&state),
        Err(ManagedEngineError::StateUnsupportedVersion)
    );
}

#[test]
fn activation_keeps_three_previous_generations_most_recent_first() {
    let mut state = ManagedToolchainState::new(generation("generation-0", "0.0.0-beta-17778"));
    for index in 1..=5 {
        state = state.activated(generation(
            &format!("generation-{index}"),
            &format!("0.0.0-beta-1800{index}"),
        ));
    }
    assert_eq!(state.active.directory, "generation-5");
    let previous: Vec<&str> = state
        .previous
        .iter()
        .map(|g| g.directory.as_str())
        .collect();
    assert_eq!(previous, ["generation-4", "generation-3", "generation-2"]);
    let back = state.activated(state.previous[0].clone());
    let previous: Vec<&str> = back.previous.iter().map(|g| g.directory.as_str()).collect();
    assert_eq!(back.active.directory, "generation-4");
    assert_eq!(previous, ["generation-5", "generation-3", "generation-2"]);
    let pending = back.with_pending(generation("generation-6", "0.0.0-beta-18006"));
    assert_eq!(pending.active.directory, "generation-4");
    assert_eq!(pending.directories().count(), 5);
    assert!(pending.generation_for("0.0.0-beta-18006").is_some());
}

#[test]
fn state_validation_rejects_unsafe_untrusted_and_below_floor_generations() {
    assert!(!is_safe_basename("../generation", MAX_GENERATION_ID_BYTES));
    for unsafe_path in [
        "../opencode2.exe",
        "C:\\opencode2.exe",
        "nested//opencode2.exe",
    ] {
        assert!(!is_safe_relative_path(unsafe_path, MAX_BINARY_PATH_BYTES));
    }
    assert!(!is_safe_sha256(&"A".repeat(64)));
    let check = |generation: &ManagedGeneration, launchable| {
        validate_generation(
            generation,
            launchable,
            ManagedEngine::OpenCode2,
            HostPlatform::WindowsX64,
        )
    };
    let mut candidate = generation("generation-a", "0.0.0-beta-17778");
    assert_eq!(check(&candidate, true), Ok(()));
    candidate.binary = "nested/opencode2.exe".into();
    assert_eq!(
        check(&candidate, true),
        Err(ManagedEngineError::ActiveGenerationUntrusted)
    );
    candidate = generation("../generation-a", "0.0.0-beta-17778");
    assert_eq!(check(&candidate, true), Err(ManagedEngineError::UnsafePath));
    candidate = generation("generation-a", "0.0");
    assert_eq!(
        check(&candidate, true),
        Err(ManagedEngineError::ActiveGenerationUntrusted)
    );
    candidate = generation("generation-a", "0.0.0-beta-100");
    assert_eq!(
        check(&candidate, true),
        Err(ManagedEngineError::ActiveGenerationUntrusted)
    );
    assert_eq!(check(&candidate, false), Ok(()));
    candidate = generation("generation-a", "0.0.0-beta-17778");
    candidate.size = Some(0);
    assert_eq!(
        check(&candidate, true),
        Err(ManagedEngineError::ActiveGenerationUntrusted)
    );

    let duplicate = ManagedToolchainState::new(generation("generation-a", "0.0.0-beta-17778"))
        .with_pending(generation("generation-a", "0.0.0-beta-19271"));
    assert_eq!(
        opencode2().encode_install_state(&duplicate),
        Err(ManagedStateError::Malformed)
    );
}

#[test]
fn state_round_trips_without_null_optionals() {
    let root = tempfile::tempdir().unwrap();
    let engine_root = root.path().join("toolchain").join("opencode2");
    fs::create_dir_all(&engine_root).unwrap();
    let first = ManagedToolchainState::new(generation("generation-a", "0.0.0-beta-17778"));
    let bytes = opencode2().encode_install_state(&first).unwrap();
    let text = String::from_utf8(bytes.clone()).unwrap();
    assert!(!text.contains("previous") && !text.contains("pending") && !text.contains("null"));
    let _committed = opencode2()
        .write_install_state(&engine_root, &first)
        .unwrap();
    assert_eq!(fs::read_dir(&engine_root).unwrap().count(), 1);
    let read = opencode2()
        .read_install_state(&engine_root)
        .unwrap()
        .unwrap();
    assert_eq!(opencode2().encode_install_state(&read).unwrap(), bytes);
}

#[test]
fn paths_use_the_exact_database_parent_and_reject_traversal() {
    let root = tempfile::tempdir().unwrap();
    let database_parent = root.path().join("data");
    fs::create_dir(&database_parent).unwrap();
    let paths =
        ManagedInstallPaths::derive(&database_parent.join("forge.db"), ManagedEngine::Claude)
            .unwrap();
    let engine_root = database_parent.join("toolchain").join("claude");
    assert_eq!(paths.engine_root(), engine_root);
    assert_eq!(paths.versions_root(), engine_root.join("versions"));
    assert_eq!(paths.lock_path(), engine_root.join("install.lock"));
    assert_eq!(paths.use_lock_path(), engine_root.join("use.lock"));
    let traversal = root.path().join("data/../other/db.sqlite");
    assert!(matches!(
        ManagedInstallPaths::derive(&traversal, ManagedEngine::Claude),
        Err(ManagedInstallPathError::InvalidRoot)
    ));
    assert!(matches!(
        ManagedInstallPaths::derive(Path::new("relative.db"), ManagedEngine::Claude),
        Err(ManagedInstallPathError::InvalidRoot)
    ));
}

#[test]
fn missing_state_is_not_installed_and_other_engine_roots_are_never_read() {
    let root = tempfile::tempdir().unwrap();
    let database = database_path(root.path());
    let other = root.path().join("toolchain").join("claude");
    fs::create_dir_all(&other).unwrap();
    fs::write(other.join("state.json"), b"not codex state").unwrap();
    let codex = ManagedEngineAuthority::for_platform(ManagedEngine::Codex, HostPlatform::LinuxX64);
    let paths = codex.install_paths(&database).unwrap();
    assert!(matches!(
        codex.read_install_state(paths.engine_root()),
        Ok(None)
    ));
    assert!(matches!(
        codex.read_install_state(&other),
        Err(ManagedStateError::InvalidRoot)
    ));
    assert!(matches!(
        codex.inspect(&database),
        Ok(EngineInspection::NotInstalled)
    ));
    assert!(matches!(
        codex.read_install_state(Path::new("toolchain/codex")),
        Err(ManagedStateError::InvalidRoot)
    ));
}

#[test]
fn bounded_state_reader_rejects_oversized_documents_without_changing_them() {
    let root = tempfile::tempdir().unwrap();
    let engine_root = root.path().join("toolchain").join("opencode2");
    fs::create_dir_all(&engine_root).unwrap();
    let state_path = engine_root.join("state.json");
    fs::write(&state_path, vec![b'x'; MAX_STATE_BYTES + 1]).unwrap();
    assert!(matches!(
        opencode2().read_install_state(&engine_root),
        Err(ManagedStateError::TooLarge)
    ));
    assert_eq!(fs::read(&state_path).unwrap().len(), MAX_STATE_BYTES + 1);
}

#[cfg(unix)]
#[test]
fn state_and_ancestor_symlinks_are_rejected() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let engine_root = root.path().join("toolchain").join("opencode2");
    fs::create_dir_all(&engine_root).unwrap();
    let real_state = root.path().join("real-state.json");
    fs::write(&real_state, b"{}").unwrap();
    symlink(&real_state, engine_root.join("state.json")).unwrap();
    assert!(matches!(
        opencode2().read_install_state(&engine_root),
        Err(ManagedStateError::UnsafePath)
    ));

    let data = root.path().join("data");
    let real_data = root.path().join("real-data");
    fs::create_dir_all(real_data.join("toolchain").join("opencode2")).unwrap();
    symlink(&real_data, &data).unwrap();
    let linked_root = data.join("toolchain").join("opencode2");
    fs::write(linked_root.join("state.json"), b"{}").unwrap();
    assert!(matches!(
        opencode2().read_install_state(&linked_root),
        Err(ManagedStateError::UnsafePath)
    ));
}

#[test]
fn errors_are_payload_free() {
    for error in [
        ManagedEngineError::UnsupportedPlatform,
        ManagedEngineError::StateMissing,
        ManagedEngineError::ExecutableHashMismatch,
        ManagedEngineError::Io,
    ] {
        assert!(!error.to_string().contains("C:\\"));
        assert!(!format!("{error:?}").contains("toolchain"));
    }
    assert!(
        !InstallError::IntegrityMismatch
            .to_string()
            .contains("https")
    );
}
