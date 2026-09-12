//! Shared configured-turn request plumbing and runtime bounds helpers.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

use artisan_transport::CancelHandle;

use super::super::observation::EngineObservation;
use super::super::process::{ChildParts, CleanupObservation, StderrState, cleanup_after_abort};
use super::super::{EngineBounds, EngineLimits, InternalTurnInput};
use super::core::{EngineOperationError, Execution, PreparedSession, SteerDelivery, TurnResult};
pub(super) struct ConfiguredTurnRequest {
    pub(super) input: InternalTurnInput,
    pub(super) deadline: Instant,
    pub(super) control: Arc<CancelHandle>,
    pub(super) prepared: oneshot::Sender<Result<PreparedSession, EngineOperationError>>,
    pub(super) authorize: oneshot::Receiver<()>,
    pub(super) observations: mpsc::Sender<EngineObservation>,
    pub(super) respond: oneshot::Sender<TurnResult>,
    pub(super) steer_rx: Option<mpsc::Receiver<SteerDelivery>>,
}

impl ConfiguredTurnRequest {
    pub(super) fn fail(self, error: EngineOperationError) -> Execution {
        let _ = self.prepared.send(Err(error.clone()));
        let _ = self.respond.send(Err(error));
        Execution::Completed
    }
}

pub(super) struct ConfiguredRuntime {
    pub(super) limits: EngineLimits,
    pub(super) bounds: EngineBounds,
}

pub(super) fn configured_runtime(
    settings: &artisan_database::ThreadEngineSettings,
    control_capacity: usize,
) -> Result<ConfiguredRuntime, EngineOperationError> {
    let runtime = settings.config().runtime();
    let limits = EngineLimits {
        readiness: Duration::from_millis(runtime.readiness_budget().get()),
        health: Duration::from_millis(runtime.health_budget().get()),
        prompt: Duration::from_millis(runtime.prompt_budget().get()),
        sse: Duration::from_millis(runtime.stream_budget().get()),
        close: Duration::from_millis(runtime.close_budget().get()),
    };
    let bounds = EngineBounds {
        max_json_body: checked_usize(runtime.max_json_body_bytes().get())?,
        max_sse_line: checked_usize(runtime.max_sse_line_bytes().get())?,
        max_sse_event: checked_usize(runtime.max_sse_event_bytes().get())?,
        max_readiness_line: checked_usize(runtime.max_readiness_line_bytes().get())?,
        max_headers: checked_usize(runtime.max_header_count().get())?,
        max_buf_bytes: checked_usize(runtime.max_http_buffer_bytes().get())?,
        stderr_cap_bytes: checked_usize(runtime.max_stderr_bytes().get())?,
        sink_capacity: checked_usize(runtime.observation_capacity().get())?,
        control_capacity,
    };
    if bounds.max_buf_bytes < 8192
        || bounds.sink_capacity == 0
        || bounds.control_capacity == 0
        || bounds.max_json_body == 0
        || bounds.max_sse_line == 0
        || bounds.max_sse_event == 0
        || bounds.max_readiness_line == 0
        || bounds.max_headers == 0
        || bounds.stderr_cap_bytes == 0
    {
        return Err(EngineOperationError::Configuration);
    }
    if tokio::time::Instant::now()
        .checked_add(limits.readiness)
        .is_none()
        || tokio::time::Instant::now()
            .checked_add(limits.health)
            .is_none()
        || tokio::time::Instant::now()
            .checked_add(limits.prompt)
            .is_none()
        || tokio::time::Instant::now()
            .checked_add(limits.sse)
            .is_none()
        || tokio::time::Instant::now()
            .checked_add(limits.close)
            .is_none()
    {
        return Err(EngineOperationError::Configuration);
    }
    Ok(ConfiguredRuntime { limits, bounds })
}

pub(super) fn checked_usize(value: u64) -> Result<usize, EngineOperationError> {
    usize::try_from(value).map_err(|_| EngineOperationError::Configuration)
}

pub(super) fn phase_deadline(budget: Duration, attempt_deadline: Instant) -> Instant {
    Instant::now()
        .checked_add(budget)
        .map_or(attempt_deadline, |candidate| {
            candidate.min(attempt_deadline)
        })
}

pub(super) async fn wait_for_authorization(
    parts: &mut ChildParts,
    authorize: &mut oneshot::Receiver<()>,
    deadline: Instant,
    shutdown: &Arc<CancelHandle>,
    control: &Arc<CancelHandle>,
) -> Result<(), EngineOperationError> {
    loop {
        tokio::select! {
            biased;
            () = shutdown.wait() => return Err(EngineOperationError::Shutdown),
            () = control.wait() => return Err(EngineOperationError::Cancelled),
            () = tokio::time::sleep_until(deadline) => return Err(EngineOperationError::Deadline),
            result = &mut *authorize => {
                return result.map_err(|_| EngineOperationError::ProviderRequestFailed);
            }
            event = parts.stderr_counter.pump(), if parts.stderr_counter.state() == StderrState::Open => {
                let _ = event;
            }
        }
    }
}

pub(super) async fn finish_turn_result(
    parts: ChildParts,
    result: TurnResult,
    respond: oneshot::Sender<TurnResult>,
    close_budget: Duration,
) -> Execution {
    if result.is_err() {
        return match cleanup_after_abort(parts, close_budget).await {
            CleanupObservation::ReapedWithoutKill(_) | CleanupObservation::ReapedAfterKill(_) => {
                let _ = respond.send(result);
                Execution::Completed
            }
            CleanupObservation::Retained(engine) => {
                let primary = result
                    .err()
                    .map_or_else(|| Box::new(EngineOperationError::ReapUnresolved), Box::new);
                let _ = respond.send(Err(EngineOperationError::UnresolvedReapDuring { primary }));
                Execution::Quarantined(engine)
            }
        };
    }

    let ChildParts {
        mut child,
        mut lifeline,
        stdout: _,
        stderr_counter,
    } = parts;
    lifeline.close();
    let first_wait = match tokio::time::Instant::now().checked_add(close_budget) {
        Some(deadline) => tokio::time::timeout_at(deadline, child.wait())
            .await
            .unwrap_or_else(|_| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "turn close budget elapsed",
                ))
            }),
        None => Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "turn close budget unrepresentable",
        )),
    };
    if let Ok(status) = first_wait {
        #[cfg(test)]
        super::super::process::note_observed_reap_for_tests(status);
        #[cfg(not(test))]
        let _ = status;
        drop(stderr_counter);
        drop(lifeline);
        let _ = respond.send(result);
        return Execution::Completed;
    }

    let parts = ChildParts {
        child,
        lifeline,
        stdout: None,
        stderr_counter,
    };
    match cleanup_after_abort(parts, Duration::ZERO).await {
        CleanupObservation::ReapedWithoutKill(status)
        | CleanupObservation::ReapedAfterKill(status) => {
            #[cfg(test)]
            super::super::process::note_observed_reap_for_tests(status);
            #[cfg(not(test))]
            let _ = status;
            let _ = respond.send(result);
            Execution::Completed
        }
        CleanupObservation::Retained(engine) => {
            let primary = result
                .err()
                .map_or_else(|| Box::new(EngineOperationError::ReapUnresolved), Box::new);
            let _ = respond.send(Err(EngineOperationError::UnresolvedReapDuring { primary }));
            Execution::Quarantined(engine)
        }
    }
}

pub(super) async fn finish_configured_start(
    request: ConfiguredTurnRequest,
    parts: ChildParts,
    error: EngineOperationError,
    close_budget: Duration,
) -> Execution {
    let ConfiguredTurnRequest {
        prepared, respond, ..
    } = request;
    let _ = prepared.send(Err(error.clone()));
    finish_turn_result(parts, Err(error), respond, close_budget).await
}
