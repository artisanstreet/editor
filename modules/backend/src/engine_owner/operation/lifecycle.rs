//! Job lifecycle: legacy readiness/health execution, intake checks,
//! one configured turn dispatch, and abort/success settlement.

use std::sync::Arc;
use std::time::Duration;

use artisan_transport::CancelHandle;
use tokio::sync::oneshot;
use tokio::time::Instant;

use super::super::EngineBounds;
use super::super::EngineLimits;
use super::super::http::HealthSecret;
use super::super::process::ChildParts;
use super::super::process::CleanupObservation;
use super::super::process::LaunchRecipe;
use super::super::process::LifelineWriter;
use super::super::process::StderrCounter;
use super::super::process::cleanup_after_abort;
use super::super::process::spawn_engine;
use super::super::readiness::ReadinessError;
use super::bootstrap::HealthPhaseCtx;
use super::bootstrap::drive_readiness;
use super::bootstrap::handle_health_phase;
use super::claude::execute_claude_turn;
use super::codex::execute_codex_turn;
use super::core::EngineOperationError;
use super::core::Execution;
use super::core::Job;
use super::core::LaunchOutcome;
use super::core::LaunchResult;
use super::cursor::execute_cursor_turn;
use super::failures::map_readiness_error;
use super::grok::execute_grok_turn;
use super::hermes::execute_hermes_turn;
use super::hermes::map_hermes_turn_error;
use super::opencode::execute_configured_turn;
use super::turn_common::ConfiguredTurnRequest;
use super::turn_common::configured_runtime;

/// Executes one legacy readiness/health job end to end.  This path remains
/// available only for the existing owner tests; configured production turns
/// use the immutable snapshot path below.
pub(super) async fn execute_legacy_job(
    recipe: &LaunchRecipe,
    generation: u64,
    job: Job,
    shutdown: &Arc<CancelHandle>,
    limits: EngineLimits,
    bounds: EngineBounds,
) -> Execution {
    let Job::Legacy {
        run_id: _,
        deadline,
        control,
        respond,
    } = job
    else {
        unreachable!("legacy executor received a configured turn");
    };
    if shutdown.is_cancelled() {
        let _ = respond.send(Err(EngineOperationError::Shutdown));
        return Execution::Completed;
    }
    if control.is_cancelled() {
        let _ = respond.send(Err(EngineOperationError::Cancelled));
        return Execution::Completed;
    }
    if Instant::now() >= deadline {
        let _ = respond.send(Err(EngineOperationError::Deadline));
        return Execution::Completed;
    }
    let Ok(secret) = HealthSecret::generate() else {
        let _ = respond.send(Err(EngineOperationError::EntropyFailed));
        return Execution::Completed;
    };
    let Ok(spawned) = spawn_engine(recipe, secret.as_str()) else {
        let _ = respond.send(Err(EngineOperationError::SpawnFailed));
        return Execution::Completed;
    };
    let mut child = spawned;
    let lifeline = LifelineWriter::take(&mut child);
    let maybe_stdout = child.stdout.take();
    let stderr_counter = StderrCounter::new(child.stderr.take(), bounds.stderr_cap_bytes);
    let Some(mut stdout) = maybe_stdout else {
        let parts = ChildParts {
            child,
            lifeline,
            stdout: None,
            stderr_counter,
        };
        return finish_aborted(
            parts,
            EngineOperationError::ReadinessFailed(ReadinessError::Io),
            respond,
            limits.close,
        )
        .await;
    };
    let mut parts = ChildParts {
        child,
        lifeline,
        stdout: None,
        stderr_counter,
    };
    let readiness_deadline = std::cmp::min(
        Instant::now()
            .checked_add(limits.readiness)
            .unwrap_or(deadline),
        deadline,
    );
    let endpoint_result = drive_readiness(
        &mut stdout,
        &mut parts,
        readiness_deadline,
        shutdown,
        &control,
        bounds.max_readiness_line,
    )
    .await;
    let endpoint = match endpoint_result {
        Ok(ep) => ep,
        Err(e) => {
            let mapped = map_readiness_error(e);
            drop(stdout);
            return finish_aborted(parts, mapped, respond, limits.close).await;
        }
    };
    drop(stdout);
    let ctx = HealthPhaseCtx {
        limits,
        bounds,
        deadline,
        control: &control,
        shutdown,
    };
    handle_health_phase(parts, generation, endpoint, secret, respond, ctx).await
}

/// Per-engine image-attachment applicability enforced at owner intake.
///
/// Mirrors the TypeScript `image_input` evidence
/// (`docs/plans/native-engines/README.md` section 1): Codex data-URL images,
/// Claude base64 blocks, `OpenCode2` per-model data-URI, Grok embedded
/// resources, and Cursor native blocks are provider-supported, so those turns
/// pass intake unchanged here. Hermes reports `image_input: false`, so any
/// image fails the turn closed with the existing typed
/// [`HermesTurnError::ImagesUnsupported`](super::super::hermes::HermesTurnError)
/// reject instead of sending a degraded text-only prompt. Runnable catalog
/// support never implies an installed binary: executable resolution and the
/// readiness handshake stay the live gate in each per-engine executor.
pub(super) fn check_turn_attachment_applicability(
    engine: artisan_domain::EngineId,
    prompt: &artisan_domain::QueueMessagePayload,
) -> Result<(), EngineOperationError> {
    match engine {
        artisan_domain::EngineId::Hermes => {
            super::super::hermes::reject_image_attachments(prompt)
                .map_err(|error| map_hermes_turn_error(&error))
        }
        artisan_domain::EngineId::OpenCode2
        | artisan_domain::EngineId::Codex
        | artisan_domain::EngineId::Claude
        | artisan_domain::EngineId::Grok
        | artisan_domain::EngineId::Cursor => Ok(()),
    }
}

/// Executes one configured `OpenCode2` turn.  The profile capability and the
/// settings snapshot are moved into this owner call and are never reread from
/// durable state or ambient process configuration.
pub(super) async fn execute_configured_job(job: Job, shutdown: &Arc<CancelHandle>) -> Execution {
    let Job::Turn {
        input,
        deadline,
        control,
        prepared,
        authorize,
        observations,
        respond,
        steer_rx,
    } = job
    else {
        unreachable!("configured executor received a legacy launch");
    };

    let request = ConfiguredTurnRequest {
        input: *input,
        deadline,
        control,
        prepared,
        authorize,
        observations,
        respond,
        steer_rx,
    };
    let runtime = match configured_runtime(&request.input.settings, request.input.control_capacity)
    {
        Ok(runtime) => runtime,
        Err(error) => return request.fail(error),
    };
    let selected_profile = match request.input.settings.config().selection() {
        artisan_domain::EngineSelection::OpenCode2(selection) => selection.profile_id().as_str(),
        artisan_domain::EngineSelection::Codex(selection) => selection.profile_id().as_str(),
        artisan_domain::EngineSelection::Claude(selection) => selection.profile_id().as_str(),
        artisan_domain::EngineSelection::Grok(selection) => selection.profile_id().as_str(),
        artisan_domain::EngineSelection::Cursor(selection) => selection.profile_id().as_str(),
        artisan_domain::EngineSelection::Hermes(selection) => selection.profile_id().as_str(),
    };
    if request.input.launch.profile_id() != selected_profile {
        return request.fail(EngineOperationError::Configuration);
    }
    if let Err(error) = check_turn_attachment_applicability(
        request.input.settings.config().selection().engine_id(),
        &request.input.prompt,
    ) {
        return request.fail(error);
    }
    if shutdown.is_cancelled() {
        return request.fail(EngineOperationError::Shutdown);
    }
    if request.control.is_cancelled() {
        return request.fail(EngineOperationError::Cancelled);
    }

    // The live dispatch asks the provider-neutral socket seam for the engine
    // identity instead of matching `InternalLaunch` variants directly. The
    // adapters borrow the admitted capability, so the launch is neither
    // cloned nor re-resolved and the executor keeps custody.
    let dispatch_engine = super::super::socket::adapter_for(&request.input.launch)
        .descriptor()
        .id;
    match dispatch_engine.as_str() {
        super::super::consts::CODEX_ENGINE_ID => {
            Box::pin(execute_codex_turn(request, runtime, shutdown)).await
        }
        super::super::consts::CLAUDE_ENGINE_ID => {
            Box::pin(execute_claude_turn(request, runtime, shutdown)).await
        }
        super::super::consts::GROK_ENGINE_ID => {
            Box::pin(execute_grok_turn(request, runtime, shutdown)).await
        }
        super::super::consts::CURSOR_ENGINE_ID => {
            Box::pin(execute_cursor_turn(request, runtime, shutdown)).await
        }
        super::super::consts::HERMES_ENGINE_ID => {
            Box::pin(execute_hermes_turn(request, runtime, shutdown)).await
        }
        // `OpenCode2` and the test-only fixture lane share the configured
        // executor, exactly as before.
        _ => Box::pin(execute_configured_turn(request, runtime, shutdown)).await,
    }
}

/// Runs the fixed cleanup for an aborted or faulted launch and settles the
/// response honestly.
pub(super) async fn finish_aborted(
    parts: ChildParts,
    cause: EngineOperationError,
    respond: oneshot::Sender<LaunchResult>,
    close_budget: Duration,
) -> Execution {
    match cleanup_after_abort(parts, close_budget).await {
        CleanupObservation::ReapedWithoutKill(_status)
        | CleanupObservation::ReapedAfterKill(_status) => {
            let _ = respond.send(Err(cause));
            Execution::Completed
        }
        CleanupObservation::Retained(engine) => {
            let _ = respond.send(Err(EngineOperationError::UnresolvedReapDuring {
                primary: Box::new(cause),
            }));
            Execution::Quarantined(engine)
        }
    }
}

/// Graceful teardown after successful readiness and health.
///
/// Closes the lifeline and waits up to `close_budget` for the child to
/// exit. A prompt reap is expected without a kill on the fixture path;
/// fallback kill preserves quarantine guarantees.
pub(super) async fn finish_success(
    parts: ChildParts,
    generation: u64,
    respond: oneshot::Sender<LaunchResult>,
    close_budget: Duration,
) -> Execution {
    let ChildParts {
        mut child,
        mut lifeline,
        stdout: _,
        stderr_counter,
    } = parts;
    lifeline.close();
    let start = Instant::now();
    let deadline = start.checked_add(close_budget);
    let first_wait = match deadline {
        Some(d) => match tokio::time::timeout_at(d, child.wait()).await {
            Ok(res) => res,
            Err(_) => Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "close budget elapsed",
            )),
        },
        None => Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "close budget unrepresentable",
        )),
    };
    if let Ok(status) = first_wait {
        #[cfg(test)]
        super::super::process::note_observed_reap_for_tests(status);
        let outcome = LaunchOutcome::ObservedExit {
            generation,
            success: status.success(),
        };
        drop(stderr_counter);
        drop(lifeline);
        let _ = respond.send(Ok(outcome));
        Execution::Completed
    } else {
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
                let outcome = LaunchOutcome::ObservedExit {
                    generation,
                    success: status.success(),
                };
                let _ = respond.send(Ok(outcome));
                Execution::Completed
            }
            CleanupObservation::Retained(engine) => {
                let _ = respond.send(Err(EngineOperationError::ReapUnresolved));
                Execution::Quarantined(engine)
            }
        }
    }
}
