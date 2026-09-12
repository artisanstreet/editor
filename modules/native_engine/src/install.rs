//! Certified native `OpenCode2` installation authority.
//!
//! This file keeps the crate-facing re-exports and the module's behavior
//! tests, split into three private children:
//!
//! - `spec` owns the certified artifact identity, validated installation
//!   paths, and the exclusive install lock.
//! - `state` owns the bounded install-state codec, validation, and atomic
//!   publication.
//! - `authority` owns inspection, active-generation resolution, and the
//!   executable verification seam.

#[path = "install/authority.rs"]
mod authority;
#[path = "install/spec.rs"]
mod spec;
#[path = "install/state.rs"]
mod state;

#[allow(clippy::wildcard_imports)]
pub use authority::*;

#[allow(clippy::wildcard_imports)]
pub use spec::*;

#[allow(clippy::wildcard_imports)]
pub use state::*;

#[cfg(test)]
use {
    crate::io as native_files,
    crate::io::{AtomicReplaceOutcome, NativeFileError},
    std::{
        fs, io,
        path::{Path, PathBuf},
    },
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_contention_predicate_uses_canonical_identity_and_fails_closed() {
        assert!(is_lock_contended(&fs2::lock_contended_error()));
        assert!(is_lock_contended(&io::Error::from(
            io::ErrorKind::WouldBlock
        )));
        assert!(!is_lock_contended(&io::Error::from(io::ErrorKind::Other)));
    }

    #[test]
    fn certified_install_spec_is_fixed_and_diagnostic_free() {
        let authority = NativeOpenCode2Authority::new();
        let spec = NativeOpenCode2Authority::certified_install_spec();
        assert_eq!(spec.engine_id(), "opencode2");
        assert_eq!(spec.version(), "0.0.0-beta-17778");
        assert_eq!(
            spec.upstream_commit(),
            "0d2684b67308380fc47540fe55deb55306a08e3f"
        );
        assert_eq!(spec.platform(), "win32");
        assert_eq!(spec.architecture(), "x64");
        assert_eq!(spec.artifact_kind(), "npm-tarball");
        assert_eq!(spec.archive_member(), "package/bin/opencode2.exe");
        assert_eq!(spec.binary(), "opencode2.exe");
        assert_eq!(spec.download_bound_bytes(), 268_435_456);
        assert_eq!(spec.executable_size_bytes(), 144_313_344);
        assert_eq!(
            spec.executable_sha256_hex(),
            "452794a764e1033e629c4cd40bde6433c10c6bd32433fb3be279bf03969a6edf"
        );
        assert_eq!(
            spec.npm_integrity_sha512(),
            "Z0oMvTBUhxmz1IYuQSMOZTpI2HoWjeIjdxJ39SoGrhDwvJZK7OI0rgIMYtDGavOucOQT8oxrazUiO4j+2hVMpw=="
        );
        assert_eq!(
            spec.npm_url(),
            "https://registry.npmjs.org/@opencode-ai/cli-windows-x64/-/cli-windows-x64-0.0.0-beta-17778.tgz"
        );
        assert_eq!(
            spec.executable_sha256(),
            &[
                0x45, 0x27, 0x94, 0xa7, 0x64, 0xe1, 0x03, 0x3e, 0x62, 0x9c, 0x4c, 0xd4, 0x0b, 0xde,
                0x64, 0x33, 0xc1, 0x0c, 0x6b, 0xd3, 0x24, 0x33, 0xfb, 0x3b, 0xe2, 0x79, 0xbf, 0x03,
                0x96, 0x9a, 0x6e, 0xdf,
            ]
        );
        assert_eq!(authority.spec().engine_id(), spec.engine_id());
        assert!(!format!("{authority:?}").contains("registry.npmjs.org"));
        assert!(!format!("{spec:?}").contains("opencode2.exe"));
    }

    #[test]
    fn install_state_codec_rejects_syntax_errors_and_full_validation_rejects_unsafe_values() {
        let active = r#"{"binary":"opencode2.exe","directory":"generation-a","sha256":"452794a764e1033e629c4cd40bde6433c10c6bd32433fb3be279bf03969a6edf","version":"0.0.0-beta-17778"}"#;
        let valid = format!(r#"{{"active":{active},"format_version":1}}"#);
        assert!(decode_state(valid.as_bytes()).is_ok());
        for malformed in [
            format!(r#"{{"active":{active},"format_version":1,"extra":true}}"#),
            format!(r#"{{"active":{active},"active":{active},"format_version":1}}"#),
            format!(r#"{{"active":{active},"format_version":1}} trailing"#),
        ] {
            assert!(matches!(
                decode_state(malformed.as_bytes()),
                Err(NativeOpenCode2Error::StateMalformed)
            ));
        }

        let authority = NativeOpenCode2Authority::new();
        let root = tempfile::tempdir().unwrap();
        let engine_root = root.path().join("toolchain").join("opencode2");
        fs::create_dir_all(&engine_root).unwrap();
        let state_path = engine_root.join("state.json");

        fs::write(
            &state_path,
            format!(r#"{{"active":{active},"format_version":2}}"#),
        )
        .unwrap();
        assert!(matches!(
            authority.read_install_state(&engine_root),
            Err(NativeOpenCode2StateError::UnsupportedVersion)
        ));

        fs::write(
            &state_path,
            r#"{"active":{"binary":"opencode2.exe","directory":"../escape","sha256":"452794a764e1033e629c4cd40bde6433c10c6bd32433fb3be279bf03969a6edf","version":"0.0.0-beta-17778"},"format_version":1}"#,
        )
        .unwrap();
        assert!(matches!(
            authority.read_install_state(&engine_root),
            Err(NativeOpenCode2StateError::UnsafePath)
        ));
    }

    #[test]
    fn state_decoder_accepts_active_with_or_without_previous() {
        let active = r#"{"binary":"opencode2.exe","directory":"generation-a","sha256":"452794a764e1033e629c4cd40bde6433c10c6bd32433fb3be279bf03969a6edf","version":"0.0.0-beta-17778"}"#;
        let without_previous = format!(r#"{{"active":{active},"format_version":1}}"#);
        let state = decode_state(without_previous.as_bytes()).unwrap();
        assert_eq!(state.format_version, 1);
        assert!(state.previous.is_none());
        assert_eq!(state.active.directory, "generation-a");

        let previous = r#"{"binary":"old.exe","directory":"generation-old","sha256":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","version":"1.2.3"}"#;
        let with_previous =
            format!(r#"{{"active":{active},"format_version":1,"previous":{previous}}}"#);
        let state = decode_state(with_previous.as_bytes()).unwrap();
        assert_eq!(state.previous.as_ref().unwrap().directory, "generation-old");
        let spec = NativeOpenCode2Authority::certified_install_spec();
        validate_generation(&state.active, true, &spec).unwrap();
        validate_generation(state.previous.as_ref().unwrap(), false, &spec).unwrap();
    }

    #[test]
    fn state_decoder_rejects_malformed_duplicate_unknown_trailing_and_null_previous() {
        let valid = r#"{"active":{"binary":"opencode2.exe","directory":"generation-a","sha256":"452794a764e1033e629c4cd40bde6433c10c6bd32433fb3be279bf03969a6edf","version":"0.0.0-beta-17778"},"format_version":1}"#;
        for malformed in [
            r#"{"active":{}}"#,
            r#"{"active":{"binary":"opencode2.exe","directory":"generation-a","sha256":"452794a764e1033e629c4cd40bde6433c10c6bd32433fb3be279bf03969a6edf","version":"0.0.0-beta-17778"},"format_version":1,"extra":true}"#,
            r#"{"active":{"binary":"opencode2.exe","directory":"generation-a","sha256":"452794a764e1033e629c4cd40bde6433c10c6bd32433fb3be279bf03969a6edf","version":"0.0.0-beta-17778"},"format_version":1,"format_version":1}"#,
            r#"{"active":{"binary":"opencode2.exe","directory":"generation-a","sha256":"452794a764e1033e629c4cd40bde6433c10c6bd32433fb3be279bf03969a6edf","version":"0.0.0-beta-17778"},"format_version":1} trailing"#,
            r#"{"active":{"binary":"opencode2.exe","directory":"generation-a","sha256":"452794a764e1033e629c4cd40bde6433c10c6bd32433fb3be279bf03969a6edf","version":"0.0.0-beta-17778"},"format_version":1,"previous":null}"#,
        ] {
            assert!(matches!(
                decode_state(malformed.as_bytes()),
                Err(NativeOpenCode2Error::StateMalformed)
            ));
        }
        assert!(decode_state(valid.as_bytes()).is_ok());
        assert!(matches!(
            decode_state(&[0xff, 0xfe]),
            Err(NativeOpenCode2Error::StateMalformed)
        ));
    }

    #[test]
    fn state_decoder_rejects_unsupported_format_and_oversized_bytes() {
        let valid = r#"{"active":{"binary":"opencode2.exe","directory":"generation-a","sha256":"452794a764e1033e629c4cd40bde6433c10c6bd32433fb3be279bf03969a6edf","version":"0.0.0-beta-17778"},"format_version":2}"#;
        let state = decode_state(valid.as_bytes()).unwrap();
        assert_eq!(state.format_version, 2);
        assert_eq!(
            validate_state_version(&state),
            Err(NativeOpenCode2Error::StateUnsupportedVersion)
        );
        assert!(matches!(
            decode_state(&vec![b' '; MAX_STATE_BYTES + 1]),
            Err(NativeOpenCode2Error::StateTooLarge)
        ));
    }

    #[test]
    fn install_state_constructor_retains_only_the_previous_active_generation() {
        let authority = NativeOpenCode2Authority::new();
        let first = authority.new_install_state("generation-a", None).unwrap();
        let second = authority
            .new_install_state("generation-b", Some(&first))
            .unwrap();

        assert_eq!(second.inner.active.directory, "generation-b");
        assert_eq!(
            second.inner.previous.as_ref().unwrap().directory,
            "generation-a"
        );
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn install_state_round_trips_without_serializing_null_previous() {
        let authority = NativeOpenCode2Authority::new();
        let root = tempfile::tempdir().unwrap();
        let engine_root = root.path().join("toolchain").join("opencode2");
        fs::create_dir_all(&engine_root).unwrap();

        let first = authority.new_install_state("generation-a", None).unwrap();
        let first_bytes = authority.encode_install_state(&first).unwrap();
        let first_text = String::from_utf8(first_bytes.clone()).unwrap();
        assert!(!first_text.contains("previous"));
        assert!(!first_text.contains("null"));
        assert!(matches!(
            authority.write_install_state(&engine_root, &first),
            Ok(AtomicReplaceOutcome::Committed)
        ));
        assert_eq!(fs::read_dir(&engine_root).unwrap().count(), 1);

        let first_read = authority.read_install_state(&engine_root).unwrap().unwrap();
        assert_eq!(
            authority.encode_install_state(&first_read).unwrap(),
            first_bytes
        );

        let second = authority
            .new_install_state("generation-b", Some(&first_read))
            .unwrap();
        let second_bytes = authority.encode_install_state(&second).unwrap();
        let second_text = String::from_utf8(second_bytes.clone()).unwrap();
        assert!(second_text.contains("\"previous\""));
        assert!(!second_text.contains("\"previous\":null"));
        assert!(matches!(
            authority.write_install_state(&engine_root, &second),
            Ok(AtomicReplaceOutcome::Committed)
        ));
        assert_eq!(fs::read_dir(&engine_root).unwrap().count(), 1);

        let second_read = authority.read_install_state(&engine_root).unwrap().unwrap();
        assert_eq!(second_read.inner.active.directory, "generation-b");
        assert_eq!(
            second_read.inner.previous.as_ref().unwrap().directory,
            "generation-a"
        );
        assert_eq!(
            authority.encode_install_state(&second_read).unwrap(),
            second_bytes
        );
    }

    #[test]
    fn install_state_validation_is_opaque_and_path_free() {
        let authority = NativeOpenCode2Authority::new();
        assert!(matches!(
            authority.read_install_state(Path::new("toolchain/opencode2")),
            Err(NativeOpenCode2StateError::InvalidRoot)
        ));

        let unsupported = NativeOpenCode2State {
            inner: ManagedToolchainStateV1 {
                active: certified_generation("generation-a"),
                format_version: 2,
                previous: None,
            },
        };
        assert_eq!(
            authority.encode_install_state(&unsupported),
            Err(NativeOpenCode2StateError::UnsupportedVersion)
        );

        let malformed = NativeOpenCode2State {
            inner: ManagedToolchainStateV1 {
                active: ManagedGenerationV1 {
                    binary: "opencode2.exe".into(),
                    directory: "generation-a".into(),
                    sha256: "not-a-digest".into(),
                    version: "0.0.0-beta-17778".into(),
                },
                format_version: 1,
                previous: None,
            },
        };
        assert_eq!(
            authority.encode_install_state(&malformed),
            Err(NativeOpenCode2StateError::ActiveGenerationUntrusted)
        );
        assert!(!format!("{}", NativeOpenCode2StateError::UnsafePath).contains("toolchain"));
    }

    #[test]
    fn unsafe_generation_binary_version_and_digest_values_fail_closed() {
        assert!(!is_safe_basename("../generation", MAX_GENERATION_ID_BYTES));
        assert!(!is_safe_relative_path(
            "../opencode2.exe",
            MAX_BINARY_PATH_BYTES
        ));
        assert!(!is_safe_relative_path(
            "C:\\opencode2.exe",
            MAX_BINARY_PATH_BYTES
        ));
        assert!(!is_safe_relative_path(
            "nested//opencode2.exe",
            MAX_BINARY_PATH_BYTES
        ));
        assert!(!is_safe_sha256(&"A".repeat(64)));
        assert!(!is_safe_version("0.0"));
        let mut active = certified_generation("generation-a");
        active.binary = "nested/opencode2.exe".into();
        assert_eq!(
            validate_generation(
                &active,
                true,
                &NativeOpenCode2Authority::certified_install_spec(),
            ),
            Err(NativeOpenCode2Error::ActiveGenerationUntrusted)
        );
        active = certified_generation("generation-a");
        active.directory = "../generation-a".into();
        assert_eq!(
            validate_generation(
                &active,
                true,
                &NativeOpenCode2Authority::certified_install_spec(),
            ),
            Err(NativeOpenCode2Error::UnsafePath)
        );
        active = certified_generation("generation-a");
        active.version = "1.2.3".into();
        assert_eq!(
            validate_generation(
                &active,
                true,
                &NativeOpenCode2Authority::certified_install_spec(),
            ),
            Err(NativeOpenCode2Error::ActiveGenerationUntrusted)
        );
        active = certified_generation("generation-a");
        active.sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into();
        assert_eq!(
            validate_generation(
                &active,
                true,
                &NativeOpenCode2Authority::certified_install_spec(),
            ),
            Err(NativeOpenCode2Error::ActiveGenerationUntrusted)
        );
    }

    #[test]
    fn authority_errors_are_payload_free() {
        let errors = [
            NativeOpenCode2Error::UnsupportedPlatform,
            NativeOpenCode2Error::StateMissing,
            NativeOpenCode2Error::StateTooLarge,
            NativeOpenCode2Error::StateMalformed,
            NativeOpenCode2Error::StateUnsupportedVersion,
            NativeOpenCode2Error::ActiveGenerationUntrusted,
            NativeOpenCode2Error::UnsafePath,
            NativeOpenCode2Error::ExecutableUnavailable,
            NativeOpenCode2Error::ExecutableChanged,
            NativeOpenCode2Error::ExecutableSizeMismatch,
            NativeOpenCode2Error::ExecutableHashMismatch,
            NativeOpenCode2Error::Io,
        ];
        for error in errors {
            assert!(!error.to_string().contains("C:\\secret"));
            assert!(!format!("{error:?}").contains("profiles.json"));
        }
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn authority_and_resolved_debug_are_redacted() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("opencode2.exe");
        let bytes = b"debug fixture";
        fs::write(&path, bytes).unwrap();
        let identity =
            native_files::verify_file(&path, bytes.len() as u64, &digest_array(bytes)).unwrap();
        let generation = ResolvedOpenCode2Generation {
            executable: PathBuf::from("C:\\secret\\opencode2.exe"),
            generation_id: "generation-a".into(),
            version: CERTIFIED_VERSION,
            upstream_commit: CERTIFIED_UPSTREAM_COMMIT,
            executable_size_bytes: CERTIFIED_EXECUTABLE_SIZE_BYTES,
            executable_sha256: CERTIFIED_EXECUTABLE_SHA256,
            verified_file_id: identity,
        };
        let debug = format!("{generation:?}");
        assert!(!debug.contains("secret"));
        assert!(!debug.contains(CERTIFIED_EXECUTABLE_SHA256_HEX));
        assert!(!debug.contains("volume"));
        assert!(!debug.contains("dev"));
    }

    #[test]
    fn exact_database_parent_toolchain_root_is_used() {
        let root = tempfile::tempdir().unwrap();
        let database_parent = root.path().join("data");
        fs::create_dir(&database_parent).unwrap();
        let database = database_parent.join("artisan.sqlite");
        let spec = NativeOpenCode2Authority::certified_install_spec();
        let paths = NativeOpenCode2InstallPaths::derive(&database, &spec).unwrap();
        assert_eq!(
            paths.engine_root(),
            database_parent.join("toolchain").join("opencode2")
        );
        assert_eq!(
            paths.versions_root(),
            database_parent
                .join("toolchain")
                .join("opencode2")
                .join("versions")
        );

        let traversal = root
            .path()
            .join("data")
            .join("..")
            .join("other")
            .join("db.sqlite");
        assert!(matches!(
            NativeOpenCode2InstallPaths::derive(&traversal, &spec),
            Err(NativeOpenCode2InstallPathError::InvalidRoot)
        ));
    }

    #[test]
    fn missing_state_is_not_a_fallback_to_previous() {
        let state = NativeOpenCode2State {
            inner: ManagedToolchainStateV1 {
                active: certified_generation("active-generation"),
                format_version: 1,
                previous: Some(certified_generation("previous-generation")),
            },
        };
        assert_eq!(state.inner.active.directory, "active-generation");
        assert_eq!(
            state.inner.previous.as_ref().unwrap().directory,
            "previous-generation"
        );

        let root = tempfile::tempdir().unwrap();
        let database = database_path(root.path());
        let authority = NativeOpenCode2Authority::new();
        let paths = authority.install_paths(&database).unwrap();
        assert!(matches!(
            authority.read_install_state(paths.engine_root()),
            Ok(None)
        ));
        assert!(!platform_supported() || authority.resolve_active(&database).is_err());
    }

    #[test]
    fn active_generation_is_the_only_generation_path_candidate() {
        let root = tempfile::tempdir().unwrap();
        let paths = NativeOpenCode2Authority::new()
            .install_paths(&database_path(root.path()))
            .unwrap();
        let state = NativeOpenCode2State {
            inner: ManagedToolchainStateV1 {
                active: certified_generation("active-generation"),
                format_version: 1,
                previous: Some(certified_generation("previous-generation")),
            },
        };
        let active_path = paths
            .versions_root()
            .join(&state.inner.active.directory)
            .join(&state.inner.active.binary);
        assert!(active_path.ends_with("active-generation/opencode2.exe"));
        assert!(!active_path.ends_with("previous-generation/opencode2.exe"));
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn replaced_executable_identity_cannot_reuse_the_old_trust() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("opencode2.exe");
        let original = b"original binary";
        let replacement = b"replaced binary";
        assert_eq!(original.len(), replacement.len());
        fs::write(&path, original).unwrap();
        let original_hash = digest_array(original);
        let original_id =
            native_files::verify_file(&path, original.len() as u64, &original_hash).unwrap();

        let backup = root.path().join("opencode2.old");
        fs::rename(&path, &backup).unwrap();
        fs::write(&path, replacement).unwrap();
        let replacement_hash = digest_array(replacement);
        let replacement_id =
            native_files::verify_file(&path, replacement.len() as u64, &replacement_hash).unwrap();
        assert_ne!(original_id, replacement_id);
        assert_eq!(
            native_files::verify_file(&path, original.len() as u64, &original_hash),
            Err(NativeFileError::FileHashMismatch)
        );
    }

    #[test]
    fn unsupported_platform_does_not_read_managed_state() {
        let root = tempfile::tempdir().unwrap();
        let inspection = NativeOpenCode2Authority::new().inspect(&database_path(root.path()));
        if !platform_supported() {
            assert!(matches!(
                inspection,
                Ok(OpenCode2Inspection::UnsupportedPlatform)
            ));
        }
    }

    #[cfg(unix)]
    #[test]
    fn state_and_executable_symlinks_are_rejected() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let database = database_path(root.path());
        let authority = NativeOpenCode2Authority::new();
        let paths = authority.install_paths(&database).unwrap();
        fs::create_dir_all(paths.engine_root()).unwrap();
        let real_state = root.path().join("real-state.json");
        fs::write(&real_state, b"{}").unwrap();
        symlink(&real_state, paths.engine_root().join("state.json")).unwrap();
        assert!(matches!(
            authority.read_install_state(paths.engine_root()),
            Err(NativeOpenCode2StateError::UnsafePath)
        ));

        let executable = root.path().join("executable.exe");
        fs::write(&executable, b"native").unwrap();
        let link = root.path().join("executable-link.exe");
        symlink(&executable, &link).unwrap();
        let expected = digest_array(b"native");
        assert!(matches!(
            native_files::verify_file(&link, 6, &expected),
            Err(NativeFileError::UnsafePath)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn ancestor_symlinks_are_rejected_before_state_read() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("data");
        fs::create_dir(&data).unwrap();
        let database = data.join("artisan.sqlite");
        let authority = NativeOpenCode2Authority::new();
        let paths = authority.install_paths(&database).unwrap();
        let real_data = root.path().join("real-data");
        fs::create_dir_all(real_data.join("toolchain").join("opencode2")).unwrap();
        fs::remove_dir(&data).unwrap();
        symlink(&real_data, &data).unwrap();
        let state = br#"{"active":{"binary":"opencode2.exe","directory":"generation-a","sha256":"452794a764e1033e629c4cd40bde6433c10c6bd32433fb3be279bf03969a6edf","version":"0.0.0-beta-17778"},"format_version":1}"#;
        fs::write(paths.engine_root().join("state.json"), state).unwrap();
        assert!(matches!(
            authority.read_install_state(paths.engine_root()),
            Err(NativeOpenCode2StateError::UnsafePath)
        ));
    }

    #[test]
    fn bounded_state_reader_never_returns_bytes_above_its_limit() {
        let root = tempfile::tempdir().unwrap();
        let authority = NativeOpenCode2Authority::new();
        let paths = authority
            .install_paths(&database_path(root.path()))
            .unwrap();
        fs::create_dir_all(paths.engine_root()).unwrap();
        fs::write(
            paths.engine_root().join("state.json"),
            vec![b'x'; MAX_STATE_BYTES + 1],
        )
        .unwrap();
        assert!(matches!(
            authority.read_install_state(paths.engine_root()),
            Err(NativeOpenCode2StateError::TooLarge)
        ));
    }

    #[test]
    fn native_file_verification_rejects_size_and_hash_mismatch() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("opencode2.exe");
        let bytes = b"native executable fixture";
        fs::write(&path, bytes).unwrap();
        let expected = digest_array(bytes);
        assert!(native_files::verify_file(&path, (bytes.len() + 1) as u64, &expected).is_err());
        assert_eq!(
            native_files::verify_file(&path, bytes.len() as u64, &[0; 32]),
            Err(NativeFileError::FileHashMismatch)
        );
        assert!(native_files::verify_file(&path, bytes.len() as u64, &expected).is_ok());
    }

    #[test]
    fn state_bytes_remain_unchanged_after_bounded_read() {
        let root = tempfile::tempdir().unwrap();
        let authority = NativeOpenCode2Authority::new();
        let paths = authority
            .install_paths(&database_path(root.path()))
            .unwrap();
        fs::create_dir_all(paths.engine_root()).unwrap();
        let state = br#"{"active":{"binary":"opencode2.exe","directory":"generation-a","sha256":"452794a764e1033e629c4cd40bde6433c10c6bd32433fb3be279bf03969a6edf","version":"0.0.0-beta-17778"},"format_version":1}"#;
        let state_path = paths.engine_root().join("state.json");
        fs::write(&state_path, state).unwrap();
        let before = fs::read(&state_path).unwrap();
        let _ = authority.read_install_state(paths.engine_root()).unwrap();
        let after = fs::read(&state_path).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn a_different_engine_root_is_never_considered() {
        let root = tempfile::tempdir().unwrap();
        let authority = NativeOpenCode2Authority::new();
        let other_state = root
            .path()
            .join("data")
            .join("toolchain")
            .join("other-engine")
            .join("state.json");
        fs::create_dir_all(other_state.parent().unwrap()).unwrap();
        fs::write(&other_state, b"not OpenCode2 state").unwrap();
        let paths = authority
            .install_paths(&database_path(root.path()))
            .unwrap();
        assert!(matches!(
            authority.read_install_state(paths.engine_root()),
            Ok(None)
        ));
    }

    fn certified_generation(directory: &str) -> ManagedGenerationV1 {
        NativeOpenCode2Authority::certified_install_spec().generation(directory)
    }

    fn database_path(root: &Path) -> PathBuf {
        root.join("artisan.sqlite")
    }

    fn digest_array(bytes: &[u8]) -> [u8; 32] {
        use sha2::{Digest, Sha256};

        let digest = Sha256::digest(bytes);
        let mut result = [0_u8; 32];
        result.copy_from_slice(&digest);
        result
    }
}
