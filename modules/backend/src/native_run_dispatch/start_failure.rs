//! Provider startup: the launch deadline and honest launch failures.
//!
//! After `launch_claimed_run` the dispatcher waits for the provider to start
//! and announce its session. That wait has its own explicit deadline
//! (`launch_deadline`), independent of the claim lease the heartbeat keeps
//! alive. When the provider fails to start or misses the deadline the owner
//! turn is torn down and the launched run is failed durably with a typed,
//! reader-facing reason, instead of being left for lease-expiry recovery to
//! report an unknown outcome.

use std::time::Duration;

use artisan_database::{
    DispatchFailureReason, FailUnstartedRun, LaunchedRunReceipt, RunErrorCode, RunErrorMessage,
};
use artisan_domain::EngineId;
use artisan_transport::CancelHandle;

use crate::engine_owner::operation::{AcceptedTurn, EngineOperationError, PreparedSession};

use super::dispatch_support::{at_or_after, mint_patch_id};
use super::{ClaimExecution, ClaimIds};

/// Why a launched provider never started.
#[derive(Debug)]
pub(super) enum StartFailure {
    /// The engine owner refused to admit the turn.
    NotAdmitted,
    /// The provider did not announce its session within the launch deadline.
    DeadlineElapsed(Duration),
    /// The owner reported a typed startup failure.
    Failed(EngineOperationError),
}

/// Result of waiting for the provider to start.
pub(super) enum ProviderStart {
    /// The provider announced its session; `stopped` records a stop request
    /// observed meanwhile (the session is still bound so the stop settles).
    Started {
        session: PreparedSession,
        stopped: bool,
    },
    /// The provider never started.
    Failed(StartFailure),
}

/// Waits for the provider session within `launch_deadline`.
///
/// A stop request wins without dropping the turn: setup continues until the
/// session exists (still within the deadline) so the durable bind gives the
/// stop an authenticated terminal path.
pub(super) async fn await_provider_start(
    turn: &mut AcceptedTurn,
    run_cancel: &CancelHandle,
    launch_deadline: Duration,
) -> ProviderStart {
    enum Wait<T> {
        Prepared(T),
        Stopped,
        Elapsed,
    }
    let deadline = tokio::time::Instant::now() + launch_deadline;
    let wait = tokio::select! {
        biased;
        result = turn.prepare() => Wait::Prepared(result),
        () = run_cancel.wait() => Wait::Stopped,
        () = tokio::time::sleep_until(deadline) => Wait::Elapsed,
    };
    let (prepared, stopped) = match wait {
        Wait::Prepared(result) => (Some(result), run_cancel.is_cancelled()),
        Wait::Stopped => (
            tokio::time::timeout_at(deadline, turn.prepare()).await.ok(),
            true,
        ),
        Wait::Elapsed => (None, run_cancel.is_cancelled()),
    };
    match prepared {
        Some(Ok(session)) => ProviderStart::Started { session, stopped },
        Some(Err(error)) => ProviderStart::Failed(StartFailure::Failed(error)),
        None => ProviderStart::Failed(StartFailure::DeadlineElapsed(launch_deadline)),
    }
}

fn engine_name(engine: EngineId) -> &'static str {
    match engine {
        EngineId::OpenCode2 => "OpenCode",
        EngineId::Codex => "Codex",
        EngineId::Claude => "Claude",
        EngineId::Cursor => "Cursor",
        EngineId::Grok => "Grok",
    }
}

fn format_deadline(deadline: Duration) -> String {
    if deadline.subsec_millis() == 0 {
        format!("{} s", deadline.as_secs())
    } else {
        format!("{} ms", deadline.as_millis())
    }
}

/// Bounded run error code and reader-facing text for one startup failure.
pub(super) fn describe(engine: EngineId, failure: &StartFailure) -> (&'static str, String) {
    let name = engine_name(engine);
    let error = match failure {
        StartFailure::NotAdmitted => {
            return (
                "provider_start_failed",
                format!("{name} could not be started: the engine owner is unavailable."),
            );
        }
        StartFailure::DeadlineElapsed(deadline) => {
            return (
                "provider_start_timeout",
                format!(
                    "{name} did not start within {}.",
                    format_deadline(*deadline)
                ),
            );
        }
        StartFailure::Failed(EngineOperationError::UnresolvedReapDuring { primary }) => primary,
        StartFailure::Failed(error) => error,
    };
    let cause = match error {
        EngineOperationError::Cancelled => {
            return (
                "provider_start_cancelled",
                format!("The run was stopped before {name} started."),
            );
        }
        EngineOperationError::Shutdown => {
            return (
                "provider_start_interrupted",
                format!("The Forge shut down before {name} started."),
            );
        }
        EngineOperationError::SpawnFailed => "its executable could not be launched",
        EngineOperationError::Configuration => "this run's engine settings were rejected",
        EngineOperationError::Deadline => "the run's attempt budget elapsed",
        EngineOperationError::IncompatibleVersion => "its version is not supported",
        EngineOperationError::ReadinessFailed(_) | EngineOperationError::HealthFailed(_) => {
            "it never became ready"
        }
        EngineOperationError::ProviderRequestFailed | EngineOperationError::FrameTooLarge => {
            "it exited or refused the session before announcing it"
        }
        _ => "its process failed during startup",
    };
    (
        "provider_start_failed",
        format!("{name} failed to start: {cause}."),
    )
}

/// Fails the launched run and its dispatch with the startup failure.
///
/// Called while the claim heartbeat still holds the lease and after the owner
/// turn was torn down, so the outcome is known. A fence miss (the pair moved
/// on) or a persistent write failure leaves the pair to lease recovery.
pub(super) async fn settle_unstarted_claim(
    context: &ClaimExecution<'_>,
    ids: &ClaimIds,
    receipt: &LaunchedRunReceipt,
    engine: EngineId,
    failure: &StartFailure,
) {
    let (code, message) = describe(engine, failure);
    let (Ok(error_code), Ok(error_message), Ok(dispatch_reason)) = (
        RunErrorCode::parse(code.to_owned()),
        RunErrorMessage::parse(message.clone()),
        DispatchFailureReason::parse(message),
    ) else {
        return;
    };
    let Some(turn_patch_id) = mint_patch_id(context.origin) else {
        return;
    };
    for _ in 0..context.config.max_command_retries.get() {
        let Some(operated_at) = at_or_after(context.origin, ids.operated_at) else {
            return;
        };
        let outcome = context
            .repository
            .fail_unstarted_run(FailUnstartedRun {
                claimed: &context.claimed,
                receipt,
                run_start_key: &ids.run_start_key,
                credentials: &ids.credentials,
                operated_at,
                turn_patch_id: &turn_patch_id,
                error_code: &error_code,
                error_message: &error_message,
                dispatch_reason: &dispatch_reason,
            })
            .await;
        if outcome.is_ok() {
            let _ = context
                .config
                .conversation_commit_notifier()
                .publish(&receipt.thread_id);
            context.config.notifier.wake_any();
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use artisan_domain::EngineId;

    use super::{EngineOperationError, StartFailure, describe};

    #[test]
    fn startup_failures_name_the_engine_and_the_cause() {
        assert_eq!(
            describe(
                EngineId::Claude,
                &StartFailure::DeadlineElapsed(Duration::from_secs(120))
            ),
            (
                "provider_start_timeout",
                "Claude did not start within 120 s.".to_owned()
            )
        );
        assert_eq!(
            describe(
                EngineId::Codex,
                &StartFailure::Failed(EngineOperationError::UnresolvedReapDuring {
                    primary: Box::new(EngineOperationError::SpawnFailed),
                })
            ),
            (
                "provider_start_failed",
                "Codex failed to start: its executable could not be launched.".to_owned()
            )
        );
        assert_eq!(
            describe(
                EngineId::Claude,
                &StartFailure::Failed(EngineOperationError::Cancelled)
            ),
            (
                "provider_start_cancelled",
                "The run was stopped before Claude started.".to_owned()
            )
        );
    }
}
