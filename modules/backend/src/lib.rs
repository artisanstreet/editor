//! Forge application assembly boundary.

use std::{future::Future, process::ExitCode, sync::Arc};

use artisan_transport::CancelHandle;

pub mod activated_conversation_replay;
pub mod agent_name_allocation_policy;
pub mod agent_name_catalog_policy;
pub mod agent_orchestrator_start_failure_policy;
pub mod app;
pub mod builtin_tool_capabilities_policy;
pub mod command_admission;
pub mod connection;
pub mod conversation_commit_notifier;
pub mod conversation_delivery_writer;
pub mod conversation_subscription_preparation;
pub mod conversation_subscription_registry;
pub mod credential_authority;
pub mod directory_controller;
pub mod directory_helper;
pub(crate) mod directory_helper_codec;
pub mod directory_selection;
pub mod engine_owner;
pub mod file_identity_policy;
pub mod forge_runtime;
pub mod git_remote_url_policy;
pub mod graph_advancement_policy;
pub mod harness_config_registry_policy;
pub mod host_identity_policy;
pub mod host_machines_policy;
pub mod host_suspend_detection_policy;
pub mod lifecycle_control;
pub mod listener;
pub mod native_run_dispatch;
pub mod orchestration_intake_policy;
pub mod preview_service_policy;
pub mod process_custody;
pub mod product_telemetry_capture_policy;
pub mod request_handler;
pub mod sqlite_write_retry_policy;
pub mod startup_reconciliation_sweep;
pub mod storage;
pub mod subscription_patch_selection_policy;
pub mod telemetry_preferences_control_policy;
pub mod terminal_transcript_consumption_policy;
pub mod thread_liveness_policy;
pub mod thread_metadata_refiner_policy;
pub mod thread_resource_quiescence_policy;
pub mod usage_interruption_model_policy;
pub mod wake_lock_policy;

#[cfg(test)]
#[path = "../../../tests/backend/native_run_dispatch.rs"]
mod native_run_dispatch_tests;

pub use process_custody::{ForgeProcessCustody, ForgeProcessCustodyError};

pub use app::{ForgeApp, ForgeConfig, ForgeShutdownError, ForgeStartupError};
pub use command_admission::{
    CommandOrigin, CommandOriginClockError, CommandOriginEntropyError, SystemCommandOrigin,
};
pub use connection::{
    AuthenticationStageError, ConnectionLimits, ForgeConnection, RequestStageError,
    ServerFrameStamp, WelcomeMetadata,
};
pub use credential_authority::{
    AuthenticatedCredential, CredentialAuthenticationError, CredentialAuthority,
    CredentialEntropyError, CredentialKind, PendingReconnect, ReconnectRotationError,
};
pub use directory_controller::{
    AdmissionError, ControllerStartError, DirectoryController, DirectoryControllerConfig,
    DirectoryPickOutcome, HealthState, HelperOperationError, OperationResult, PickOperation,
    ShutdownReport,
};
// The directory helper keeps its surface private to this crate: only
// `directory_helper::run_if_requested` is public, for `main` composition.
pub use directory_selection::{
    DirectorySelectionAdmissionError, IssuedDirectory, MAX_LIFETIME_ISSUED_IDENTITIES,
    MAX_LIVE_SELECTIONS, SELECTION_TIME_TO_LIVE, SelectedDirectory, SelectedDirectoryAuthority,
};
pub use forge_runtime::{
    CredentialMaterialError, ForgeCleanupError, ForgeConfigError, ForgeLaunchConfig,
    ForgePrimaryCleanupError, ForgeRuntimeError, ForgeServiceError, ReadinessError,
};
pub use lifecycle_control::LifecycleController;
pub use listener::{
    AdmissionCause, ForgeListener, ListenerError, ListenerLimits, MetadataError,
    RequestTermination, ServiceReport,
};
pub use native_run_dispatch::{
    NativeRunDispatcherConfig, NativeRunDispatcherConfigError, NativeRunDispatcherShutdown,
};
pub use request_handler::RequestHandler;
pub use storage::{ForgeStorage, ForgeStorageCloseError, ForgeStorageOpenError};

/// Runs the synchronous Forge binary boundary.
///
/// The helper dispatch remains in `main` so this function is reached only for
/// normal Forge startup. It owns one current-thread runtime, parses only the
/// explicit long-form launch contract, and keeps the caller-owned cancellation
/// handle attached to the signal and process futures.
#[must_use]
pub fn run() -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("Forge runtime construction failed: {error}");
            return ExitCode::from(forge_runtime::EXIT_CODE_SHUTDOWN);
        }
    };

    let cancel = Arc::new(CancelHandle::new());
    let config = match forge_runtime::parse_args(std::env::args_os().skip(1), Arc::clone(&cancel)) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(forge_runtime::EXIT_CODE_CONFIGURATION);
        }
    };

    let result = runtime.block_on(run_with_signal(config, cancel));
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(error.exit_code())
        }
    }
}

async fn run_with_signal(
    config: ForgeLaunchConfig,
    cancel: Arc<CancelHandle>,
) -> Result<(), ForgeRuntimeError> {
    let signal = tokio::signal::ctrl_c();
    tokio::pin!(signal);
    let process = forge_runtime::run(config);
    tokio::pin!(process);

    let mut cancellation_seen = false;
    loop {
        if cancellation_seen {
            return process.await;
        }
        tokio::select! {
            biased;

            signal_result = &mut signal => match signal_result {
                Ok(()) => {
                    cancel.cancel();
                    cancellation_seen = true;
                }
                Err(error) => {
                    return finish_after_signal_failure(cancel.as_ref(), process.as_mut(), error)
                        .await;
                }
            },
            result = &mut process => return result,
        }
    }
}

async fn finish_after_signal_failure<F>(
    cancel: &CancelHandle,
    process: F,
    signal_error: std::io::Error,
) -> Result<(), ForgeRuntimeError>
where
    F: Future<Output = Result<(), ForgeRuntimeError>>,
{
    // Signal registration/observation failed, but the already-owned process
    // future must still run its full cancellation and cleanup path before the
    // signal remains the primary exit-73 failure.
    cancel.cancel();
    match process.await {
        Ok(()) => Err(ForgeRuntimeError::Signal(signal_error)),
        Err(cleanup) => Err(ForgeRuntimeError::with_cleanup(
            ForgeRuntimeError::Signal(signal_error),
            vec![cleanup],
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        path::PathBuf,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
    };

    use super::*;

    #[test]
    fn signal_failure_awaits_process_and_retains_signal_as_primary() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("signal test runtime should build");
        let cancel = Arc::new(CancelHandle::new());
        let observed = Arc::new(AtomicBool::new(false));
        let process_cancel = Arc::clone(&cancel);
        let process_observed = Arc::clone(&observed);
        let process = async move {
            assert!(process_cancel.is_cancelled());
            process_observed.store(true, Ordering::Relaxed);
            Err(ForgeRuntimeError::ReadinessCleanup(
                forge_runtime::ReadinessError::TargetReplaced {
                    path: PathBuf::from("/absolute/ready.json"),
                },
            ))
        };

        let result = runtime.block_on(finish_after_signal_failure(
            cancel.as_ref(),
            process,
            std::io::Error::other("signal registration failed"),
        ));
        let error = result.expect_err("signal failure should be returned");
        assert!(observed.load(Ordering::Relaxed));
        assert_eq!(error.exit_code(), forge_runtime::EXIT_CODE_SHUTDOWN);
        let composite = error
            .as_primary_with_cleanup()
            .expect("signal must remain primary over process cleanup");
        assert!(matches!(composite.primary(), ForgeRuntimeError::Signal(_)));
        assert!(matches!(
            composite.cleanup_failures(),
            [ForgeRuntimeError::ReadinessCleanup(_)]
        ));
    }
}
