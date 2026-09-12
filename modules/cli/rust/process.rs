use std::time::Duration;

const FORGE_START_TIMEOUT: Duration = Duration::from_secs(30);
const FORGE_SHUTDOWN_GRACE_MAX: Duration = Duration::from_secs(2);
const FORGE_READY_INTERVAL: Duration = Duration::from_millis(100);
const MAX_READINESS_BYTES: usize = 4096;
const READY_SCHEMA: &str = "artisan-forge-ready-v1";

const OWNED_FORGE_ALREADY_RUNNING: &str =
    "refusing to adopt an already-running Forge without process ownership";
const OWNED_FORGE_START_FAILURE: &str = "owned Forge process operation failed during startup";
const OWNED_FORGE_READINESS_FAILURE: &str = "owned Forge readiness operation failed";
const OWNED_FORGE_SHUTDOWN_FAILURE: &str = "could not confirm owned Forge shutdown";

mod lifecycle;
mod receipt;
mod spec;

pub use lifecycle::{ForgeProcessLease, start, start_owned, start_owned_until, start_until};
pub use receipt::readiness_status;
pub(crate) use spec::validate_credential_manifest;
pub use spec::{ForgeLaunchSpec, ForgeReadiness, ForgeReadinessStatus, StartResult};

#[cfg(test)]
use std::process::{Command, Stdio};

#[cfg(test)]
use self::{
    lifecycle::{
        OwnedProcessIdentity, OwnedProcessStartInfo, OwnedStartDecision, clamp_shutdown_grace,
        forge_owned_command_with_environment, forge_startup_outcome_error,
        owned_already_running_failure, owned_process_identity, owned_process_identity_matches,
        owned_readiness_candidate, owned_shutdown_failure, owned_start_decision,
    },
    receipt::{
        BackgroundStartDecision, ChildProbe, ReadinessFileIdentity, ReadinessFileRead,
        ReadinessFileSnapshot, background_start_decision, poll_delay, readiness_file_replaced,
        readiness_matches_child, readiness_matches_process, wait_for_readiness_with,
    },
    spec::{configure_environment, forge_command, is_forbidden_environment_key, native_argv},
};
#[cfg(test)]
mod tests {
    use std::{
        ffi::{OsStr, OsString},
        num::NonZeroU32,
        path::{Path, PathBuf},
        time::{Duration, Instant},
    };

    use super::{
        BackgroundStartDecision, ChildProbe, Command, FORGE_SHUTDOWN_GRACE_MAX, ForgeLaunchSpec,
        ForgeProcessLease, ForgeReadiness, OwnedProcessIdentity, OwnedProcessStartInfo,
        OwnedStartDecision, ReadinessFileIdentity, ReadinessFileRead, ReadinessFileSnapshot,
        StartResult, background_start_decision, clamp_shutdown_grace, configure_environment,
        forge_owned_command_with_environment, forge_startup_outcome_error,
        is_forbidden_environment_key, native_argv, owned_already_running_failure,
        owned_process_identity, owned_process_identity_matches, owned_readiness_candidate,
        owned_shutdown_failure, owned_start_decision, poll_delay, readiness_file_replaced,
        readiness_matches_child, readiness_matches_process, wait_for_readiness_with,
    };

    use crate::{
        CliError,
        credentials::ForgeCredentialPaths,
        error::{ForgeExitCode, ForgeTermination},
        instance::{
            NativeInstanceConfig, NativeListenerConfig, NativeRunConfig, NativeRunConfigInput,
        },
        manifest::InstallationManifest,
    };

    fn test_native_config(home: &Path, credentials_manifest: &Path) -> NativeInstanceConfig {
        NativeInstanceConfig::new(
            home.join("data").join("forge.sqlite3"),
            home.join("custody").join("forge.lock"),
            home.join("readiness").join("forge.json"),
            credentials_manifest.to_path_buf(),
            NativeListenerConfig::new(
                11,
                12,
                13,
                14,
                NonZeroU32::new(3).unwrap(),
                NonZeroU32::new(4).unwrap(),
            ),
            NativeRunConfig::new(NativeRunConfigInput {
                claim_lease_ms: 15,
                poll_interval_ms: 16,
                retry_backoff_ms: 17,
                shutdown_budget_ms: 18,
                queue_capacity: 5,
                max_command_retries: 6,
                prompt_delivery: "queue".to_owned(),
                stream_after: 7,
            })
            .unwrap(),
        )
        .unwrap()
    }

    fn test_manifest() -> InstallationManifest {
        InstallationManifest {
            activation_state: "active".into(),
            finalization_state: Some("complete".into()),
            active_version: Some("1.2.3".into()),
            install_root: if cfg!(windows) {
                PathBuf::from(r"C:\Users\Ada\Artisan Street")
            } else {
                PathBuf::from("/opt/Artisan Street")
            },
            permanent_ae_path: None,
        }
    }

    fn test_launch_spec() -> ForgeLaunchSpec {
        let home = tempfile::tempdir().unwrap();
        let credentials = ForgeCredentialPaths::from_home(home.path()).unwrap();
        let config = test_native_config(home.path(), credentials.manifest_path());
        ForgeLaunchSpec::new(&test_manifest(), &config, &credentials).unwrap()
    }

    #[test]
    fn native_launch_spec_uses_the_versioned_forge_and_exact_argv() {
        let spec = test_launch_spec();
        assert_eq!(spec.executable, test_manifest().forge_executable());
        assert_eq!(spec.argv.len(), 40);
        assert_eq!(spec.argv[0], OsString::from("--database"));
        assert_eq!(spec.argv[2], OsString::from("--custody"));
        assert_eq!(spec.argv[4], OsString::from("--certificate-der"));
        assert_eq!(spec.argv[6], OsString::from("--private-key-der"));
        assert_eq!(spec.argv[8], OsString::from("--bootstrap-capability"));
        assert_eq!(spec.argv[10], OsString::from("--ready-file"));
        assert_eq!(spec.argv[12], OsString::from("--admission-timeout-ms"));
        assert_eq!(spec.argv[14], OsString::from("--handshake-timeout-ms"));
        assert_eq!(spec.argv[16], OsString::from("--request-timeout-ms"));
        assert_eq!(spec.argv[18], OsString::from("--drain-timeout-ms"));
        assert_eq!(spec.argv[20], OsString::from("--admission-capacity"));
        assert_eq!(spec.argv[21], OsString::from("3"));
        assert_eq!(spec.argv[22], OsString::from("--requests-per-connection"));
        assert_eq!(spec.argv[23], OsString::from("4"));
        assert_eq!(spec.argv[24], OsString::from("--native-run-claim-lease-ms"));
        assert_eq!(spec.argv[25], OsString::from("15"));
        assert_eq!(
            spec.argv[26],
            OsString::from("--native-run-poll-interval-ms")
        );
        assert_eq!(spec.argv[27], OsString::from("16"));
        assert_eq!(
            spec.argv[28],
            OsString::from("--native-run-retry-backoff-ms")
        );
        assert_eq!(spec.argv[29], OsString::from("17"));
        assert_eq!(
            spec.argv[30],
            OsString::from("--native-run-shutdown-budget-ms")
        );
        assert_eq!(spec.argv[31], OsString::from("18"));
        assert_eq!(spec.argv[32], OsString::from("--native-run-queue-capacity"));
        assert_eq!(spec.argv[33], OsString::from("5"));
        assert_eq!(
            spec.argv[34],
            OsString::from("--native-run-max-command-retries")
        );
        assert_eq!(spec.argv[35], OsString::from("6"));
        assert_eq!(
            spec.argv[36],
            OsString::from("--native-run-prompt-delivery")
        );
        assert_eq!(spec.argv[37], OsString::from("queue"));
        assert_eq!(spec.argv[38], OsString::from("--native-run-stream-after"));
        assert_eq!(spec.argv[39], OsString::from("7"));
    }

    #[test]
    fn launch_spec_debug_redacts_credential_paths_and_argv_values() {
        let spec = ForgeLaunchSpec {
            executable: PathBuf::from("FORGE-EXECUTABLE-SECRET"),
            argv: vec![
                OsString::from("--private-key-der"),
                OsString::from("ARGV-CREDENTIAL-SECRET"),
            ],
            readiness_path: PathBuf::from("READINESS-PATH-SECRET"),
        };

        let debug = format!("{spec:?}");
        assert!(debug.contains("ForgeLaunchSpec"));
        assert!(debug.contains("argv_count: 2"));
        for secret in [
            "FORGE-EXECUTABLE-SECRET",
            "ARGV-CREDENTIAL-SECRET",
            "READINESS-PATH-SECRET",
        ] {
            assert!(!debug.contains(secret), "debug leaked {secret}");
        }
    }

    #[test]
    fn native_argv_preserves_repeated_certificate_order_and_os_paths() {
        let home = tempfile::tempdir().unwrap();
        let credentials = ForgeCredentialPaths::from_home(home.path()).unwrap();
        let config = test_native_config(home.path(), credentials.manifest_path());
        let certificates = vec![
            home.path().join("Artisan Street").join("leaf.der"),
            home.path().join("Artisan Street").join("intermediate.der"),
        ];
        let argv = native_argv(
            &config,
            &certificates,
            credentials.private_key_path(),
            credentials.capability_path(),
        );

        let mut expected = Vec::new();
        for (option, path) in [
            ("--database", config.database_path()),
            ("--custody", config.custody_path()),
        ] {
            expected.push(OsString::from(option));
            expected.push(path.as_os_str().to_os_string());
        }
        for certificate in &certificates {
            expected.push(OsString::from("--certificate-der"));
            expected.push(certificate.as_os_str().to_os_string());
        }
        for (option, path) in [
            ("--private-key-der", credentials.private_key_path()),
            ("--bootstrap-capability", credentials.capability_path()),
            ("--ready-file", config.readiness_path()),
        ] {
            expected.push(OsString::from(option));
            expected.push(path.as_os_str().to_os_string());
        }
        expected.extend([
            OsString::from("--admission-timeout-ms"),
            OsString::from("11"),
            OsString::from("--handshake-timeout-ms"),
            OsString::from("12"),
            OsString::from("--request-timeout-ms"),
            OsString::from("13"),
            OsString::from("--drain-timeout-ms"),
            OsString::from("14"),
            OsString::from("--admission-capacity"),
            OsString::from("3"),
            OsString::from("--requests-per-connection"),
            OsString::from("4"),
            OsString::from("--native-run-claim-lease-ms"),
            OsString::from("15"),
            OsString::from("--native-run-poll-interval-ms"),
            OsString::from("16"),
            OsString::from("--native-run-retry-backoff-ms"),
            OsString::from("17"),
            OsString::from("--native-run-shutdown-budget-ms"),
            OsString::from("18"),
            OsString::from("--native-run-queue-capacity"),
            OsString::from("5"),
            OsString::from("--native-run-max-command-retries"),
            OsString::from("6"),
            OsString::from("--native-run-prompt-delivery"),
            OsString::from("queue"),
            OsString::from("--native-run-stream-after"),
            OsString::from("7"),
        ]);
        assert_eq!(argv, expected);
    }

    #[test]
    fn foreground_and_background_commands_share_the_same_executable_and_argv() {
        let spec = test_launch_spec();
        let foreground = super::forge_command(&spec);
        let mut background = super::forge_command(&spec);
        background
            .stdin(super::Stdio::null())
            .stdout(super::Stdio::null())
            .stderr(super::Stdio::null());

        assert_eq!(foreground.get_program(), background.get_program());
        assert_eq!(
            foreground.get_args().collect::<Vec<_>>(),
            background.get_args().collect::<Vec<_>>()
        );
    }

    #[test]
    fn owned_processkit_command_projects_exact_launch_and_safe_policy() {
        let spec = test_launch_spec();
        let command = forge_owned_command_with_environment(
            &spec,
            [
                (OsString::from("PATH"), OsString::from("safe")),
                (OsString::from("ARTISAN_SECRET"), OsString::from("secret")),
                (OsString::from("NODE_OPTIONS"), OsString::from("legacy")),
                (OsString::from("ELECTRON_RUN_AS_NODE"), OsString::from("1")),
                (
                    OsString::from("CODEX_SQLITE_HOME"),
                    OsString::from("legacy.sqlite"),
                ),
            ],
        );

        assert_eq!(command.program(), spec.executable().as_os_str());
        assert_eq!(command.arguments(), spec.argv());
        assert!(command.stdin_source().is_some());
        let environment = command.env_overrides();
        assert!(environment.iter().any(|(key, value)| {
            key == OsStr::new("PATH") && value.as_deref() == Some(OsStr::new("safe"))
        }));
        assert!(
            environment
                .iter()
                .all(|(key, _)| !is_forbidden_environment_key(key))
        );

        // processkit's public Debug view exposes the non-secret launch intent
        // while redacting argv/environment values; use it only for the fields
        // that do not contain credential material.
        let debug = format!("{command:?}");
        assert!(!debug.contains("secret"));
        assert!(debug.contains("args: 40"));
        assert!(debug.contains("env_clear: true"));
        assert!(debug.contains("stdin: Some(Stdin(\"Empty\"))"));
        assert!(debug.contains("stdout_mode: Null"));
        assert!(debug.contains("stderr_mode: Null"));
        assert!(debug.contains("creation_flags_extra: 134217728"));
    }

    #[tokio::test]
    async fn forge_process_lease_debug_redacts_process_details() {
        let command = if cfg!(windows) {
            processkit::Command::new("cmd.exe").args(["/C", "ping", "-n", "6", "127.0.0.1"])
        } else {
            processkit::Command::new("sh").args(["-c", "sleep 5"])
        };
        let process = command
            .stdin(processkit::Stdin::empty())
            .stdout(processkit::StdioMode::Null)
            .stderr(processkit::StdioMode::Null)
            .create_no_window()
            .start()
            .await
            .expect("test process must start");
        let pid = process.pid().expect("test process must have a PID");
        let lease = ForgeProcessLease {
            process,
            pid,
            readiness: valid_readiness(pid),
        };

        let debug = format!("{lease:?}");
        assert!(debug.contains("ForgeProcessLease"));
        assert!(debug.contains(&format!("pid: {pid}")));
        assert!(!debug.contains("LEASE-CREDENTIAL-SECRET"));
        lease
            .shutdown(Duration::ZERO)
            .await
            .expect("test process teardown must be confirmed");
    }

    #[test]
    fn credential_manifest_mismatch_is_a_typed_launch_error() {
        let home = tempfile::tempdir().unwrap();
        let credentials = ForgeCredentialPaths::from_home(home.path()).unwrap();
        let config = test_native_config(home.path(), &home.path().join("other-manifest.json"));
        assert!(matches!(
            ForgeLaunchSpec::new(&test_manifest(), &config, &credentials),
            Err(CliError::CredentialManifestMismatch { .. })
        ));
    }

    #[test]
    fn native_forge_launch_drops_legacy_arguments_and_environment() {
        let spec = test_launch_spec();
        for forbidden in [
            "--listen-port",
            "--listen-host",
            "--host",
            "--mode",
            "--static-root",
            "--token",
            "--state",
            "--database-path",
            "--broker",
            "--node",
        ] {
            assert!(
                !spec
                    .argv
                    .iter()
                    .any(|argument| argument.as_os_str() == OsStr::new(forbidden))
            );
        }
        assert!(
            super::forge_command(&spec)
                .get_envs()
                .all(|(key, _)| !is_forbidden_environment_key(key))
        );

        let mut command = Command::new("forge");
        configure_environment(
            &mut command,
            [
                (OsString::from("PATH"), OsString::from("safe")),
                (OsString::from("ARTISAN_HOME"), OsString::from("legacy")),
                (
                    OsString::from("ARTISAN_AUTH_TOKEN"),
                    OsString::from("secret"),
                ),
                (
                    OsString::from("ARTISAN_DATABASE_PATH"),
                    OsString::from("legacy.db"),
                ),
                (
                    OsString::from("ARTISAN_FORGE_STATE_PATH"),
                    OsString::from("legacy.state"),
                ),
                (
                    OsString::from("ARTISAN_LISTEN_HOST"),
                    OsString::from("127.0.0.1"),
                ),
                (
                    OsString::from("ARTISAN_LISTEN_PORT"),
                    OsString::from("4317"),
                ),
                (
                    OsString::from("ARTISAN_FORGE_MODE"),
                    OsString::from("local"),
                ),
                (
                    OsString::from("ARTISAN_BROKER_PATH"),
                    OsString::from("legacy-broker"),
                ),
                (
                    OsString::from("ARTISAN_NODE_EXECUTABLE"),
                    OsString::from("legacy-node"),
                ),
                (
                    OsString::from("ARTISAN_STATIC_FRONTEND_ROOT"),
                    OsString::from("legacy.frontend"),
                ),
                (OsString::from("NODE_PATH"), OsString::from("legacy.node")),
                (OsString::from("ELECTRON_RUN_AS_NODE"), OsString::from("1")),
                (
                    OsString::from("CODEX_SQLITE_HOME"),
                    OsString::from("legacy.sqlite"),
                ),
            ],
        );
        assert!(
            command
                .get_envs()
                .any(|(key, value)| key == OsStr::new("PATH") && value == Some(OsStr::new("safe")))
        );
        assert!(
            command
                .get_envs()
                .all(|(key, _)| !is_forbidden_environment_key(key))
        );
    }

    fn valid_readiness(pid: u32) -> ForgeReadiness {
        ForgeReadiness::new(
            super::READY_SCHEMA,
            "127.0.0.1:4317",
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            pid,
        )
        .unwrap()
    }

    fn assert_spawn_preserves_prior(
        decision: BackgroundStartDecision,
        expected: &ReadinessFileSnapshot,
    ) {
        let BackgroundStartDecision::Spawn { prior_readiness } = decision else {
            panic!("existing receipt unexpectedly authorized AlreadyRunning");
        };
        let Some(prior) = prior_readiness else {
            panic!("existing receipt was not retained as a stale-file guard");
        };
        assert_eq!(&prior, expected);
        assert!(!readiness_file_replaced(Some(&prior), &prior));
    }

    #[test]
    fn live_existing_receipt_selects_already_running_without_spawning() {
        let expected = if cfg!(windows) {
            PathBuf::from(r"C:\Artisan\versions\1.2.3\bin\forge.exe")
        } else {
            PathBuf::from("/opt/Artisan/versions/1.2.3/bin/forge")
        };
        let actual = expected.clone();
        let existing = ReadinessFileRead::Present(ReadinessFileSnapshot {
            identity: Some(ReadinessFileIdentity {
                first: 1,
                second: 10,
            }),
            bytes: serde_json::to_vec(&valid_readiness(42)).unwrap(),
        });

        assert_eq!(
            background_start_decision(existing, &expected, move |_| Some(actual.clone())),
            BackgroundStartDecision::AlreadyRunning
        );
    }

    #[test]
    fn owned_launch_refuses_a_live_receipt_instead_of_adopting_it() {
        let expected = if cfg!(windows) {
            PathBuf::from(r"C:\Artisan\versions\1.2.3\bin\forge.exe")
        } else {
            PathBuf::from("/opt/Artisan/versions/1.2.3/bin/forge")
        };
        let actual = expected.clone();
        let existing = ReadinessFileRead::Present(ReadinessFileSnapshot {
            identity: Some(ReadinessFileIdentity {
                first: 1,
                second: 10,
            }),
            bytes: serde_json::to_vec(&valid_readiness(42)).unwrap(),
        });

        assert_eq!(
            owned_start_decision(existing, &expected, move |_| Some(actual)),
            OwnedStartDecision::RefuseAlreadyRunning
        );
        assert!(matches!(
            owned_already_running_failure(),
            CliError::Unsupported(message) if message == super::OWNED_FORGE_ALREADY_RUNNING
        ));
    }

    #[test]
    fn owned_process_identity_requires_an_exact_start_token() {
        let expected = 900;
        assert_eq!(
            owned_process_identity(expected, OwnedProcessStartInfo::Present(expected)),
            OwnedProcessIdentity::Match
        );
        for current in [
            OwnedProcessStartInfo::QueryFailed,
            OwnedProcessStartInfo::Missing,
            OwnedProcessStartInfo::MissingStartTime,
            OwnedProcessStartInfo::Present(expected + 1),
        ] {
            assert_eq!(
                owned_process_identity(expected, current),
                OwnedProcessIdentity::FailClosed
            );
            assert!(!owned_process_identity_matches(expected, current));
        }
    }

    #[test]
    fn stale_or_mismatched_existing_receipts_remain_spawn_only() {
        let expected = if cfg!(windows) {
            PathBuf::from(r"C:\Artisan\versions\1.2.3\bin\forge.exe")
        } else {
            PathBuf::from("/opt/Artisan/versions/1.2.3/bin/forge")
        };
        let identity = Some(ReadinessFileIdentity {
            first: 1,
            second: 10,
        });

        let stale = ReadinessFileSnapshot {
            identity,
            bytes: serde_json::to_vec(&valid_readiness(42)).unwrap(),
        };
        assert_spawn_preserves_prior(
            background_start_decision(ReadinessFileRead::Present(stale.clone()), &expected, |_| {
                None
            }),
            &stale,
        );

        let malformed = ReadinessFileSnapshot {
            identity,
            bytes: b"not-json".to_vec(),
        };
        assert_spawn_preserves_prior(
            background_start_decision(
                ReadinessFileRead::Present(malformed.clone()),
                &expected,
                |_| panic!("malformed receipt must not resolve a process"),
            ),
            &malformed,
        );

        let wrong_pid = ReadinessFileSnapshot {
            identity,
            bytes: serde_json::to_vec(&valid_readiness(7)).unwrap(),
        };
        let live_expected = expected.clone();
        assert_spawn_preserves_prior(
            background_start_decision(
                ReadinessFileRead::Present(wrong_pid.clone()),
                &expected,
                move |pid| {
                    if pid == 42 {
                        Some(live_expected.clone())
                    } else {
                        None
                    }
                },
            ),
            &wrong_pid,
        );

        let wrong_executable = ReadinessFileSnapshot {
            identity,
            bytes: serde_json::to_vec(&valid_readiness(42)).unwrap(),
        };
        let other = expected.with_file_name(if cfg!(windows) {
            "editor.exe"
        } else {
            "editor"
        });
        assert_spawn_preserves_prior(
            background_start_decision(
                ReadinessFileRead::Present(wrong_executable.clone()),
                &expected,
                move |_| Some(other.clone()),
            ),
            &wrong_executable,
        );

        assert_eq!(
            background_start_decision(ReadinessFileRead::Missing, &expected, |_| {
                panic!("missing receipt must not resolve a process")
            }),
            BackgroundStartDecision::Spawn {
                prior_readiness: None,
            }
        );
        assert_eq!(
            background_start_decision(ReadinessFileRead::Invalid, &expected, |_| {
                panic!("unsafe receipt must not resolve a process")
            }),
            BackgroundStartDecision::Spawn {
                prior_readiness: None,
            }
        );
    }

    #[test]
    fn readiness_receipt_round_trips_exactly_with_private_typed_fields() {
        let readiness = valid_readiness(42);
        let bytes = serde_json::to_vec(&readiness).unwrap();
        assert_eq!(
            String::from_utf8(bytes.clone()).unwrap(),
            r#"{"schema":"artisan-forge-ready-v1","endpoint":"127.0.0.1:4317","certificate_sha256":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","pid":42}"#
        );
        let decoded = ForgeReadiness::from_json(&bytes).unwrap();
        assert_eq!(decoded, readiness);
        assert_eq!(decoded.schema(), super::READY_SCHEMA);
        assert_eq!(decoded.endpoint(), "127.0.0.1:4317");
        assert_eq!(decoded.certificate_sha256().len(), 64);
        assert_eq!(decoded.pid(), 42);
    }

    #[test]
    fn readiness_rejects_every_frozen_validation_edge() {
        assert!(
            ForgeReadiness::new(
                "artisan-forge-ready-v2",
                "127.0.0.1:4317",
                "a".repeat(64),
                42,
            )
            .is_err()
        );

        for endpoint in [
            "127.0.0.1:0",
            "127.0.0.2:4317",
            "0.0.0.0:4317",
            "localhost:4317",
            "[::1]:4317",
            "http://127.0.0.1:4317",
            "127.0.0.1",
            "127.0.0.1:01",
            "127.0.0.1:65536",
        ] {
            assert!(
                ForgeReadiness::new(super::READY_SCHEMA, endpoint, "a".repeat(64), 42).is_err(),
                "endpoint {endpoint}"
            );
        }

        for hash in [
            String::new(),
            "a".to_owned(),
            "a".repeat(63),
            "a".repeat(65),
            "g".repeat(64),
            "é".repeat(64),
        ] {
            assert!(
                ForgeReadiness::new(super::READY_SCHEMA, "127.0.0.1:4317", hash.as_str(), 42)
                    .is_err(),
                "hash {hash:?}"
            );
        }

        assert!(
            ForgeReadiness::new(super::READY_SCHEMA, "127.0.0.1:4317", "A".repeat(64), 42,).is_ok()
        );
        assert!(
            ForgeReadiness::new(super::READY_SCHEMA, "127.0.0.1:4317", "a".repeat(64), 0,).is_err()
        );
        assert!(ForgeReadiness::from_json(
            br#"{"schema":"artisan-forge-ready-v1","endpoint":"127.0.0.1:4317","certificate_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","pid":42,"extra":true}"#
        )
        .is_err());
        let oversized = [b' '; super::MAX_READINESS_BYTES + 1];
        assert!(ForgeReadiness::from_json(&oversized).is_err());
        for pid in [0, u32::MAX] {
            let receipt = format!(
                r#"{{"schema":"artisan-forge-ready-v1","endpoint":"127.0.0.1:4317","certificate_sha256":"{}","pid":{pid}}}"#,
                "a".repeat(64)
            );
            if pid == 0 {
                assert!(ForgeReadiness::from_json(receipt.as_bytes()).is_err());
            } else {
                assert!(ForgeReadiness::from_json(receipt.as_bytes()).is_ok());
            }
        }
    }

    #[test]
    fn stale_receipts_require_a_replaced_file_identity() {
        let receipt = serde_json::to_vec(&valid_readiness(42)).unwrap();
        let prior = ReadinessFileSnapshot {
            identity: Some(ReadinessFileIdentity {
                first: 1,
                second: 10,
            }),
            bytes: receipt.clone(),
        };
        let unchanged = prior.clone();
        assert!(!readiness_file_replaced(Some(&prior), &unchanged));
        assert!(!readiness_file_replaced(
            Some(&prior),
            &ReadinessFileSnapshot {
                identity: prior.identity,
                bytes: serde_json::to_vec(&valid_readiness(7)).unwrap(),
            }
        ));
        assert!(readiness_file_replaced(
            Some(&prior),
            &ReadinessFileSnapshot {
                identity: Some(ReadinessFileIdentity {
                    first: 1,
                    second: 11,
                }),
                bytes: receipt,
            }
        ));
        assert!(readiness_file_replaced(None, &unchanged));
        assert!(!readiness_file_replaced(
            Some(&ReadinessFileSnapshot {
                identity: None,
                bytes: Vec::new(),
            }),
            &unchanged,
        ));
        assert!(!readiness_file_replaced(
            None,
            &ReadinessFileSnapshot {
                identity: None,
                bytes: Vec::new(),
            },
        ));
    }

    #[test]
    fn readiness_identity_rejects_wrong_pid_executable_and_pid_reuse() {
        let expected = if cfg!(windows) {
            PathBuf::from(r"C:\Artisan\versions\1.2.3\bin\forge.exe")
        } else {
            PathBuf::from("/opt/Artisan/versions/1.2.3/bin/forge")
        };
        let actual = expected.clone();
        let readiness = valid_readiness(42);
        assert!(readiness_matches_child(
            &readiness,
            42,
            &expected,
            |_| Some(actual.clone()),
        ));
        assert!(!readiness_matches_child(
            &readiness,
            7,
            &expected,
            |_| Some(actual.clone()),
        ));
        assert!(!readiness_matches_child(&readiness, 42, &expected, |_| {
            Some(expected.with_file_name(if cfg!(windows) {
                "editor.exe"
            } else {
                "editor"
            }))
        },));
        assert!(!readiness_matches_process(&readiness, &expected, |_| Some(
            PathBuf::from(if cfg!(windows) {
                r"C:\Windows\System32\notepad.exe"
            } else {
                "/usr/bin/notepad"
            })
        ),));
        assert!(!readiness_matches_child(&readiness, 42, &expected, |_| {
            None
        }));
    }

    struct FakeChild {
        pid: u32,
        termination: Option<ForgeTermination>,
    }

    impl ChildProbe for FakeChild {
        fn pid(&self) -> u32 {
            self.pid
        }

        fn poll_termination(&mut self) -> crate::Result<Option<ForgeTermination>> {
            Ok(self.termination.take())
        }
    }

    #[test]
    fn child_exit_before_readiness_preserves_each_known_forge_code() {
        for code in [64, 70, 71, 72, 73, 75] {
            let mut child = FakeChild {
                pid: 42,
                termination: Some(ForgeTermination::Exited(ForgeExitCode::from_code(code))),
            };
            let result = wait_for_readiness_with(
                &mut child,
                Path::new("/opt/Artisan/versions/1.2.3/bin/forge"),
                Path::new("/tmp/forge-ready.json"),
                None,
                Instant::now() + Duration::from_secs(30),
                |_| panic!("readiness must not be read after child exit"),
                |_| panic!("identity must not be queried after child exit"),
                |_, _| panic!("poll must not sleep after child exit"),
            );
            assert!(matches!(
                result,
                Err(CliError::ForgeTerminated {
                    termination: ForgeTermination::Exited(exit)
                }) if exit.code() == code
            ));
        }
    }

    #[test]
    fn readiness_success_is_tied_to_the_spawned_child_and_exact_forge_path() {
        let expected = Path::new("/opt/Artisan/versions/1.2.3/bin/forge");
        let readiness = valid_readiness(42);
        let snapshot = ReadinessFileSnapshot {
            identity: Some(ReadinessFileIdentity {
                first: 1,
                second: 2,
            }),
            bytes: serde_json::to_vec(&readiness).unwrap(),
        };
        let mut child = FakeChild {
            pid: 42,
            termination: None,
        };
        let result = wait_for_readiness_with(
            &mut child,
            expected,
            Path::new("/tmp/forge-ready.json"),
            None,
            Instant::now() + Duration::from_secs(30),
            move |_| ReadinessFileRead::Present(snapshot.clone()),
            move |_| Some(expected.to_path_buf()),
            |_, _| panic!("ready child must not sleep"),
        )
        .unwrap();
        assert_eq!(result, StartResult::Spawned { pid: 42 });
    }

    #[test]
    fn owned_readiness_requires_spawned_pid_exact_path_and_replaced_identity() {
        let expected = Path::new("/opt/Artisan/versions/1.2.3/bin/forge");
        let prior = ReadinessFileSnapshot {
            identity: Some(ReadinessFileIdentity {
                first: 1,
                second: 10,
            }),
            bytes: serde_json::to_vec(&valid_readiness(7)).unwrap(),
        };
        let current = ReadinessFileSnapshot {
            identity: Some(ReadinessFileIdentity {
                first: 1,
                second: 11,
            }),
            bytes: serde_json::to_vec(&valid_readiness(42)).unwrap(),
        };

        assert!(
            owned_readiness_candidate(
                ReadinessFileRead::Present(current.clone()),
                Some(&prior),
                42,
                expected,
                900,
                OwnedProcessStartInfo::Present(900),
                |_| Some(expected.to_path_buf()),
            )
            .is_some()
        );

        for unavailable in [
            OwnedProcessStartInfo::QueryFailed,
            OwnedProcessStartInfo::Missing,
            OwnedProcessStartInfo::MissingStartTime,
            OwnedProcessStartInfo::Present(901),
        ] {
            assert!(
                owned_readiness_candidate(
                    ReadinessFileRead::Present(current.clone()),
                    Some(&prior),
                    42,
                    expected,
                    900,
                    unavailable,
                    |_| Some(expected.to_path_buf()),
                )
                .is_none()
            );
        }

        let unchanged = ReadinessFileSnapshot {
            identity: prior.identity,
            ..current.clone()
        };
        assert!(
            owned_readiness_candidate(
                ReadinessFileRead::Present(unchanged),
                Some(&prior),
                42,
                expected,
                900,
                OwnedProcessStartInfo::Present(900),
                |_| Some(expected.to_path_buf()),
            )
            .is_none()
        );

        assert!(
            owned_readiness_candidate(
                ReadinessFileRead::Present(current.clone()),
                Some(&prior),
                7,
                expected,
                900,
                OwnedProcessStartInfo::Present(900),
                |_| Some(expected.to_path_buf()),
            )
            .is_none()
        );

        let other = expected.with_file_name("editor");
        assert!(
            owned_readiness_candidate(
                ReadinessFileRead::Present(current),
                Some(&prior),
                42,
                expected,
                900,
                OwnedProcessStartInfo::Present(900),
                |_| Some(other),
            )
            .is_none()
        );
    }

    #[test]
    fn readiness_poll_uses_one_absolute_deadline_and_bounded_interval() {
        let now = Instant::now();
        let deadline = now + Duration::from_secs(5);
        assert_eq!(
            poll_delay(deadline, Duration::from_millis(100), now),
            Some(Duration::from_millis(100))
        );
        assert_eq!(
            poll_delay(
                deadline,
                Duration::from_millis(100),
                now + Duration::from_secs(4) + Duration::from_millis(950)
            ),
            Some(Duration::from_millis(50))
        );
        assert_eq!(
            poll_delay(deadline, Duration::from_millis(100), deadline),
            None
        );
    }

    #[test]
    fn owned_shutdown_grace_is_clamped_to_two_seconds() {
        assert_eq!(clamp_shutdown_grace(Duration::ZERO), Duration::ZERO);
        assert_eq!(
            clamp_shutdown_grace(Duration::from_millis(1_500)),
            Duration::from_millis(1_500)
        );
        assert_eq!(
            clamp_shutdown_grace(FORGE_SHUTDOWN_GRACE_MAX),
            FORGE_SHUTDOWN_GRACE_MAX
        );
        assert_eq!(
            clamp_shutdown_grace(Duration::from_secs(3)),
            FORGE_SHUTDOWN_GRACE_MAX
        );
    }

    #[test]
    fn owned_shutdown_failure_is_a_bounded_io_error() {
        let error = owned_shutdown_failure();
        assert!(matches!(
            error,
            CliError::Io { context, source }
                if context == "shutdown Forge"
                    && source.to_string() == super::OWNED_FORGE_SHUTDOWN_FAILURE
        ));
    }

    #[test]
    fn owned_startup_outcomes_keep_existing_forge_termination_mapping() {
        assert!(matches!(
            forge_startup_outcome_error(processkit::Outcome::Exited(70)),
            CliError::ForgeTerminated { termination } if termination.exit_code() == Some(70)
        ));
        assert!(matches!(
            forge_startup_outcome_error(processkit::Outcome::Signalled(Some(9))),
            CliError::ForgeTerminated { termination } if termination.exit_code().is_none()
        ));
        assert!(matches!(
            forge_startup_outcome_error(processkit::Outcome::TimedOut),
            CliError::Control(message) if message == super::OWNED_FORGE_START_FAILURE
        ));
    }
}
