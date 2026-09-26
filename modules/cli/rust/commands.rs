//! CLI command surface for `ae`.
//!
//! The clap grammar and top-level dispatch live in `cli`, engine subcommands
//! in `engine`, and the lifecycle, open, autostart, and doctor flows in their
//! own child modules.

use std::time::Duration;

#[cfg(test)]
use std::{num::NonZeroU32, path::PathBuf};

mod autostart;
mod cli;
mod doctor;
mod engine;
mod lifecycle;
mod open;

pub use crate::engine_profiles::{EngineProfileCommand, EngineProfileHomeArg};
pub use cli::{
    Cli, Commands, EngineCommand, NativeRunPromptDelivery, TelemetryChoice, TelemetryCommand, run,
};

use cli::{
    NativeSetupValues, load_native_instance, native_launch_spec, require_installation,
    require_launchable_installation,
};

#[cfg(test)]
use cli::{parse_native_run_duration_ms, parse_native_run_prompt_delivery, parse_positive_u64};

#[cfg(test)]
use clap::Parser;

// The test module exercises the child flows through the parent's historical
// `super::*` surface, so the names stay bound here for test builds.
#[cfg(test)]
use autostart::{delegate_installer, setup_native, start, unsupported_lifecycle_control};
#[cfg(test)]
use doctor::doctor;
#[cfg(test)]
use lifecycle::{status, stop};
#[cfg(test)]
use open::{OpenFlow, handle_protocol, open};

#[cfg(test)]
use crate::{
    CliError,
    instance::{NativeInstanceConfig, NativeListenerConfig, NativeRunConfig, NativeRunConfigInput},
    manifest::InstallationFinalization,
    paths::Layout,
    process,
};

const FORGE_READY_TIMEOUT: Duration = Duration::from_secs(30);
const FORGE_START_LAUNCH_URL: &str = "artisan://forge/start";
const INCOMPLETE_INSTALLATION_GUIDANCE: &str =
    "installation finalization is incomplete; rerun the installer or run `ae doctor --fix`";
const INVALID_INSTALLATION_GUIDANCE: &str =
    "installation finalization state is invalid; rerun the installer or run `ae doctor --fix`";

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::Path,
        time::{Duration, Instant},
    };

    use artisan_protocol::{
        ClientRequest, ErrorCode, HelloCredential, LifecycleRequest, LifecycleState,
        LifecycleStatus, LifecycleStopDisposition, LifecycleStopReceipt, WireEnvelopeBody,
    };
    use artisan_transport::{ClientRequestError, ClientSessionError, DeadlineError, OperationKind};
    use serde_json::json;

    use crate::credentials::{ForgeCredentialError, ForgeCredentialPaths};
    use crate::payload;

    use super::autostart::{
        ScheduledTaskDeletion, StableLauncher, scheduled_task_action, scheduled_task_create_args,
        scheduled_task_deletion, stable_launcher_kind,
    };
    use super::doctor::{
        DoctorFinalizationReport, DoctorPayloadIssue, inspect_existing_credentials,
        inspect_native_instance, payload_issue_codes,
    };
    use super::lifecycle::{
        LifecycleOperation, classify_lifecycle_failure, classify_stop_receipt,
        lifecycle_connect_error, lifecycle_credential_error, lifecycle_hello_with_capability,
        lifecycle_request, lifecycle_request_error, lifecycle_request_requires_quarantine,
        lifecycle_state_name,
    };
    #[cfg(target_os = "windows")]
    use super::open::EDITOR_CREATION_FLAGS;
    use super::open::{
        ReadyState, handoff_json, launch_editor, mint_pair_code, open_flow_requires_ready,
        open_ready, ready_state, resolve_browser_origin, resolved_open_flow, start_until,
        validate_origin,
    };
    use super::*;

    fn test_layout(root: &Path) -> Layout {
        Layout {
            manifest: root.join("installation.json"),
            root: root.to_path_buf(),
        }
    }

    fn native_permanent_path(root: &Path) -> PathBuf {
        root.join("bin")
            .join(if cfg!(windows) { "ae.exe" } else { "ae" })
    }

    fn write_test_manifest(
        root: &Path,
        activation_state: &str,
        finalization_state: Option<&str>,
    ) -> PathBuf {
        fs::create_dir_all(root).unwrap();
        let mut value = json!({
            "activation_state": activation_state,
            "active_version": "1.2.3",
            "install_root": root,
            "permanent_ae_path": native_permanent_path(root),
        });
        if let Some(finalization_state) = finalization_state {
            value["finalization_state"] = json!(finalization_state);
        }
        let path = root.join("installation.json");
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        path
    }

    fn assert_installation_error(error: CliError, guidance: &str) {
        assert_eq!(error.exit_code(), 4);
        assert!(matches!(error, CliError::Installation(message) if message == guidance));
    }

    fn test_native_config(root: &Path, credentials_manifest: PathBuf) -> NativeInstanceConfig {
        NativeInstanceConfig::new(
            root.join("data").join("forge.sqlite3"),
            root.join("custody").join("forge.lock"),
            root.join("readiness").join("forge.json"),
            credentials_manifest,
            NativeListenerConfig::new(
                1,
                2,
                3,
                4,
                NonZeroU32::new(1).unwrap(),
                NonZeroU32::new(1).unwrap(),
            ),
            NativeRunConfig::new(NativeRunConfigInput {
                claim_lease_ms: 1,
                poll_interval_ms: 2,
                retry_backoff_ms: 3,
                shutdown_budget_ms: 4,
                queue_capacity: 1,
                max_command_retries: 1,
                prompt_delivery: "queue".to_owned(),
                stream_after: 0,
            })
            .unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn telemetry_commands_are_explicit_and_reset_requires_confirmation() {
        let analytics = Cli::try_parse_from(["ae", "telemetry", "analytics", "enable"])
            .expect("analytics telemetry command");
        assert!(matches!(
            analytics.command,
            Some(Commands::Telemetry {
                command: TelemetryCommand::Analytics {
                    choice: TelemetryChoice::Enable,
                },
            })
        ));
        assert!(Cli::try_parse_from(["ae", "telemetry", "reset-identity"]).is_err());
        assert!(Cli::try_parse_from(["ae", "telemetry", "reset-identity", "--yes",]).is_ok());
    }

    #[test]
    fn plain_invocation_maps_to_open() {
        let cli = Cli::try_parse_from(["ae"]).unwrap();
        assert!(cli.command.is_none());
    }

    #[test]
    fn finalization_check_has_a_hidden_json_only_grammar() {
        let cli = Cli::try_parse_from(["ae", "doctor", "--json", "--finalization-check"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Doctor {
                fix: false,
                json: true,
                finalization_check: true,
            })
        ));
        assert!(Cli::try_parse_from(["ae", "doctor", "--finalization-check"]).is_err());
        assert!(
            Cli::try_parse_from(["ae", "doctor", "--json", "--fix", "--finalization-check"])
                .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "ae",
                "doctor",
                "--json",
                "--finalization-check",
                "--finalization-check",
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "ae",
                "doctor",
                "--json",
                "--finalization-check",
                "--unknown"
            ])
            .is_err()
        );
    }

    #[test]
    fn open_defaults_to_the_editor_and_keeps_explicit_browser_and_handoff_flows() {
        let default_open = Cli::try_parse_from(["ae", "open"]).unwrap();
        assert!(matches!(
            default_open.command,
            Some(Commands::Open {
                browser: false,
                handoff: false,
                origin: None,
            })
        ));
        let browser = Cli::try_parse_from(["ae", "open", "--browser"]).unwrap();
        assert!(matches!(
            browser.command,
            Some(Commands::Open { browser: true, .. })
        ));
        let handoff = Cli::try_parse_from(["ae", "open", "--handoff"]).unwrap();
        assert!(matches!(
            handoff.command,
            Some(Commands::Open { handoff: true, .. })
        ));
        // The handoff prints a capability for a trusted local caller; it never
        // combines with a browser navigation that would expose it elsewhere.
        assert!(Cli::try_parse_from(["ae", "open", "--handoff", "--browser"]).is_err());
        assert!(
            Cli::try_parse_from(["ae", "open", "--handoff", "--origin", "http://127.0.0.1:1"])
                .is_err()
        );
    }

    #[test]
    fn idle_pid_stop_is_the_only_supported_stop_syntax() {
        let pid_stop = Cli::try_parse_from(["ae", "stop", "--pid", "6172", "--if-idle"]).unwrap();
        assert!(matches!(
            pid_stop.command,
            Some(Commands::Stop {
                pid,
                if_idle: true,
            }) if pid.get() == 6172
        ));
        assert!(Cli::try_parse_from(["ae", "stop"]).is_err());
        assert!(Cli::try_parse_from(["ae", "stop", "--pid", "6172"]).is_err());
        assert!(Cli::try_parse_from(["ae", "stop", "--if-idle"]).is_err());
        assert!(Cli::try_parse_from(["ae", "stop", "--pid", "0", "--if-idle"]).is_err());
        assert!(Cli::try_parse_from(["ae", "stop", "--pid", "6172", "--force"]).is_err());
        assert!(Cli::try_parse_from(["ae", "stop", "--instance-id", "forge-1"]).is_err());
        let disable = Cli::try_parse_from(["ae", "autostart", "--disable"]).unwrap();
        assert!(matches!(
            disable.command,
            Some(Commands::Autostart { disable: true })
        ));
    }

    #[test]
    fn installed_editor_flow_never_requires_forge_readiness_before_launch() {
        assert_eq!(resolved_open_flow(OpenFlow::Editor, true), OpenFlow::Editor);
        assert!(!open_flow_requires_ready(OpenFlow::Editor));
        assert!(open_flow_requires_ready(OpenFlow::Browser));
        assert!(open_flow_requires_ready(OpenFlow::Handoff));
        // A development home has no editor payload, so it deliberately falls
        // back to the browser flow that must pair with a ready Forge.
        assert_eq!(
            resolved_open_flow(OpenFlow::Editor, false),
            OpenFlow::Browser
        );
    }

    #[test]
    fn launch_admission_accepts_only_a_complete_canonical_pointer() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("Artisan Street");
        let layout = test_layout(&root);

        let error = require_launchable_installation(&layout).unwrap_err();
        assert_installation_error(error, INCOMPLETE_INSTALLATION_GUIDANCE);

        write_test_manifest(&root, "active", Some("pending"));
        let error = require_launchable_installation(&layout).unwrap_err();
        assert_installation_error(error, INCOMPLETE_INSTALLATION_GUIDANCE);

        write_test_manifest(&root, "active", None);
        let error = require_launchable_installation(&layout).unwrap_err();
        assert_installation_error(error, INCOMPLETE_INSTALLATION_GUIDANCE);

        write_test_manifest(&root, "active", Some("unknown"));
        let error = require_launchable_installation(&layout).unwrap_err();
        assert_installation_error(error, INVALID_INSTALLATION_GUIDANCE);

        write_test_manifest(&root, "inactive", Some("complete"));
        let error = require_launchable_installation(&layout).unwrap_err();
        assert_installation_error(error, INVALID_INSTALLATION_GUIDANCE);

        write_test_manifest(&root, "active", Some("complete"));
        let manifest = require_launchable_installation(&layout).unwrap();
        assert_eq!(
            manifest.finalization_status(),
            InstallationFinalization::Complete
        );
        assert_eq!(manifest.install_root, root);
    }

    #[test]
    fn pending_admission_fails_before_every_forge_capable_entry() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("Artisan Street");
        let manifest_path = write_test_manifest(&root, "active", Some("pending"));
        let layout = test_layout(&root);
        let before = fs::read(&manifest_path).unwrap();

        for result in [
            start(&layout, false).map(|_| ()),
            start_until(&layout, false, Instant::now()).map(|_| ()),
            ready_state(&layout).map(|_| ()),
            status(&layout, false),
            stop(&layout, NonZeroU32::new(1).unwrap(), true),
            open(&layout, None, OpenFlow::Editor),
            open(&layout, None, OpenFlow::Browser),
            open(&layout, None, OpenFlow::Handoff),
            handle_protocol(&layout, FORGE_START_LAUNCH_URL),
            launch_editor(&layout),
            native_launch_spec(&layout).map(|_| ()),
            delegate_installer(&layout, "update", false),
        ] {
            let error = result.expect_err("pending installation was admitted");
            assert_installation_error(error, INCOMPLETE_INSTALLATION_GUIDANCE);
        }

        let readiness = process::ForgeReadiness::new(
            "artisan-forge-ready-v1",
            "127.0.0.1:4317",
            "a".repeat(64),
            42,
        )
        .unwrap();
        let error = mint_pair_code(&layout, &readiness)
            .expect_err("pending installation reached pair-code connection");
        assert_installation_error(error, INCOMPLETE_INSTALLATION_GUIDANCE);
        for flow in [OpenFlow::Browser, OpenFlow::Handoff] {
            let error = open_ready(
                &layout,
                None,
                flow,
                &ReadyState {
                    readiness: readiness.clone(),
                },
            )
            .expect_err("pending installation reached pair-code connection");
            assert_installation_error(error, INCOMPLETE_INSTALLATION_GUIDANCE);
        }

        assert_eq!(fs::read(&manifest_path).unwrap(), before);
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
    }

    #[test]
    fn doctor_keeps_unverifiable_payload_diagnostic_while_launch_admission_rejects_it() {
        let root = tempfile::tempdir().unwrap();
        let health = payload::verify(root.path());

        assert_eq!(health, payload::PayloadHealth::Unverifiable);
        assert_eq!(health.as_str(), "unverifiable");
        assert!(!matches!(health, payload::PayloadHealth::Modified(_)));

        let error = payload::require_verified(root.path()).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Artisan is not installed correctly: active version payload is not verified"
        );
    }

    #[test]
    fn finalization_report_is_stable_enum_only_and_redacts_payload_paths() {
        let report = DoctorFinalizationReport {
            schema: "artisan-doctor-finalization-v1",
            healthy: false,
            finalization: "pending",
            installation: "ok",
            protocol: "deferred",
            instance: "ok",
            credentials: "ok",
            payload: "modified",
            payload_issues: vec![DoctorPayloadIssue::Modified],
        };
        let encoded = serde_json::to_string(&report).unwrap();
        assert_eq!(
            encoded,
            r#"{"schema":"artisan-doctor-finalization-v1","healthy":false,"finalization":"pending","installation":"ok","protocol":"deferred","instance":"ok","credentials":"ok","payload":"modified","payload_issues":["modified"]}"#
        );
        let value: serde_json::Value = serde_json::from_str(&encoded).unwrap();
        let mut keys = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        keys.sort_unstable();
        assert_eq!(
            keys.as_slice(),
            &[
                "credentials",
                "finalization",
                "healthy",
                "installation",
                "instance",
                "payload",
                "payload_issues",
                "protocol",
                "schema",
            ]
        );
        let canary = "payload-secret-canary";
        let source_issues = vec![
            format!("modified: {canary}"),
            format!("missing: {canary}"),
            format!("unexpected: {canary}"),
        ];
        assert_eq!(
            payload_issue_codes(&source_issues),
            vec![
                DoctorPayloadIssue::Modified,
                DoctorPayloadIssue::Missing,
                DoctorPayloadIssue::Unexpected,
            ]
        );
        assert!(
            !serde_json::to_string(&payload_issue_codes(&source_issues))
                .unwrap()
                .contains(canary)
        );
    }

    #[test]
    fn finalization_doctor_uses_read_only_native_v2_checks_without_legacy_fallback() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("Artisan Street");
        let layout = test_layout(&root);
        let manifest_path = write_test_manifest(&root, "active", Some("pending"));
        let legacy_config = root.join("config.json");
        let legacy_secrets = root.join("secrets.json");
        fs::write(&legacy_config, b"legacy-config-secret-canary").unwrap();
        fs::write(&legacy_secrets, b"legacy-secrets-secret-canary").unwrap();
        let before_manifest = fs::read(&manifest_path).unwrap();
        let before_config = fs::read(&legacy_config).unwrap();
        let before_secrets = fs::read(&legacy_secrets).unwrap();
        let mut before_entries = fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        before_entries.sort();

        let error = doctor(&layout, false, true, true).unwrap_err();
        assert_installation_error(error, "doctor found unresolved issues");
        assert_eq!(fs::read(&manifest_path).unwrap(), before_manifest);
        assert_eq!(fs::read(&legacy_config).unwrap(), before_config);
        assert_eq!(fs::read(&legacy_secrets).unwrap(), before_secrets);
        let mut after_entries = fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<Vec<_>>();
        after_entries.sort();
        assert_eq!(after_entries, before_entries);
        assert!(!root.join("instance-v2.json").exists());
        assert!(!root.join("credentials").exists());
        assert_eq!(inspect_native_instance(&layout).0, "missing");
    }

    #[test]
    fn native_v2_and_credential_inspection_are_typed_and_path_fenced() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("Artisan Street");
        let layout = test_layout(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("config.json"), b"legacy config").unwrap();
        fs::write(root.join("secrets.json"), b"legacy secrets").unwrap();
        assert_eq!(inspect_native_instance(&layout).0, "missing");

        let credentials = ForgeCredentialPaths::from_home(&root).unwrap();
        let config = test_native_config(&root, credentials.manifest_path().to_path_buf());
        config.write_to_home(&root).unwrap();
        let (state, loaded) = inspect_native_instance(&layout);
        assert_eq!(state, "ok");
        let loaded = loaded.unwrap();
        assert_eq!(inspect_existing_credentials(&layout, &loaded), "missing");

        let mismatch = test_native_config(&root, root.join("wrong-credentials.json"));
        assert_eq!(inspect_existing_credentials(&layout, &mismatch), "invalid");

        fs::create_dir_all(root.join("credentials")).unwrap();
        fs::write(
            root.join("credentials").join("manifest.json"),
            b"credential-secret-canary",
        )
        .unwrap();
        assert_eq!(inspect_existing_credentials(&layout, &loaded), "invalid");

        let legacy_config = fs::read(root.join("config.json")).unwrap();
        let legacy_secrets = fs::read(root.join("secrets.json")).unwrap();
        fs::write(
            layout.native_instance_path(),
            br#"{"schema":"artisan-instance-v2","version":2,"unknown":true}"#,
        )
        .unwrap();
        assert_eq!(inspect_native_instance(&layout).0, "invalid");
        assert_eq!(fs::read(root.join("config.json")).unwrap(), legacy_config);
        assert_eq!(fs::read(root.join("secrets.json")).unwrap(), legacy_secrets);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn installed_editor_is_detached_from_the_ae_launcher() {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const DETACHED_PROCESS: u32 = 0x0000_0008;

        assert_eq!(EDITOR_CREATION_FLAGS, CREATE_NO_WINDOW | DETACHED_PROCESS);
    }

    #[test]
    fn restart_remains_explicitly_unsupported() {
        assert!(matches!(
            unsupported_lifecycle_control(),
            Err(CliError::UnsupportedLifecycleControl)
        ));
        assert_eq!(
            CliError::UnsupportedLifecycleControl.to_string(),
            "native Forge lifecycle control is unsupported by this Forge"
        );
    }

    #[test]
    fn lifecycle_requests_are_correlated_and_stop_is_idle_fenced() {
        let (status, status_id) = lifecycle_request(LifecycleOperation::Status).unwrap();
        assert_eq!(status.frame_id.to_request_id().unwrap(), status_id);
        assert!(matches!(
            status.body,
            WireEnvelopeBody::Request(ClientRequest::Lifecycle(LifecycleRequest::Status))
        ));

        let (stop, stop_id) = lifecycle_request(LifecycleOperation::Stop).unwrap();
        assert_eq!(stop.frame_id.to_request_id().unwrap(), stop_id);
        assert_ne!(status.frame_id, stop.frame_id);
        assert!(matches!(
            stop.body,
            WireEnvelopeBody::Request(ClientRequest::Lifecycle(LifecycleRequest::Stop {
                require_idle: true
            }))
        ));
    }

    #[test]
    fn lifecycle_failure_boundaries_are_typed_without_retryable_outcomes() {
        assert_eq!(
            classify_lifecycle_failure(LifecycleOperation::Stop, ErrorCode::LifecycleBusy)
                .unwrap_err()
                .exit_code(),
            5
        );
        assert_eq!(
            classify_lifecycle_failure(LifecycleOperation::Status, ErrorCode::Internal)
                .unwrap_err()
                .exit_code(),
            72
        );
        assert_eq!(
            classify_lifecycle_failure(LifecycleOperation::Status, ErrorCode::UnsupportedFeature)
                .unwrap_err()
                .exit_code(),
            1
        );
        assert_eq!(
            lifecycle_connect_error(&ClientSessionError::DeliveryAlreadyTaken).exit_code(),
            72
        );
    }

    #[test]
    fn lifecycle_handshake_and_request_failures_are_terminal_and_redacted() {
        let handshake_timeout = ClientSessionError::Handshake(DeadlineError::Timeout {
            operation: OperationKind::Handshake,
            limit: Duration::from_millis(10),
        });
        assert_eq!(lifecycle_connect_error(&handshake_timeout).exit_code(), 75);

        let connect_timeout = ClientSessionError::Connect(DeadlineError::Timeout {
            operation: OperationKind::Connect,
            limit: Duration::from_millis(10),
        });
        assert_eq!(lifecycle_connect_error(&connect_timeout).exit_code(), 72);

        let acknowledgement_timeout = ClientRequestError::Exchange(DeadlineError::Timeout {
            operation: OperationKind::Receive,
            limit: Duration::from_millis(10),
        });
        assert!(lifecycle_request_requires_quarantine(
            &acknowledgement_timeout
        ));
        let failure = lifecycle_request_error(&acknowledgement_timeout);
        assert_eq!(failure.exit_code(), 72);
        assert!(!failure.to_string().contains("10ms"));

        let correlation_failure =
            ClientRequestError::Correlation(artisan_domain::IdentifierError::Empty);
        assert!(lifecycle_request_requires_quarantine(&correlation_failure));
    }

    #[test]
    fn lifecycle_hello_advertises_control_with_only_reconnect_credential() {
        let Ok(hello) =
            lifecycle_hello_with_capability(artisan_protocol::ReconnectCapability::from_bytes(
                [0xa5; artisan_protocol::RECONNECT_CAPABILITY_BYTES],
            ))
        else {
            panic!("lifecycle hello construction failed");
        };
        let WireEnvelopeBody::Hello(hello) = hello.body else {
            panic!("lifecycle hello body");
        };
        assert!(hello.supports_lifecycle_control);
        assert!(matches!(hello.credential, HelloCredential::Reconnect(_)));
    }

    #[test]
    fn lifecycle_custody_states_keep_missing_malformed_and_stale_fences_distinct() {
        assert_eq!(
            lifecycle_credential_error(&ForgeCredentialError::ReconnectRecordMissing).exit_code(),
            64
        );
        assert_eq!(
            lifecycle_credential_error(&ForgeCredentialError::ReconnectRecordMalformed).exit_code(),
            64
        );
        assert_eq!(
            lifecycle_credential_error(&ForgeCredentialError::CapabilityBusy).exit_code(),
            75
        );
        assert_eq!(
            lifecycle_credential_error(&ForgeCredentialError::ReconnectStaleWriter).exit_code(),
            75
        );
        assert_eq!(
            lifecycle_credential_error(&ForgeCredentialError::ReconnectBindingMismatch).exit_code(),
            75
        );
    }

    #[test]
    fn lifecycle_status_accepts_all_valid_states_and_stop_only_drains() {
        for (state, count) in [
            (LifecycleState::Ready, 0),
            (LifecycleState::Busy, 2),
            (LifecycleState::Draining, 2),
        ] {
            let status = LifecycleStatus::new(state, count).unwrap();
            assert_eq!(
                lifecycle_state_name(state),
                match state {
                    LifecycleState::Ready => "ready",
                    LifecycleState::Busy => "busy",
                    LifecycleState::Draining => "draining",
                }
            );
            assert!(status.validate().is_ok());
        }
        let ready_receipt = LifecycleStopReceipt {
            disposition: LifecycleStopDisposition::Accepted,
            state: LifecycleState::Ready,
        };
        assert_eq!(
            classify_stop_receipt(&ready_receipt)
                .unwrap_err()
                .exit_code(),
            72
        );
        for disposition in [
            LifecycleStopDisposition::Accepted,
            LifecycleStopDisposition::Duplicate,
            LifecycleStopDisposition::AlreadyStopping,
        ] {
            let receipt = LifecycleStopReceipt {
                disposition,
                state: LifecycleState::Draining,
            };
            assert!(classify_stop_receipt(&receipt).is_ok());
        }
    }

    #[test]
    fn handoff_uses_only_validated_non_secret_readiness_data() {
        let readiness = process::ForgeReadiness::new(
            "artisan-forge-ready-v1",
            "127.0.0.1:4317",
            "a".repeat(64),
            42,
        )
        .unwrap();
        let handoff = handoff_json(&ReadyState { readiness }, "pair");
        assert_eq!(handoff["endpoint"], "http://127.0.0.1:4317");
        assert_eq!(handoff["pair_code"], "pair");
        assert!(handoff.get("owned_instance_id").is_none());
    }

    #[test]
    fn scheduled_task_uses_a_fixed_limited_current_user_logon_argv() {
        let action = scheduled_task_action(
            Path::new(r"C:\Program Files\Artisan\ae.exe"),
            StableLauncher::Executable,
            None,
        )
        .unwrap();
        let args = scheduled_task_create_args(&action);
        assert_eq!(
            args,
            [
                "/Create",
                "/TN",
                "Artisan Forge",
                "/TR",
                r#""C:\Program Files\Artisan\ae.exe" start"#,
                "/SC",
                "ONLOGON",
                "/RL",
                "LIMITED",
                "/F",
            ]
        );
    }

    #[test]
    fn scheduled_task_runs_stable_cmd_launchers_through_trusted_cmd() {
        let action = scheduled_task_action(
            Path::new(r"C:\Program Files\Artisan\bin\ae.cmd"),
            StableLauncher::CommandScript,
            Some(PathBuf::from(r"C:\Windows\System32\cmd.exe")),
        )
        .unwrap();
        assert_eq!(
            action,
            r#""C:\Windows\System32\cmd.exe" /d /s /c ""C:\Program Files\Artisan\bin\ae.cmd" start""#
        );
        assert_eq!(
            stable_launcher_kind(Path::new(r"C:\Program Files\Artisan\bin\ae.cmd")),
            Some(StableLauncher::CommandScript)
        );
        assert_eq!(
            stable_launcher_kind(Path::new(r"C:\Program Files\Artisan\bin\ae.bat")),
            Some(StableLauncher::CommandScript)
        );
        assert_eq!(
            stable_launcher_kind(Path::new(r"C:\Program Files\Artisan\bin\ae.ps1")),
            None
        );
        for unsafe_character in ['%', '!', '^', '&', '|', '<', '>', '(', ')'] {
            let path = PathBuf::from(format!(
                r"C:\Program Files\Artisan{unsafe_character}Co\bin\ae.cmd"
            ));
            assert!(
                scheduled_task_action(
                    &path,
                    StableLauncher::CommandScript,
                    Some(PathBuf::from(r"C:\Windows\System32\cmd.exe")),
                )
                .is_err()
            );
        }
    }

    #[test]
    fn scheduled_task_removal_is_idempotent_without_hiding_delete_failures() {
        assert_eq!(
            scheduled_task_deletion(false, false),
            ScheduledTaskDeletion::AlreadyAbsent
        );
        assert_eq!(
            scheduled_task_deletion(true, true),
            ScheduledTaskDeletion::Deleted
        );
        assert_eq!(
            scheduled_task_deletion(true, false),
            ScheduledTaskDeletion::Failed
        );
    }

    #[test]
    fn handoff_wait_budget_covers_the_renderer_cold_start_window() {
        assert_eq!(FORGE_READY_TIMEOUT, Duration::from_secs(30));
    }

    fn explicit_setup_args() -> Vec<String> {
        let (database, custody, readiness) = if cfg!(windows) {
            (
                r"C:\Artisan Street\data\forge.sqlite3",
                r"C:\Artisan Street\custody\forge.lock",
                r"C:\Artisan Street\readiness\forge.json",
            )
        } else {
            (
                "/tmp/Artisan Street/data/forge.sqlite3",
                "/tmp/Artisan Street/custody/forge.lock",
                "/tmp/Artisan Street/readiness/forge.json",
            )
        };
        [
            "ae",
            "setup",
            "--database-path",
            database,
            "--custody-path",
            custody,
            "--readiness-path",
            readiness,
            "--admission-timeout-ms",
            "101",
            "--handshake-timeout-ms",
            "202",
            "--request-timeout-ms",
            "303",
            "--drain-timeout-ms",
            "404",
            "--admission-capacity",
            "3",
            "--requests-per-connection",
            "4",
            "--native-run-claim-lease-ms",
            "505",
            "--native-run-poll-interval-ms",
            "506",
            "--native-run-retry-backoff-ms",
            "507",
            "--native-run-shutdown-budget-ms",
            "508",
            "--native-run-queue-capacity",
            "9",
            "--native-run-max-command-retries",
            "10",
            "--native-run-prompt-delivery",
            "queue",
            "--native-run-stream-after",
            "0",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    }

    fn replace_setup_value(arguments: &mut [String], option: &str, value: &str) {
        let position = arguments
            .iter()
            .position(|argument| argument == option)
            .expect("setup option should exist");
        arguments[position + 1] = value.to_owned();
    }

    #[test]
    fn setup_requires_explicit_native_values_and_preserves_exact_values() {
        let valid = Cli::try_parse_from(explicit_setup_args()).unwrap();
        assert!(matches!(
            valid.command,
            Some(Commands::Setup {
                database_path,
                custody_path,
                readiness_path,
                admission_timeout_ms: 101,
                handshake_timeout_ms: 202,
                request_timeout_ms: 303,
                drain_timeout_ms: 404,
                admission_capacity,
                requests_per_connection,
                native_run_claim_lease_ms: 505,
                native_run_poll_interval_ms: 506,
                native_run_retry_backoff_ms: 507,
                native_run_shutdown_budget_ms: 508,
                native_run_queue_capacity,
                native_run_max_command_retries,
                native_run_prompt_delivery,
                native_run_stream_after: 0,
                ..
            }) if database_path.is_absolute()
                && custody_path.is_absolute()
                && readiness_path.is_absolute()
                && admission_capacity.get() == 3
                && requests_per_connection.get() == 4
                && native_run_queue_capacity.get() == 9
                && native_run_max_command_retries.get() == 10
                && native_run_prompt_delivery.0 == "queue"
        ));

        assert!(Cli::try_parse_from(["ae", "setup"]).is_err());
        for option in [
            "--admission-timeout-ms",
            "--handshake-timeout-ms",
            "--request-timeout-ms",
            "--drain-timeout-ms",
            "--native-run-claim-lease-ms",
            "--native-run-poll-interval-ms",
            "--native-run-retry-backoff-ms",
            "--native-run-shutdown-budget-ms",
            "--native-run-queue-capacity",
            "--native-run-max-command-retries",
        ] {
            let mut arguments = explicit_setup_args();
            replace_setup_value(&mut arguments, option, "0");
            assert!(
                Cli::try_parse_from(arguments).is_err(),
                "zero argument {option}"
            );
        }
    }

    #[test]
    fn setup_rejects_native_run_boundary_and_legacy_values() {
        assert_eq!(parse_positive_u64(&u64::MAX.to_string()), Ok(u64::MAX));
        assert_eq!(
            parse_native_run_duration_ms(&(u64::MAX / 2).to_string()),
            Ok(u64::MAX / 2)
        );
        assert_eq!(
            parse_native_run_prompt_delivery(&"p".repeat(256))
                .map(NativeRunPromptDelivery::into_string),
            Ok("p".repeat(256))
        );
        assert_eq!(
            parse_native_run_duration_ms("0"),
            Err(String::from(
                "must be positive and fit the native-run duration range"
            ))
        );
        assert_eq!(
            parse_native_run_prompt_delivery("queue").map(NativeRunPromptDelivery::into_string),
            Ok(String::from("queue"))
        );
        for (option, invalid) in [
            ("--native-run-prompt-delivery", ""),
            ("--native-run-prompt-delivery", "line\nbreak"),
            ("--native-run-claim-lease-ms", "not-a-number"),
            ("--native-run-retry-backoff-ms", "-1"),
        ] {
            let mut arguments = explicit_setup_args();
            replace_setup_value(&mut arguments, option, invalid);
            assert!(Cli::try_parse_from(arguments).is_err(), "argument {option}");
        }
        let too_long_prompt = "p".repeat(257);
        let mut arguments = explicit_setup_args();
        replace_setup_value(
            &mut arguments,
            "--native-run-prompt-delivery",
            &too_long_prompt,
        );
        assert!(Cli::try_parse_from(arguments).is_err());
        for option in [
            "--native-run-claim-lease-ms",
            "--native-run-poll-interval-ms",
            "--native-run-retry-backoff-ms",
            "--native-run-shutdown-budget-ms",
        ] {
            let mut arguments = explicit_setup_args();
            replace_setup_value(&mut arguments, option, &(u64::MAX / 2 + 1).to_string());
            assert!(
                Cli::try_parse_from(arguments).is_err(),
                "overflow argument {option}"
            );
        }
        for (option, invalid) in [
            ("--native-run-queue-capacity", "4294967296"),
            ("--native-run-max-command-retries", "4294967296"),
        ] {
            let mut arguments = explicit_setup_args();
            replace_setup_value(&mut arguments, option, invalid);
            assert!(
                Cli::try_parse_from(arguments).is_err(),
                "overflow argument {option}"
            );
        }
        for legacy in [
            "--listen-port",
            "--listen-host",
            "--mode",
            "--data-root",
            "--serve-frontend",
            "--token",
        ] {
            let mut arguments = explicit_setup_args();
            arguments.push(legacy.to_owned());
            arguments.push("legacy".to_owned());
            assert!(
                Cli::try_parse_from(arguments).is_err(),
                "legacy option {legacy}"
            );
        }
    }

    fn initial_setup_values(root: &Path) -> NativeSetupValues {
        NativeSetupValues {
            database_path: root.join("data").join("forge.sqlite3"),
            custody_path: root.join("custody").join("forge.lock"),
            readiness_path: root.join("readiness").join("forge.json"),
            listener: NativeListenerConfig::new(
                101,
                202,
                303,
                404,
                NonZeroU32::new(3).unwrap(),
                NonZeroU32::new(4).unwrap(),
            ),
            native_run: NativeRunConfig::new(NativeRunConfigInput {
                claim_lease_ms: 505,
                poll_interval_ms: 506,
                retry_backoff_ms: 507,
                shutdown_budget_ms: 508,
                queue_capacity: 9,
                max_command_retries: 10,
                prompt_delivery: "queue".to_owned(),
                stream_after: 0,
            })
            .unwrap(),
        }
    }

    fn replacement_setup_values(root: &Path) -> NativeSetupValues {
        NativeSetupValues {
            database_path: root.join("data").join("replacement.sqlite3"),
            custody_path: root.join("custody").join("replacement.lock"),
            readiness_path: root.join("readiness").join("replacement.json"),
            listener: NativeListenerConfig::new(
                111,
                222,
                333,
                444,
                NonZeroU32::new(5).unwrap(),
                NonZeroU32::new(6).unwrap(),
            ),
            native_run: NativeRunConfig::new(NativeRunConfigInput {
                claim_lease_ms: 555,
                poll_interval_ms: 556,
                retry_backoff_ms: 557,
                shutdown_budget_ms: 558,
                queue_capacity: 11,
                max_command_retries: 12,
                prompt_delivery: "replacement".to_owned(),
                stream_after: 1,
            })
            .unwrap(),
        }
    }

    fn refused_setup_values(root: &Path) -> NativeSetupValues {
        NativeSetupValues {
            database_path: root.join("data").join("refused.sqlite3"),
            custody_path: root.join("custody").join("refused.lock"),
            readiness_path: root.join("readiness").join("refused.json"),
            listener: NativeListenerConfig::new(
                1,
                2,
                3,
                4,
                NonZeroU32::new(1).unwrap(),
                NonZeroU32::new(1).unwrap(),
            ),
            native_run: NativeRunConfig::new(NativeRunConfigInput {
                claim_lease_ms: 1,
                poll_interval_ms: 2,
                retry_backoff_ms: 3,
                shutdown_budget_ms: 4,
                queue_capacity: 1,
                max_command_retries: 1,
                prompt_delivery: "queue".to_owned(),
                stream_after: 0,
            })
            .unwrap(),
        }
    }

    fn assert_initial_native_config(layout: &Layout, config: &NativeInstanceConfig) {
        let credentials = ForgeCredentialPaths::from_home(&layout.root).unwrap();
        assert_eq!(config.credentials_manifest(), credentials.manifest_path());
        assert_eq!(config.listener().admission_timeout_ms(), 101);
        assert_eq!(config.listener().requests_per_connection().get(), 4);
        assert_eq!(config.native_run().claim_lease_ms(), 505);
        assert_eq!(config.native_run().poll_interval_ms(), 506);
        assert_eq!(config.native_run().retry_backoff_ms(), 507);
        assert_eq!(config.native_run().shutdown_budget_ms(), 508);
        assert_eq!(config.native_run().queue_capacity().get(), 9);
        assert_eq!(config.native_run().max_command_retries().get(), 10);
        assert_eq!(config.native_run().prompt_delivery(), "queue");
        assert_eq!(config.native_run().stream_after(), 0);
    }

    #[test]
    fn native_setup_writes_the_v2_instance_and_missing_config_is_typed() {
        let temporary = tempfile::tempdir().unwrap();
        let layout = Layout {
            manifest: temporary.path().join("installation.json"),
            root: temporary.path().join("Artisan Street"),
        };
        assert!(matches!(
            load_native_instance(&layout),
            Err(CliError::MissingInstance)
        ));

        setup_native(&layout, initial_setup_values(&layout.root)).unwrap();

        let config = load_native_instance(&layout).unwrap();
        assert_initial_native_config(&layout, &config);
        assert!(layout.native_instance_path().is_file());

        let instance_id = config.instance_id();
        setup_native(&layout, replacement_setup_values(&layout.root)).unwrap();
        assert_eq!(
            load_native_instance(&layout).unwrap().instance_id(),
            instance_id
        );

        let instance_path = layout.native_instance_path();
        let malformed = br#"{"schema":"artisan-instance-v2","version":1}"#;
        fs::write(&instance_path, malformed).unwrap();
        let before = fs::read(&instance_path).unwrap();
        let result = setup_native(&layout, refused_setup_values(&layout.root));
        assert!(matches!(result, Err(CliError::NativeInstance(_))));
        assert_eq!(fs::read(&instance_path).unwrap(), before);
    }

    #[test]
    fn native_setup_rejects_invalid_explicit_paths_before_provisioning() {
        let temporary = tempfile::tempdir().unwrap();
        let layout = Layout {
            manifest: temporary.path().join("installation.json"),
            root: temporary.path().join("Artisan Street"),
        };
        let values = NativeSetupValues {
            database_path: PathBuf::from("relative.sqlite3"),
            custody_path: layout.root.join("custody").join("forge.lock"),
            readiness_path: layout.root.join("readiness").join("forge.json"),
            listener: NativeListenerConfig::new(
                1,
                2,
                3,
                4,
                NonZeroU32::new(1).unwrap(),
                NonZeroU32::new(1).unwrap(),
            ),
            native_run: NativeRunConfig::new(NativeRunConfigInput {
                claim_lease_ms: 1,
                poll_interval_ms: 2,
                retry_backoff_ms: 3,
                shutdown_budget_ms: 4,
                queue_capacity: 1,
                max_command_retries: 1,
                prompt_delivery: "queue".to_owned(),
                stream_after: 0,
            })
            .unwrap(),
        };
        assert!(matches!(
            setup_native(&layout, values),
            Err(CliError::NativeInstance(_))
        ));
    }

    #[test]
    fn the_profile_flag_no_longer_exists() {
        // One Forge per Artisan home: naming an instance is not a concept.
        for arguments in [
            ["ae", "setup", "--profile", "default"],
            ["ae", "start", "--profile", "default"],
            ["ae", "open", "--profile", "default"],
        ] {
            assert!(Cli::try_parse_from(arguments).is_err());
        }
    }

    #[test]
    fn removal_of_data_is_explicit() {
        let cli = Cli::try_parse_from(["ae", "uninstall"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Uninstall { remove_data: false })
        ));
    }

    #[test]
    fn protocol_command_accepts_only_one_url_argument() {
        let cli =
            Cli::try_parse_from(["ae", "protocol", FORGE_START_LAUNCH_URL]).expect("protocol");
        assert!(matches!(
            cli.command,
            Some(Commands::Protocol { url }) if url == FORGE_START_LAUNCH_URL
        ));
        assert!(
            Cli::try_parse_from(["ae", "protocol", FORGE_START_LAUNCH_URL, "unexpected"]).is_err()
        );
    }

    #[test]
    fn protocol_decoder_rejects_every_non_capability_url() {
        let root = std::env::temp_dir().join("artisan-protocol-test-home");
        let layout = Layout {
            manifest: root.join("installation.json"),
            root,
        };
        for candidate in [
            "artisan://forge/start?command=calc",
            "artisan://forge/start#token",
            "artisan://forge/stop",
            "https://forge/start",
        ] {
            assert!(matches!(
                handle_protocol(&layout, candidate),
                Err(CliError::Control(message)) if message == "unsupported artisan:// launch request"
            ));
        }
    }

    #[test]
    fn project_root_is_not_a_supported_argument() {
        assert!(Cli::try_parse_from(["ae", "setup", "--project-root", "."]).is_err());
    }

    #[test]
    fn parsed_setup_debug_redacts_native_run_prompt_delivery() {
        let canary = "native-run-prompt-delivery-canary";
        let mut arguments = explicit_setup_args();
        replace_setup_value(&mut arguments, "--native-run-prompt-delivery", canary);
        let cli = Cli::try_parse_from(arguments).unwrap();
        let debug = format!("{cli:?}");
        assert!(!debug.contains(canary));
        assert!(debug.contains("NativeRunPromptDelivery"));
        assert!(debug.contains("byte_length:"));
        assert!(debug.contains("category: \"validated\""));
    }

    #[test]
    fn browser_origin_rejects_remote_and_credentialed_urls() {
        assert!(validate_origin("https://example.com").is_err());
        assert!(validate_origin("http://user@localhost").is_err());
        assert!(validate_origin("http://artisan-editor.localhost").is_ok());
        assert!(validate_origin("http://127.0.0.1:4317").is_ok());
    }

    #[test]
    fn browser_origin_defaults_to_the_live_forge_endpoint() {
        assert_eq!(
            resolve_browser_origin(None, "http://127.0.0.1:62244/").expect("Forge endpoint"),
            "http://127.0.0.1:62244"
        );
        assert_eq!(
            resolve_browser_origin(
                Some("http://artisan-editor.localhost"),
                "http://127.0.0.1:62244/"
            )
            .expect("explicit local forwarding origin"),
            "http://artisan-editor.localhost"
        );
    }
}
