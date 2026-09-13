//! Grok executor seam: one finite `grok agent stdio` ACP turn, its pump
//! loop, bridge bookkeeping, and teardown helpers.

use std::sync::Arc;
use std::time::Duration;

use artisan_domain::RunId;
use artisan_transport::CancelHandle;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::time::Instant;

use super::super::observation::EngineObservation;
use super::super::observation::TerminalState;
use super::super::process::StderrCounter;
use super::core::EngineOperationError;
use super::core::EngineTurnResult;
use super::core::Execution;
use super::core::PreparedSession;
use super::core::TurnResult;
use super::turn_common::ConfiguredRuntime;
use super::turn_common::ConfiguredTurnRequest;
use super::turn_common::phase_deadline;

/// Executes one finite Grok turn over `grok agent stdio` through the shared
/// ACP core.
///
/// Single-owner match arm beside the Codex/Claude executors: no second task,
/// no second queue, no loop fork. Performs the bounded `initialize`
/// handshake with the row's auth classifier, `session/new` (or
/// `session/load` for a gated continuation that reopens the same provider
/// conversation), the bind authorization gate carrying the exact native
/// session identity, then exactly one prompt with the update pump. Streaming
/// session updates carry no root-text projection in G3 (a later packet);
/// per-round usage projects best-effort onto the shared usage vocabulary
/// without blocking the turn; permission and elicitation agent requests
/// normalize through the A2 bridges into the pending table with no
/// control-flow side effect and are never auto-answered. EOF before the
/// prompt result maps to `Interrupted`, explicit cancel to `Cancelled`, and
/// stall/failure to `Failed`. Teardown closes the stdin lifeline first,
/// terminates the whole process group (no orphaned grok grandchildren
/// holding pipes), and reaps within the close budget; an unobserved reap
/// reports `UnresolvedReapDuring` without owner quarantine (see
/// `finish_grok_turn`).
#[allow(clippy::too_many_lines)]
pub(super) async fn execute_grok_turn(
    request: ConfiguredTurnRequest,
    runtime: ConfiguredRuntime,
    shutdown: &Arc<CancelHandle>,
) -> Execution {
    use super::super::acp as acp_core;
    use super::super::grok as grok_runtime;

    let artisan_domain::EngineSelection::Grok(selection) =
        request.input.settings.config().selection()
    else {
        return request.fail(EngineOperationError::Configuration);
    };
    let settings = grok_runtime::GrokSettings::from_selection(selection);
    if settings.profile_id() != request.input.launch.profile_id() {
        return request.fail(EngineOperationError::Configuration);
    }
    let super::super::InternalLaunch::Grok(launch) = &request.input.launch else {
        return request.fail(EngineOperationError::Configuration);
    };
    // G3 continuation gate: same-engine is fenced by the dispatcher (grok
    // bindings only); the owner additionally requires an explicit target
    // model and a recorded CLI version. Anything else is typed
    // incompatible — never a silent fresh start and never a cross-engine
    // resume.
    let resume_stored_session_id: Option<String> = match &request.input.continuation {
        None => None,
        Some(continuation) => {
            let gate = grok_runtime::check_grok_native_continuation(
                &grok_runtime::GrokContinuationGateInput {
                    cli_version: launch.version(),
                    target_model: selection
                        .model_id()
                        .map(artisan_domain::EngineModelId::as_str),
                    advertised_models: None,
                    same_engine: true,
                },
            );
            if !matches!(gate, grok_runtime::GrokContinuationDecision::Compatible) {
                return request.fail(EngineOperationError::Configuration);
            }
            Some(continuation.provider_session_id().to_owned())
        }
    };
    let definition = grok_runtime::GrokSettings::definition();
    let argv = (definition.build_args)(&settings.launch_args());
    let Ok(mut child) = acp_core::spawn_acp_child(
        launch.executable_path().as_os_str(),
        &argv,
        Some(std::path::Path::new(request.input.project_root.as_str())),
    ) else {
        return request.fail(EngineOperationError::SpawnFailed);
    };
    let Some(pipes) = child.take_pipes() else {
        let ConfiguredTurnRequest {
            prepared, respond, ..
        } = request;
        let _ = prepared.send(Err(EngineOperationError::SpawnFailed));
        return finish_grok_turn(
            child,
            Err(EngineOperationError::SpawnFailed),
            respond,
            runtime.limits.close,
        )
        .await;
    };
    let acp_core::AcpPipes {
        stdin,
        stdout,
        stderr,
    } = pipes;
    let mut stderr_counter = StderrCounter::new(Some(stderr), runtime.bounds.stderr_cap_bytes);
    // Owner bounds map onto the caller-supplied ACP transport bounds: the
    // SSE line ceiling bounds NDJSON lines, the generic JSON ceiling bounds
    // envelopes, the handshake window is the prompt budget, and the stream
    // budget arms the inactivity deadline the update loop recomputes.
    let Ok(bounds) = acp_core::AcpBounds::new(
        runtime.bounds.max_sse_line,
        runtime.bounds.max_json_body,
        grok_runtime::GROK_MAX_SESSION_ID_BYTES,
        runtime.limits.prompt,
        runtime.limits.sse,
        runtime.limits.close,
    ) else {
        let ConfiguredTurnRequest {
            prepared, respond, ..
        } = request;
        let _ = prepared.send(Err(EngineOperationError::Configuration));
        return finish_grok_turn(
            child,
            Err(EngineOperationError::Configuration),
            respond,
            runtime.limits.close,
        )
        .await;
    };
    let mut transport = acp_core::AcpTransport::new(stdout, stdin, bounds);

    // initialize ----------------------------------------------------------
    let handshake_deadline = phase_deadline(runtime.limits.prompt, request.deadline);
    let initialize = tokio::select! {
        biased;
        () = shutdown.wait() => Err(EngineOperationError::Shutdown),
        () = request.control.wait() => Err(EngineOperationError::Cancelled),
        () = tokio::time::sleep_until(handshake_deadline) => Err(EngineOperationError::Deadline),
        result = transport.initialize() => result.map_err(map_grok_acp_error),
    };
    let initialize = match initialize {
        Ok(initialize) => initialize,
        Err(error) => {
            return finish_grok_start(Some(transport), child, request, error, runtime.limits.close)
                .await;
        }
    };
    let available: Vec<&str> = initialize.auth_methods.iter().map(String::as_str).collect();
    let Some(auth_method) =
        (definition.select_auth_method)(&available, grok_runtime::api_key_present())
    else {
        // No usable auth method is durable configuration state (the user
        // must sign in), not a transient provider failure.
        return finish_grok_start(
            Some(transport),
            child,
            request,
            EngineOperationError::Configuration,
            runtime.limits.close,
        )
        .await;
    };
    let authenticated = tokio::select! {
        biased;
        () = shutdown.wait() => Err(EngineOperationError::Shutdown),
        () = request.control.wait() => Err(EngineOperationError::Cancelled),
        () = tokio::time::sleep_until(handshake_deadline) => Err(EngineOperationError::Deadline),
        result = transport.authenticate(auth_method) => result.map_err(map_grok_acp_error),
    };
    if let Err(error) = authenticated {
        return finish_grok_start(Some(transport), child, request, error, runtime.limits.close)
            .await;
    }

    // session/new or session/load -------------------------------------------
    // A gated continuation reopens the stored provider conversation
    // (`session/load` over the same cwd a fresh start would use); the
    // prepared identity must equal the stored one, so resume reopens the
    // same conversation id. Fresh turns open exactly one new session.
    // Either way provider-owned state is resumed, never invented, and a
    // restart replays the durable prefix without duplicating provider
    // effects with a second conversation.
    let cwd = request.input.project_root.as_str().to_owned();
    let session = if let Some(stored) = resume_stored_session_id.as_deref() {
        let Some(validated) = grok_runtime::grok_resume_session_id(stored) else {
            let _ = transport.shutdown_writer().await;
            return finish_grok_start(
                Some(transport),
                child,
                request,
                EngineOperationError::Configuration,
                runtime.limits.close,
            )
            .await;
        };
        let Ok(resumed) =
            acp_core::SessionId::parse(validated.as_str(), grok_runtime::GROK_MAX_SESSION_ID_BYTES)
        else {
            let _ = transport.shutdown_writer().await;
            return finish_grok_start(
                Some(transport),
                child,
                request,
                EngineOperationError::Configuration,
                runtime.limits.close,
            )
            .await;
        };
        let loaded = tokio::select! {
            biased;
            () = shutdown.wait() => Err(EngineOperationError::Shutdown),
            () = request.control.wait() => Err(EngineOperationError::Cancelled),
            () = tokio::time::sleep_until(handshake_deadline) => Err(EngineOperationError::Deadline),
            result = transport.load_session(&resumed, cwd.as_str()) => result.map_err(map_grok_acp_error),
        };
        if let Err(error) = loaded {
            return finish_grok_start(Some(transport), child, request, error, runtime.limits.close)
                .await;
        }
        if !grok_runtime::grok_loaded_session_is_stored(resumed.as_str(), stored) {
            return finish_grok_start(
                Some(transport),
                child,
                request,
                EngineOperationError::ProviderRequestFailed,
                runtime.limits.close,
            )
            .await;
        }
        resumed
    } else {
        let fresh = tokio::select! {
            biased;
            () = shutdown.wait() => Err(EngineOperationError::Shutdown),
            () = request.control.wait() => Err(EngineOperationError::Cancelled),
            () = tokio::time::sleep_until(handshake_deadline) => Err(EngineOperationError::Deadline),
            result = transport.new_session(cwd.as_str()) => result.map_err(map_grok_acp_error),
        };
        match fresh {
            Ok(session) => session,
            Err(error) => {
                return finish_grok_start(
                    Some(transport),
                    child,
                    request,
                    error,
                    runtime.limits.close,
                )
                .await;
            }
        }
    };

    // Bind authorization gate: the dispatcher binds the native session id
    // before exactly one prompt is authorized. Destructure here so the
    // prepared session carries the exact native identity.
    let ConfiguredTurnRequest {
        input,
        deadline,
        control,
        prepared,
        mut authorize,
        observations,
        respond,
        // Grok carries no steer verb: admit never wires a channel here, so
        // every steer attempt resolves `Unsupported` without prompt-state
        // plumbing.
        steer_rx: _,
    } = request;
    if prepared
        .send(Ok(PreparedSession::new(session.as_str().to_owned())))
        .is_err()
    {
        let _ = transport.shutdown_writer().await;
        drop(transport);
        return finish_grok_turn(
            child,
            Err(EngineOperationError::Cancelled),
            respond,
            runtime.limits.close,
        )
        .await;
    }
    let authorized = loop {
        tokio::select! {
            biased;
            () = shutdown.wait() => break Err(EngineOperationError::Shutdown),
            () = control.wait() => break Err(EngineOperationError::Cancelled),
            () = tokio::time::sleep_until(deadline) => break Err(EngineOperationError::Deadline),
            result = &mut authorize => {
                break result.map_err(|_| EngineOperationError::ProviderRequestFailed);
            }
            event = stderr_counter.pump(), if stderr_counter.state() == super::super::process::StderrState::Open => {
                let _ = event;
            }
        }
    };
    if let Err(error) = authorized {
        let _ = transport.shutdown_writer().await;
        drop(transport);
        return finish_grok_turn(child, Err(error), respond, runtime.limits.close).await;
    }

    // session/prompt + update pump -----------------------------------------
    let prompt_text = input
        .prompt
        .text()
        .map(|text| text.as_str().to_owned())
        .unwrap_or_default();
    let content =
        match acp_core::build_prompt_content(definition.image_mode, &prompt_text, &[], None) {
            Ok(content) => content,
            Err(error) => {
                let _ = transport.shutdown_writer().await;
                drop(transport);
                return finish_grok_turn(
                    child,
                    Err(map_grok_acp_error(error)),
                    respond,
                    runtime.limits.close,
                )
                .await;
            }
        };
    let prompt_id = match transport.prompt(&session, content).await {
        Ok(prompt_id) => prompt_id,
        Err(error) => {
            let _ = transport.shutdown_writer().await;
            drop(transport);
            return finish_grok_turn(
                child,
                Err(map_grok_acp_error(error)),
                respond,
                runtime.limits.close,
            )
            .await;
        }
    };
    let mut bridges = super::super::acp_bridges::PendingBridgeTable::new();
    // Best-effort usage scope: explicit model plus thread scope, else usage
    // results stay diagnostics. Usage never blocks the turn.
    let usage_attribution = match (&input.thread_id, input.settings.config().selection()) {
        (Some(thread_id), artisan_domain::EngineSelection::Grok(selection)) => selection
            .model_id()
            .map(|model| grok_runtime::GrokUsageAttribution {
                thread_id: thread_id.clone(),
                model_id: model.clone(),
            }),
        _ => None,
    };
    let usage_scope = usage_attribution
        .as_ref()
        .map(|attribution| grok_runtime::GrokUsageScope {
            thread_id: &attribution.thread_id,
            model_id: &attribution.model_id,
            provider_session_id: session.as_str(),
        });
    let outcome = grok_pump_loop(
        &mut transport,
        &session,
        &prompt_id,
        &mut bridges,
        deadline,
        shutdown,
        &control,
        &mut stderr_counter,
        &observations,
        &input.run_id,
        usage_scope.as_ref(),
    )
    .await;
    let _ = transport.shutdown_writer().await;
    drop(transport);
    drop(observations);
    match outcome {
        GrokPumpOutcome::Terminal(state) => {
            finish_grok_turn(
                child,
                Ok(EngineTurnResult { terminal: state }),
                respond,
                runtime.limits.close,
            )
            .await
        }
        GrokPumpOutcome::Failed(error) => {
            finish_grok_turn(child, Err(error), respond, runtime.limits.close).await
        }
    }
}

enum GrokPumpOutcome {
    Terminal(super::super::observation::TerminalState),
    Failed(EngineOperationError),
}

/// Drives one authorized Grok prompt round to its terminal state.
///
/// Mirrors the Codex pump structure over the ACP update loop: owner
/// shutdown, explicit cancellation (with a best-effort provider cancel),
/// and the attempt deadline preempt the transport; EOF before the prompt
/// result is interruption, a silent window is failure, and only the matching
/// prompt result settles the turn. Agent requests normalize into the pending
/// bridge table; per-round usage projects best-effort onto the shared usage
/// vocabulary without blocking the turn; streaming updates await the later
/// text-projection packet.
#[allow(clippy::too_many_arguments)]
async fn grok_pump_loop(
    transport: &mut super::super::acp::AcpTransport<
        tokio::process::ChildStdout,
        tokio::process::ChildStdin,
    >,
    session: &super::super::acp::SessionId,
    prompt: &super::super::acp::AcpId,
    bridges: &mut super::super::acp_bridges::PendingBridgeTable,
    deadline: Instant,
    shutdown: &Arc<CancelHandle>,
    control: &Arc<CancelHandle>,
    stderr_counter: &mut StderrCounter,
    observations: &mpsc::Sender<EngineObservation>,
    run_id: &RunId,
    usage: Option<&super::super::grok::GrokUsageScope<'_>>,
) -> GrokPumpOutcome {
    use super::super::acp::AcpError;
    use super::super::acp::UpdateEvent;

    let mut usage_sequence: u64 = 0;
    loop {
        if shutdown.is_cancelled() {
            return GrokPumpOutcome::Failed(EngineOperationError::Shutdown);
        }
        if control.is_cancelled() {
            // Best-effort provider cancel before reporting cancellation.
            let _ = transport.cancel(session).await;
            return GrokPumpOutcome::Terminal(TerminalState::Cancelled);
        }
        if Instant::now() >= deadline {
            return GrokPumpOutcome::Failed(EngineOperationError::Deadline);
        }
        tokio::select! {
            biased;
            () = shutdown.wait() => return GrokPumpOutcome::Failed(EngineOperationError::Shutdown),
            () = control.wait() => {
                let _ = transport.cancel(session).await;
                return GrokPumpOutcome::Terminal(TerminalState::Cancelled);
            }
            () = tokio::time::sleep_until(deadline) => {
                return GrokPumpOutcome::Failed(EngineOperationError::Deadline);
            }
            event = stderr_counter.pump(), if stderr_counter.state() == super::super::process::StderrState::Open => {
                let _ = event;
            }
            update = transport.next_update(session, prompt) => match update {
                Ok(UpdateEvent::SessionUpdate(update)) => {
                    if let Some(title) = update.update.summary_title() {
                        let _ = observations.send(EngineObservation::SummaryTitle { run_id: run_id.clone(), title }).await;
                    }
                }
                Ok(UpdateEvent::AgentRequest { id, method, params }) => {
                    note_grok_agent_request(bridges, &id, &method, &params);
                }
                Ok(UpdateEvent::PromptResult(outcome)) => {
                    if let Some(sample) = outcome
                        .usage
                        .as_ref()
                        .and_then(super::super::grok::grok_sample_from_token_usage)
                    {
                        usage_sequence = usage_sequence.saturating_add(1);
                        if super::super::grok::project_grok_usage_sample(
                            observations,
                            run_id,
                            usage,
                            usage_sequence,
                            &sample,
                        )
                        .await
                        .is_some()
                        {
                            return GrokPumpOutcome::Terminal(TerminalState::Interrupted);
                        }
                    }
                    return GrokPumpOutcome::Terminal(if outcome.cancelled {
                        TerminalState::Cancelled
                    } else {
                        TerminalState::Completed
                    });
                }
                Err(AcpError::Cancelled) => {
                    let _ = transport.cancel(session).await;
                    return GrokPumpOutcome::Terminal(TerminalState::Cancelled);
                }
                Err(AcpError::PeerClosed) => {
                    return GrokPumpOutcome::Terminal(TerminalState::Interrupted);
                }
                Err(AcpError::InactivityStall) => {
                    return GrokPumpOutcome::Terminal(TerminalState::Failed);
                }
                Err(error) => return GrokPumpOutcome::Failed(map_grok_acp_error(error)),
            },
        }
    }
}

/// Tracks one agent-initiated ACP request in the pending bridge table.
///
/// Permission and elicitation frames normalize through the A2 bridges and
/// validate fail-closed: malformed frames never reach the durable rails.
/// Unknown methods (including cursor-specific extensions on the shared wire)
/// stay untracked and unanswered; the bridges never auto-answer.
fn note_grok_agent_request(
    table: &mut super::super::acp_bridges::PendingBridgeTable,
    id: &super::super::acp::AcpId,
    method: &str,
    params: &Value,
) {
    if method == super::super::grok::GROK_PERMISSION_METHOD {
        if let Ok(pending) = super::super::acp_bridges::normalize_permission_request(params) {
            let _ = table.insert_approval(super::super::grok::GROK_MAX_PENDING_REQUESTS, pending);
        }
    } else if method == super::super::grok::GROK_ELICITATION_METHOD {
        let provider_id = match id {
            super::super::acp::AcpId::Number(number) => number.to_string(),
            super::super::acp::AcpId::Text(text) => text.clone(),
        };
        if let Ok(pending) =
            super::super::acp_bridges::normalize_elicitation_request(provider_id.as_str(), params)
        {
            let _ =
                table.insert_elicitation(super::super::grok::GROK_MAX_PENDING_REQUESTS, pending);
        }
    }
}

/// Maps one ACP core failure onto the owner error vocabulary.
///
/// Cancellation stays distinct; a protocol version mismatch surfaces as
/// `IncompatibleVersion`; every other wire failure (including auth,
/// framing, stall, and child errors) is a provider request failure. No
/// provider bytes cross this boundary: the core error is payload-free.
fn map_grok_acp_error(error: super::super::acp::AcpError) -> EngineOperationError {
    match error {
        super::super::acp::AcpError::Cancelled => EngineOperationError::Cancelled,
        super::super::acp::AcpError::UnsupportedVersion => {
            EngineOperationError::IncompatibleVersion
        }
        _ => EngineOperationError::ProviderRequestFailed,
    }
}

/// Runs the fixed pre-prompt teardown for a faulted Grok start and settles
/// both owner channels: the stdin lifeline closes first so an EOF-clean
/// agent can exit on its own, then the child reaps within the close budget.
async fn finish_grok_start(
    transport: Option<
        super::super::acp::AcpTransport<tokio::process::ChildStdout, tokio::process::ChildStdin>,
    >,
    child: super::super::acp::AcpChild,
    request: ConfiguredTurnRequest,
    error: EngineOperationError,
    close_budget: Duration,
) -> Execution {
    if let Some(mut transport) = transport {
        let _ = transport.shutdown_writer().await;
    }
    let ConfiguredTurnRequest {
        prepared, respond, ..
    } = request;
    let _ = prepared.send(Err(error.clone()));
    finish_grok_turn(child, Err(error), respond, close_budget).await
}

/// Settles one Grok turn after its transport is gone.
///
/// Mirrors the success path of the shared cleanup (bounded reap, then
/// settle) over the ACP child's fixed teardown. An unobserved reap cannot
/// quarantine through the owner `process` contract (`AcpRetainedChild`
/// carries no observable wait): the retained handle drops `ÔÇö` the spawn sets
/// `kill_on_drop`, and ACP children hold no ports or secrets `ÔÇö` while the
/// caller still observes `UnresolvedReapDuring`. Full quarantine returns
/// with the verified-launch authority packet.
async fn finish_grok_turn(
    child: super::super::acp::AcpChild,
    result: TurnResult,
    respond: oneshot::Sender<TurnResult>,
    close_budget: Duration,
) -> Execution {
    match super::super::acp::shutdown_acp_child(child, close_budget).await {
        super::super::acp::AcpShutdown::ReapedWithoutKill(_)
        | super::super::acp::AcpShutdown::ReapedAfterKill(_) => {
            let _ = respond.send(result);
            Execution::Completed
        }
        super::super::acp::AcpShutdown::Retained(retained) => {
            drop(retained);
            let primary = result
                .err()
                .map_or_else(|| Box::new(EngineOperationError::ReapUnresolved), Box::new);
            let _ = respond.send(Err(EngineOperationError::UnresolvedReapDuring { primary }));
            Execution::Completed
        }
    }
}
