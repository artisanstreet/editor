//! Codex executor seam: one finite `codex app-server --stdio` turn,
//! its bounded preflight and turn-start waits, JSON-RPC pumps, and
//! steer delivery plumbing.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use artisan_transport::CancelHandle;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::time::Instant;

use super::super::observation::EngineObservation;
use super::super::observation::TerminalState;
use super::super::process::ChildParts;
use super::super::process::LifelineWriter;
use super::super::process::StderrCounter;
use super::core::EngineOperationError;
use super::core::EngineTurnResult;
use super::core::Execution;
use super::core::PreparedSession;
use super::core::SteerDelivery;
use super::core::SteerError;
use super::core::settle_steers_closed;
use super::turn_common::ConfiguredRuntime;
use super::turn_common::ConfiguredTurnRequest;
use super::turn_common::finish_configured_start;
use super::turn_common::finish_turn_result;
use super::turn_common::phase_deadline;
use super::turn_common::wait_for_authorization;

/// Executes one finite Codex turn over `codex app-server --stdio`.
///
/// Single-owner match arm beside the `OpenCode2` executor: no second task, no
/// second queue. Performs initialize, thread/start (or `thread/resume` for a
/// gated continuation that reopens the same provider thread), the bind
/// authorization gate, then turn/start plus the streaming pump. Text deltas
/// normalize onto the shared S1a vocabulary; token-usage frames project
/// best-effort to cumulative usage observations without blocking the turn;
/// approval/question frames populate the pending tracker with no
/// control-flow side effect; child-thread frames never adopt the root turn.
/// External-kill EOF maps to `Interrupted`, explicit cancel to `Cancelled`,
/// and stall/failure to `Failed`. Teardown terminates the whole process
/// group (no orphaned codex grandchildren holding pipes) and quarantines on
/// unobserved reaps.
#[expect(
    clippy::too_many_lines,
    reason = "one configured turn is a single linear protocol sequence over the spawned child; extraction would thread the full child state"
)]
pub(super) async fn execute_codex_turn(
    request: ConfiguredTurnRequest,
    runtime: ConfiguredRuntime,
    shutdown: &Arc<CancelHandle>,
) -> Execution {
    use super::super::codex as codex_runtime;
    use super::super::process::spawn_codex_engine;

    let artisan_domain::EngineSelection::Codex(selection) =
        request.input.settings.config().selection()
    else {
        return request.fail(EngineOperationError::Configuration);
    };
    let Ok(settings) = codex_runtime::CodexSettings::from_selection(selection) else {
        return request.fail(EngineOperationError::Configuration);
    };
    if settings.profile_id() != request.input.launch.profile_id() {
        return request.fail(EngineOperationError::Configuration);
    }
    let super::super::InternalLaunch::Codex(launch) = &request.input.launch else {
        return request.fail(EngineOperationError::Configuration);
    };
    // X3 continuation gate: same-engine is fenced by the dispatcher (codex
    // bindings only); the owner additionally requires an explicit target
    // model and CLI >= 0.145.0. Anything else is typed incompatible — never
    // a silent fresh start and never a cross-engine resume.
    let resume_stored_thread_id: Option<String> = match &request.input.continuation {
        None => None,
        Some(continuation) => {
            let gate = codex_runtime::check_codex_native_continuation(
                &codex_runtime::CodexContinuationGateInput {
                    cli_version: request.input.launch.version(),
                    target_model: selection.model_id().map(artisan_domain::EngineModelId::as_str),
                    advertised_models: None,
                    same_engine: true,
                },
            );
            if !matches!(gate, codex_runtime::CodexContinuationDecision::Compatible) {
                return request.fail(EngineOperationError::Configuration);
            }
            Some(continuation.provider_session_id().to_owned())
        }
    };
    let Ok(mut child) = spawn_codex_engine(launch.as_ref(), &request.input.project_root) else {
        return request.fail(EngineOperationError::SpawnFailed);
    };
    let stdin_opt = child.stdin.take();
    let stdout_opt = child.stdout.take();
    let stderr_opt = child.stderr.take();
    let lifeline = LifelineWriter::take(&mut child);
    let stderr_counter = StderrCounter::new(stderr_opt, runtime.bounds.stderr_cap_bytes);
    let (Some(mut stdin), Some(stdout)) = (stdin_opt, stdout_opt) else {
        let parts = ChildParts {
            child,
            lifeline,
            stdout: None,
            stderr_counter,
        };
        return finish_configured_start(
            request,
            parts,
            EngineOperationError::SpawnFailed,
            runtime.limits.close,
        )
        .await;
    };
    let mut parts = ChildParts {
        child,
        lifeline,
        stdout: None,
        stderr_counter,
    };
    let mut reader = tokio::io::BufReader::new(stdout);
    let mut next_id: u64 = 1;

    // initialize ---------------------------------------------------------
    let init_line = codex_runtime::request_line(
        next_id,
        "initialize",
        &codex_runtime::initialize_params("artisan-editor", "0.3.0"),
    );
    next_id += 1;
    if write_codex_line(&mut stdin, &init_line).await.is_err() {
        return finish_configured_start(
            request,
            parts,
            EngineOperationError::ProviderRequestFailed,
            runtime.limits.close,
        )
        .await;
    }
    let mut line = String::new();
    // Correlated wait: the server may emit notifications before the
    // `initialize` result, so the reply is matched by request id.
    match codex_await_preflight_result(
        &mut reader,
        &mut line,
        1,
        phase_deadline(runtime.limits.prompt, request.deadline),
        shutdown,
        &request.control,
    )
    .await
    {
        CodexPreflightWait::Ready => {}
        CodexPreflightWait::Failed(error) => {
            return finish_configured_start(request, parts, error, runtime.limits.close).await;
        }
    }
    // Official handshake order (`Handshake` in
    // `modules/engines/src/codex/app-server-session.ts`): the client notifies
    // `initialized` (no id, no params) once the `initialize` result arrives,
    // before any `thread/*` request. A notification never consumes a request
    // id, so `next_id` still names the `thread/*` request below.
    let initialized_line = codex_runtime::notification_line("initialized");
    if write_codex_line(&mut stdin, &initialized_line)
        .await
        .is_err()
    {
        return finish_configured_start(
            request,
            parts,
            EngineOperationError::ProviderRequestFailed,
            runtime.limits.close,
        )
        .await;
    }
    // thread/start or thread/resume ---------------------------------------
    // A gated continuation reopens the stored provider thread
    // (`thread/resume` over the same options a fresh start would use); the
    // response must name the same thread id or the turn fails closed. Fresh
    // turns start exactly one thread. Either way provider-owned state is
    // resumed, never invented, and a restart never duplicates provider
    // effects with a second thread.
    let thread_id = if let Some(stored) = resume_stored_thread_id.as_deref() {
        let Some(resume_params) =
            codex_runtime::thread_resume_params(&settings, &request.input.project_root, stored)
        else {
            return finish_configured_start(
                request,
                parts,
                EngineOperationError::Configuration,
                runtime.limits.close,
            )
            .await;
        };
        let thread_request_id = next_id;
        let resume_line =
            codex_runtime::request_line(thread_request_id, "thread/resume", &resume_params);
        next_id += 1;
        if write_codex_line(&mut stdin, &resume_line).await.is_err() {
            return finish_configured_start(
                request,
                parts,
                EngineOperationError::ProviderRequestFailed,
                runtime.limits.close,
            )
            .await;
        }
        // Correlated wait: notifications arrive before the `thread/resume`
        // result, so the reply is matched by request id; a mismatch or a
        // matching error fails closed without a silent fresh start.
        match codex_await_preflight_result(
            &mut reader,
            &mut line,
            thread_request_id,
            phase_deadline(runtime.limits.prompt, request.deadline),
            shutdown,
            &request.control,
        )
        .await
        {
            CodexPreflightWait::Ready => {}
            CodexPreflightWait::Failed(error) => {
                return finish_configured_start(request, parts, error, runtime.limits.close).await;
            }
        }
        let Some(thread_id) = codex_resumed_thread_id(&line, thread_request_id, stored) else {
            return finish_configured_start(
                request,
                parts,
                EngineOperationError::ProviderRequestFailed,
                runtime.limits.close,
            )
            .await;
        };
        thread_id
    } else {
        let thread_request_id = next_id;
        let thread_line = codex_runtime::request_line(
            thread_request_id,
            "thread/start",
            &settings.thread_params(&request.input.project_root),
        );
        next_id += 1;
        if write_codex_line(&mut stdin, &thread_line).await.is_err() {
            return finish_configured_start(
                request,
                parts,
                EngineOperationError::ProviderRequestFailed,
                runtime.limits.close,
            )
            .await;
        }
        // Correlated wait, matching the resume branch above.
        match codex_await_preflight_result(
            &mut reader,
            &mut line,
            thread_request_id,
            phase_deadline(runtime.limits.prompt, request.deadline),
            shutdown,
            &request.control,
        )
        .await
        {
            CodexPreflightWait::Ready => {}
            CodexPreflightWait::Failed(error) => {
                return finish_configured_start(request, parts, error, runtime.limits.close).await;
            }
        }
        let Some(thread_id) = codex_thread_id(&line, thread_request_id) else {
            return finish_configured_start(
                request,
                parts,
                EngineOperationError::ProviderRequestFailed,
                runtime.limits.close,
            )
            .await;
        };
        thread_id
    };

    // Bind authorization gate: the dispatcher binds the native thread id
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
        mut steer_rx,
    } = request;
    if prepared
        .send(Ok(PreparedSession::new(thread_id.clone())))
        .is_err()
    {
        drop(stdin);
        return finish_turn_result(
            parts,
            Err(EngineOperationError::Cancelled),
            respond,
            runtime.limits.close,
        )
        .await;
    }
    if let Err(error) =
        wait_for_authorization(&mut parts, &mut authorize, deadline, shutdown, &control).await
    {
        drop(stdin);
        return finish_turn_result(parts, Err(error), respond, runtime.limits.close).await;
    }

    // turn/start + streaming pump -----------------------------------------
    // The request binds the exact native thread id from `thread/start` (or
    // `thread/resume`); the real server rejects a missing `threadId` with
    // `-32600`. The reply is awaited with a bounded notification-aware wait:
    // the real server emits interleaved notifications (for example
    // `thread/started`) between the `thread/*` result and the `turn/start`
    // result, so lines are correlated by request id instead of assuming the
    // next line is the reply. Interleaved notifications are processed through
    // the same event pipeline (never discarded), a matching error envelope
    // fails the turn fast, and anything else keeps waiting inside the same
    // absolute phase deadline.
    let prompt_text = input
        .prompt
        .text()
        .map(|text| text.as_str().to_owned())
        .unwrap_or_default();
    let turn_params = settings.turn_start_params(thread_id.as_str(), &prompt_text);
    let turn_request_id = next_id;
    let turn_line = codex_runtime::request_line(next_id, "turn/start", &turn_params);
    next_id += 1;
    if write_codex_line(&mut stdin, &turn_line).await.is_err() {
        drop(stdin);
        return finish_turn_result(
            parts,
            Err(EngineOperationError::ProviderRequestFailed),
            respond,
            runtime.limits.close,
        )
        .await;
    }
    let inactivity = runtime.limits.sse;
    let mut tracker = codex_runtime::CodexPendingTracker::new();
    // Root thread authority for rich activity: the native thread from
    // thread/start (or the resumed thread) is the only thread whose frames
    // may emit onto the root activity channel. Bound before the turn/start
    // wait so legitimate interleaved root frames already normalize.
    tracker.bind_native_thread(&thread_id);
    let mut active_turn: Option<String> = None;
    let mut frame_sequence: u64 = 0;
    let mut last_activity = Instant::now();
    // Best-effort usage scope: explicit model plus thread scope, else usage
    // frames stay diagnostics. Usage never blocks the turn.
    let usage_attribution = match (&input.thread_id, input.settings.config().selection()) {
        (Some(thread_id), artisan_domain::EngineSelection::Codex(selection)) => selection
            .model_id()
            .map(|model| codex_runtime::CodexUsageAttribution {
                thread_id: thread_id.clone(),
                model_id: model.clone(),
            }),
        _ => None,
    };
    let usage_scope =
        usage_attribution
            .as_ref()
            .map(|attribution| codex_runtime::CodexUsageScope {
                thread_id: &attribution.thread_id,
                model_id: &attribution.model_id,
                provider_session_id: thread_id.as_str(),
            });
    // Steers arriving before the provider turn id is known wait here,
    // bounded by the same phase deadline: the id is never invented, and a
    // turn that never starts rejects every buffered steer typed.
    let mut pending_steers: Vec<SteerDelivery> = Vec::new();
    let turn_wait = codex_await_turn_start(
        &mut reader,
        &mut line,
        &input.run_id,
        &mut tracker,
        &mut active_turn,
        &mut frame_sequence,
        &mut last_activity,
        phase_deadline(runtime.limits.prompt, deadline),
        shutdown,
        &control,
        &observations,
        usage_scope.as_ref(),
        turn_request_id,
        &mut steer_rx,
        &mut pending_steers,
    )
    .await;
    let mut no_pending_acks: HashMap<u64, oneshot::Sender<Result<(), SteerError>>> = HashMap::new();
    let provider_turn_id = match turn_wait {
        CodexTurnWait::Accepted(turn_id) => turn_id,
        CodexTurnWait::Terminal(state) => {
            settle_steers_closed(&mut steer_rx, &mut pending_steers, &mut no_pending_acks);
            drop(stdin);
            drop(observations);
            return finish_turn_result(
                parts,
                Ok(EngineTurnResult { terminal: state }),
                respond,
                runtime.limits.close,
            )
            .await;
        }
        CodexTurnWait::Failed(error) => {
            settle_steers_closed(&mut steer_rx, &mut pending_steers, &mut no_pending_acks);
            drop(stdin);
            return finish_turn_result(parts, Err(error), respond, runtime.limits.close).await;
        }
    };
    // Flush pre-turn-start steers in arrival order against the now-known
    // provider turn id. Each write allocates its own request id and
    // registers its ack for the correlated `turn/steer` result; a failed
    // write resolves that delivery immediately. The ack is never the
    // write alone: only the provider result counts, and a correlated
    // error (for example an `expectedTurnId` mismatch) fails typed
    // without settling the turn.
    let mut pending_steer_acks: HashMap<u64, oneshot::Sender<Result<(), SteerError>>> =
        HashMap::new();
    for delivery in std::mem::take(&mut pending_steers) {
        service_codex_steer_delivery(
            &mut stdin,
            &mut next_id,
            thread_id.as_str(),
            Some(provider_turn_id.as_str()),
            delivery,
            &mut pending_steer_acks,
        )
        .await;
    }
    // The turn is in flight from the server's `turn/start` result: seeding
    // the active turn lets the inactivity deadline settle a silent turn as
    // stalled instead of waiting idle until the attempt budget expires.
    active_turn = Some(provider_turn_id);
    last_activity = Instant::now();
    let terminal = codex_pump_loop(
        &mut reader,
        &mut stdin,
        &mut parts,
        &mut line,
        &input.run_id,
        &mut tracker,
        &mut active_turn,
        &mut frame_sequence,
        &mut last_activity,
        inactivity,
        deadline,
        shutdown,
        &control,
        &observations,
        &thread_id,
        usage_scope.as_ref(),
        &mut steer_rx,
        &mut pending_steer_acks,
        &mut next_id,
    )
    .await;
    drop(stdin);
    let _ = next_id;
    match terminal {
        CodexPumpOutcome::Terminal(state) => {
            drop(observations);
            finish_turn_result(
                parts,
                Ok(EngineTurnResult { terminal: state }),
                respond,
                runtime.limits.close,
            )
            .await
        }
        CodexPumpOutcome::Failed(error) => {
            finish_turn_result(parts, Err(error), respond, runtime.limits.close).await
        }
    }
}

enum CodexPumpOutcome {
    Terminal(super::super::observation::TerminalState),
    Failed(EngineOperationError),
}

/// Outcome of one bounded preflight reply wait.
enum CodexPreflightWait {
    /// The matching result arrived; the raw line stays in the caller's buffer
    /// for id-specific extraction.
    Ready,
    /// Shutdown, cancellation, deadline, EOF, or a matching error envelope.
    Failed(EngineOperationError),
}

/// Waits for one preflight reply (`initialize`, `thread/start`,
/// `thread/resume`) while surviving interleaved traffic.
///
/// The real server emits notifications (for example `remoteControl/*`,
/// `deprecationNotice`, `mcpStartup`, `threadStatus`, `thread/started`)
/// before the matching result, so every line is correlated by request id
/// instead of assuming the next line is the reply. Non-matching traffic —
/// method notifications, uncorrelated results/errors, unparseable lines —
/// keeps the wait alive inside the same absolute phase deadline; the turn
/// is not yet authorized, so nothing is forwarded to the observation sink.
/// A matching error envelope fails fast. Shutdown, cancellation, deadline,
/// and EOF map exactly like the previous single-read sites, and a failed
/// resume never falls back to a fresh start.
async fn codex_await_preflight_result(
    reader: &mut tokio::io::BufReader<tokio::process::ChildStdout>,
    line: &mut String,
    expected_id: u64,
    deadline: Instant,
    shutdown: &Arc<CancelHandle>,
    control: &Arc<CancelHandle>,
) -> CodexPreflightWait {
    use super::super::codex as codex_runtime;

    loop {
        if read_codex_line(reader, line, deadline, shutdown, control)
            .await
            .is_err()
        {
            let error = if shutdown.is_cancelled() {
                EngineOperationError::Shutdown
            } else if control.is_cancelled() {
                EngineOperationError::Cancelled
            } else {
                EngineOperationError::ProviderRequestFailed
            };
            return CodexPreflightWait::Failed(error);
        }
        let trimmed = line.trim_end_matches(['\r', '\n']).to_owned();
        if codex_runtime::is_codex_error_response(&trimmed)
            && codex_response_id_matches(&trimmed, expected_id)
        {
            return CodexPreflightWait::Failed(EngineOperationError::ProviderRequestFailed);
        }
        if is_codex_result_for(&trimmed, expected_id) {
            return CodexPreflightWait::Ready;
        }
    }
}

/// Outcome of the bounded `turn/start` reply wait.
enum CodexTurnWait {
    /// The server accepted the turn; carries the native turn identity.
    Accepted(String),
    /// An interleaved notification already settled the turn.
    Terminal(super::super::observation::TerminalState),
    /// Shutdown, cancellation, deadline, EOF, or a matching error envelope.
    Failed(EngineOperationError),
}

/// Waits for the `turn/start` reply while preserving interleaved traffic.
///
/// The real server emits notifications (for example `thread/started`)
/// between the `thread/*` result and the `turn/start` result, so every line
/// is correlated by request id instead of assuming the next line is the
/// reply. Interleaved notifications flow through the shared event pipeline —
/// deltas reach the observation sink and a terminal event settles the turn —
/// while a matching error envelope (for example `-32600` for a missing
/// `threadId`) fails fast. Uncorrelated error envelopes and unparseable
/// lines keep the wait alive inside the same absolute phase deadline; the
/// turn is not yet in flight, so no inactivity deadline applies here.
///
/// Steers arriving before the provider turn id is known are buffered in
/// arrival order into `pending_steers` and flushed by the caller once the
/// id is known — the id is never invented. A wait that ends without a turn
/// rejects every buffered steer typed through the shared settle helper.
#[allow(clippy::too_many_arguments)]
async fn codex_await_turn_start(
    reader: &mut tokio::io::BufReader<tokio::process::ChildStdout>,
    line: &mut String,
    run_id: &artisan_domain::RunId,
    tracker: &mut super::super::codex::CodexPendingTracker,
    active_turn: &mut Option<String>,
    frame_sequence: &mut u64,
    last_activity: &mut Instant,
    deadline: Instant,
    shutdown: &Arc<CancelHandle>,
    control: &Arc<CancelHandle>,
    observations: &mpsc::Sender<EngineObservation>,
    usage: Option<&super::super::codex::CodexUsageScope<'_>>,
    turn_request_id: u64,
    steer_rx: &mut Option<mpsc::Receiver<SteerDelivery>>,
    pending_steers: &mut Vec<SteerDelivery>,
) -> CodexTurnWait {
    use super::super::codex as codex_runtime;

    loop {
        line.clear();
        tokio::select! {
            biased;
            () = shutdown.wait() => {
                return CodexTurnWait::Failed(EngineOperationError::Shutdown);
            }
            () = control.wait() => {
                return CodexTurnWait::Failed(EngineOperationError::Cancelled);
            }
            () = tokio::time::sleep_until(deadline) => {
                return CodexTurnWait::Failed(EngineOperationError::Deadline);
            }
            steer_msg = async {
                match steer_rx.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                if let Some(delivery) = steer_msg {
                    pending_steers.push(delivery);
                }
            }
            result = read_codex_line(reader, line, deadline, shutdown, control) => {
                if result.is_err() {
                    if shutdown.is_cancelled() {
                        return CodexTurnWait::Failed(EngineOperationError::Shutdown);
                    }
                    if control.is_cancelled() {
                        return CodexTurnWait::Failed(EngineOperationError::Cancelled);
                    }
                    if Instant::now() >= deadline {
                        return CodexTurnWait::Failed(EngineOperationError::Deadline);
                    }
                    return CodexTurnWait::Failed(EngineOperationError::ProviderRequestFailed);
                }
                *last_activity = Instant::now();
                *frame_sequence += 1;
                let trimmed = line.trim_end_matches(['\r', '\n']).to_owned();
                if codex_runtime::is_codex_error_response(&trimmed)
                    && codex_response_id_matches(&trimmed, turn_request_id)
                {
                    return CodexTurnWait::Failed(EngineOperationError::ProviderRequestFailed);
                }
                if let Some(turn_id) = codex_turn_id(&trimmed, turn_request_id) {
                    return CodexTurnWait::Accepted(turn_id);
                }
                if let Ok(event) = codex_runtime::parse_frame(&trimmed, *frame_sequence)
                    && let Some(terminal) = codex_runtime::apply_event(
                        event,
                        run_id,
                        tracker,
                        active_turn,
                        observations,
                        *frame_sequence,
                        usage,
                    )
                    .await
                {
                    return CodexTurnWait::Terminal(terminal);
                }
            }
        }
    }
}

/// Extracts the JSON-RPC response id of one inbound line, if it carries one.
///
/// Method envelopes (notifications and server requests) carry no id and
/// yield `None`; numeric and string id forms both correlate. Used to route
/// correlated `turn/steer` replies to their pending delivery instead of
/// misparsing a steer response as turn completion.
fn codex_response_id(line: &str) -> Option<u64> {
    if line.len() > super::super::codex::CODEX_MAX_FRAME_BYTES {
        return None;
    }
    let value: Value = serde_json::from_str(line).ok()?;
    if value.get("method").is_some() {
        return None;
    }
    let id = value.get("id")?;
    if let Some(numeric) = id.as_u64() {
        return Some(numeric);
    }
    id.as_str()?.parse::<u64>().ok()
}

/// Services one codex steer delivery from the pump's owned stdin.
///
/// Writes the `turn/steer` verb with the actual provider turn id as
/// `expectedTurnId` and registers the delivery ack for the correlated
/// provider result: the ack resolves `Ok(())` only on the matching
/// `turn/steer` result, and `Err(DeliveryFailed)` on a failed write or a
/// correlated error reply (for example an `expectedTurnId` mismatch) —
/// never success on error, and never a turn settlement either way. The
/// write alone does not resolve: only the provider result counts.
/// Progress while the ack is outstanding comes from split ownership — the
/// dispatch Steer arm keeps draining observations while awaiting it, and
/// this pump polls the steer receiver alongside its observation sends.
/// A missing turn id rejects typed without inventing one; exactly one
/// write attempt is made per delivery, never a retry of an ambiguous
/// write.
pub(crate) async fn service_codex_steer_delivery<W: tokio::io::AsyncWrite + Unpin>(
    stdin: &mut W,
    next_id: &mut u64,
    thread_id: &str,
    turn_id: Option<&str>,
    delivery: SteerDelivery,
    pending_acks: &mut HashMap<u64, oneshot::Sender<Result<(), SteerError>>>,
) {
    use super::super::codex as codex_runtime;

    let Some(turn_id) = turn_id else {
        let _ = delivery.ack.send(Err(SteerError::DeliveryFailed));
        return;
    };
    let steer_id = *next_id;
    match codex_runtime::steer_live_turn(stdin, next_id, thread_id, turn_id, &delivery.text).await {
        Ok(()) => {
            pending_acks.insert(steer_id, delivery.ack);
        }
        Err(_) => {
            let _ = delivery.ack.send(Err(SteerError::DeliveryFailed));
        }
    }
}

/// Routes one inbound line to its pending steer delivery, if correlated.
///
/// Returns true when the line answers an outstanding `turn/steer`
/// request: a VALID result envelope (matching id plus a `result` member,
/// the only shape the protocol requires of a `turn/steer` reply — see
/// `session.Request("turn/steer", ...)` in
/// `modules/engines/src/codex/engine.ts`) resolves the delivery
/// successfully, and a valid error envelope resolves it failed. A
/// matching id with NEITHER (for example a bare `{id}` with no result)
/// is a malformed provider reply and resolves typed failure, never
/// success. The line never becomes a turn event either way — a steer
/// response is not turn completion, and a rejected follow-up fails only
/// its own delivery, never the turn. Uncorrelated lines return false and
/// keep the existing turn handling (notably the fail-fast on unrelated
/// error envelopes).
pub(crate) fn ack_codex_steer_response(
    line: &str,
    pending_acks: &mut HashMap<u64, oneshot::Sender<Result<(), SteerError>>>,
) -> bool {
    let Some(id) = codex_response_id(line) else {
        return false;
    };
    let Some(ack) = pending_acks.remove(&id) else {
        return false;
    };
    if super::super::codex::is_codex_error_response(line) {
        let _ = ack.send(Err(SteerError::DeliveryFailed));
    } else if is_codex_result_for(line, id) {
        let _ = ack.send(Ok(()));
    } else {
        let _ = ack.send(Err(SteerError::DeliveryFailed));
    }
    true
}

#[allow(clippy::too_many_arguments)]
async fn codex_pump_loop(
    reader: &mut tokio::io::BufReader<tokio::process::ChildStdout>,
    stdin: &mut tokio::process::ChildStdin,
    parts: &mut ChildParts,
    line: &mut String,
    run_id: &artisan_domain::RunId,
    tracker: &mut super::super::codex::CodexPendingTracker,
    active_turn: &mut Option<String>,
    frame_sequence: &mut u64,
    last_activity: &mut Instant,
    inactivity: Duration,
    deadline: Instant,
    shutdown: &Arc<CancelHandle>,
    control: &Arc<CancelHandle>,
    observations: &mpsc::Sender<EngineObservation>,
    thread_id: &str,
    usage: Option<&super::super::codex::CodexUsageScope<'_>>,
    steer_rx: &mut Option<mpsc::Receiver<SteerDelivery>>,
    pending_acks: &mut HashMap<u64, oneshot::Sender<Result<(), SteerError>>>,
    next_id: &mut u64,
) -> CodexPumpOutcome {
    let outcome = codex_pump_loop_inner(
        reader,
        stdin,
        parts,
        line,
        run_id,
        tracker,
        active_turn,
        frame_sequence,
        last_activity,
        inactivity,
        deadline,
        shutdown,
        control,
        observations,
        thread_id,
        usage,
        steer_rx,
        pending_acks,
        next_id,
    )
    .await;
    // Every exit settles unsettled steers typed: stop/cancel interrupts
    // pending steer requests deterministically instead of leaving
    // `steer_text` on an indefinite ack await.
    let mut buffered: Vec<SteerDelivery> = Vec::new();
    settle_steers_closed(steer_rx, &mut buffered, pending_acks);
    outcome
}

#[expect(
    clippy::too_many_lines,
    reason = "one pump-loop body keeps cancellation, reader, and steer state in one local scope; extraction would thread the whole child state"
)]
#[allow(clippy::too_many_arguments)]
async fn codex_pump_loop_inner(
    reader: &mut tokio::io::BufReader<tokio::process::ChildStdout>,
    stdin: &mut tokio::process::ChildStdin,
    parts: &mut ChildParts,
    line: &mut String,
    run_id: &artisan_domain::RunId,
    tracker: &mut super::super::codex::CodexPendingTracker,
    active_turn: &mut Option<String>,
    frame_sequence: &mut u64,
    last_activity: &mut Instant,
    inactivity: Duration,
    deadline: Instant,
    shutdown: &Arc<CancelHandle>,
    control: &Arc<CancelHandle>,
    observations: &mpsc::Sender<EngineObservation>,
    thread_id: &str,
    usage: Option<&super::super::codex::CodexUsageScope<'_>>,
    steer_rx: &mut Option<mpsc::Receiver<SteerDelivery>>,
    pending_acks: &mut HashMap<u64, oneshot::Sender<Result<(), SteerError>>>,
    next_id: &mut u64,
) -> CodexPumpOutcome {
    use super::super::codex as codex_runtime;
    use tokio::io::AsyncBufReadExt as _;

    loop {
        if shutdown.is_cancelled() {
            return CodexPumpOutcome::Failed(EngineOperationError::Shutdown);
        }
        if control.is_cancelled() {
            // Best-effort provider interrupt before reporting cancellation.
            if let Some(turn_id) = active_turn.clone() {
                let mut request_id = u64::MAX;
                let _ =
                    codex_runtime::interrupt_live_turn(stdin, &mut request_id, thread_id, &turn_id)
                        .await;
            }
            return CodexPumpOutcome::Terminal(TerminalState::Cancelled);
        }
        if Instant::now() >= deadline {
            return CodexPumpOutcome::Failed(EngineOperationError::Deadline);
        }
        if codex_runtime::has_stalled(
            active_turn.is_some(),
            *last_activity,
            inactivity,
            Instant::now(),
        ) {
            return CodexPumpOutcome::Terminal(TerminalState::Failed);
        }
        let stall_at = last_activity
            .checked_add(inactivity)
            .unwrap_or(deadline)
            .min(deadline);
        line.clear();
        tokio::select! {
            biased;
            () = shutdown.wait() => return CodexPumpOutcome::Failed(EngineOperationError::Shutdown),
            () = control.wait() => {
                if let Some(turn_id) = active_turn.clone() {
                    let mut request_id = u64::MAX;
                    let _ = codex_runtime::interrupt_live_turn(stdin, &mut request_id, thread_id, &turn_id).await;
                }
                return CodexPumpOutcome::Terminal(TerminalState::Cancelled);
            }
            () = tokio::time::sleep_until(deadline) => {
                return CodexPumpOutcome::Failed(EngineOperationError::Deadline);
            }
            () = tokio::time::sleep_until(stall_at) => {
                if codex_runtime::has_stalled(
                    active_turn.is_some(),
                    *last_activity,
                    inactivity,
                    Instant::now(),
                ) {
                    return CodexPumpOutcome::Terminal(TerminalState::Failed);
                }
                let _ = parts.stderr_counter.pump().await;
            }
            steer_msg = async {
                match steer_rx.as_mut() {
                    Some(rx) => rx.recv().await,
                    None => std::future::pending().await,
                }
            } => {
                let Some(delivery) = steer_msg else {
                    continue;
                };
                *last_activity = Instant::now();
                // Servicing writes the verb and registers the ack for the
                // correlated provider result; the ack resolves only when
                // that result arrives below. Progress meanwhile comes from
                // split ownership: the dispatch arm drains observations
                // while awaiting, and this loop keeps polling both sides.
                service_codex_steer_delivery(
                    stdin,
                    next_id,
                    thread_id,
                    active_turn.as_deref(),
                    delivery,
                    pending_acks,
                )
                .await;
            }
            read = reader.read_line(line) => {
                match read {
                    Ok(0) => {
                        // External kill: interruption, never cancel/failure.
                        return CodexPumpOutcome::Terminal(TerminalState::Interrupted);
                    }
                    Ok(_) => {
                        *last_activity = Instant::now();
                        *frame_sequence += 1;
                        let trimmed = line.trim_end_matches(['\r', '\n']).to_owned();
                        // Correlated `turn/steer` replies resolve their own
                        // delivery and never become turn events: a steer
                        // response is not turn completion, and a rejected
                        // follow-up fails only its delivery, never the turn.
                        if ack_codex_steer_response(&trimmed, pending_acks) {
                            continue;
                        }
                        // Late JSON-RPC error replies (for example a rejected
                        // steer/interrupt) never become turn events: fail the
                        // turn fast instead of ignoring them until the lease
                        // expires. Method envelopes are never error replies,
                        // so server requests and notifications are unaffected.
                        if codex_runtime::is_codex_error_response(&trimmed) {
                            return CodexPumpOutcome::Failed(
                                EngineOperationError::ProviderRequestFailed,
                            );
                        }
                        if let Ok(event) = codex_runtime::parse_frame(&trimmed, *frame_sequence)
                            && let Some(terminal) = codex_runtime::apply_event(
                                event,
                                run_id,
                                tracker,
                                active_turn,
                                observations,
                                *frame_sequence,
                                usage,
                            )
                            .await
                        {
                            return CodexPumpOutcome::Terminal(terminal);
                        }
                    }
                    Err(_) => return CodexPumpOutcome::Failed(EngineOperationError::StreamFailed),
                }
            }
        }
    }
}

async fn write_codex_line(
    stdin: &mut tokio::process::ChildStdin,
    line: &str,
) -> Result<(), EngineOperationError> {
    use tokio::io::AsyncWriteExt as _;
    stdin
        .write_all(line.as_bytes())
        .await
        .map_err(|_| EngineOperationError::ProviderRequestFailed)?;
    stdin
        .write_all(b"\n")
        .await
        .map_err(|_| EngineOperationError::ProviderRequestFailed)?;
    stdin
        .flush()
        .await
        .map_err(|_| EngineOperationError::ProviderRequestFailed)?;
    Ok(())
}

async fn read_codex_line(
    reader: &mut tokio::io::BufReader<tokio::process::ChildStdout>,
    line: &mut String,
    deadline: Instant,
    shutdown: &Arc<CancelHandle>,
    control: &Arc<CancelHandle>,
) -> Result<(), EngineOperationError> {
    use tokio::io::AsyncBufReadExt as _;
    line.clear();
    tokio::select! {
        biased;
        () = shutdown.wait() => Err(EngineOperationError::Shutdown),
        () = control.wait() => Err(EngineOperationError::Cancelled),
        () = tokio::time::sleep_until(deadline) => Err(EngineOperationError::Deadline),
        read = reader.read_line(line) => match read {
            Ok(0) | Err(_) => Err(EngineOperationError::ProviderRequestFailed),
            Ok(_) => Ok(()),
        },
    }
}

/// Returns whether one handshake line is the result for the request id.
///
/// Bounds the line before parsing and requires a `result` member; anything
/// else fails the handshake closed without spawning further phases.
pub(crate) fn is_codex_result_for(line: &str, id: u64) -> bool {
    if line.len() > super::super::codex::CODEX_MAX_FRAME_BYTES {
        return false;
    }
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return false;
    };
    let matches_id = value.get("id").is_some_and(|candidate| {
        candidate.as_u64() == Some(id)
            || candidate
                .as_str()
                .is_some_and(|text| text == id.to_string())
    });
    matches_id && value.get("result").is_some()
}

/// Extracts the exact native thread identity from a `thread/start` result.
///
/// Returns `None` on id mismatch, missing thread, or out-of-bound identity
/// so the dispatcher never binds a corrupt session.
pub(crate) fn codex_thread_id(line: &str, id: u64) -> Option<String> {
    if line.len() > super::super::codex::CODEX_MAX_FRAME_BYTES {
        return None;
    }
    let value: Value = serde_json::from_str(line).ok()?;
    let matches_id = value.get("id").is_some_and(|candidate| {
        candidate.as_u64() == Some(id)
            || candidate
                .as_str()
                .is_some_and(|text| text == id.to_string())
    });
    if !matches_id {
        return None;
    }
    let thread = value.get("result")?.get("thread")?;
    let id = thread.get("id")?.as_str()?;
    if id.is_empty() || id.len() > 256 {
        return None;
    }
    Some(id.to_owned())
}

/// Extracts the exact native turn identity from a `turn/start` result.
///
/// Returns `None` on id mismatch, on a JSON-RPC error envelope (for example
/// `-32600` for a missing `threadId`), or on a missing/out-of-bound turn
/// identity, so the dispatcher fails the turn fast instead of pumping a turn
/// that the server never started.
pub(crate) fn codex_turn_id(line: &str, id: u64) -> Option<String> {
    if line.len() > super::super::codex::CODEX_MAX_FRAME_BYTES {
        return None;
    }
    let value: Value = serde_json::from_str(line).ok()?;
    let matches_id = value.get("id").is_some_and(|candidate| {
        candidate.as_u64() == Some(id)
            || candidate
                .as_str()
                .is_some_and(|text| text == id.to_string())
    });
    if !matches_id {
        return None;
    }
    if value.get("error").is_some() {
        return None;
    }
    let turn = value.get("result")?.get("turn")?;
    let id = turn.get("id")?.as_str()?;
    if id.is_empty() || id.len() > 256 {
        return None;
    }
    Some(id.to_owned())
}

/// Returns whether one inbound line carries a JSON-RPC response id equal to
/// the supplied request id (numeric or string form).
///
/// Used to correlate error envelopes with their pending request: only the
/// matching reply fails its phase fast, while uncorrelated lines keep the
/// bounded wait alive.
pub(crate) fn codex_response_id_matches(line: &str, id: u64) -> bool {
    if line.len() > super::super::codex::CODEX_MAX_FRAME_BYTES {
        return false;
    }
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return false;
    };
    value.get("id").is_some_and(|candidate| {
        candidate.as_u64() == Some(id)
            || candidate
                .as_str()
                .is_some_and(|text| text == id.to_string())
    })
}

/// Extracts the resumed native thread identity, requiring the same thread.
///
/// `thread/resume` reopens provider-owned state only: a result naming any
/// other thread fails closed (`None`) instead of adopting a foreign session,
/// so resume reopens the same thread id and a restart replays the durable
/// prefix without duplicating provider effects.
pub(crate) fn codex_resumed_thread_id(
    line: &str,
    id: u64,
    stored_thread_id: &str,
) -> Option<String> {
    let resumed = codex_thread_id(line, id)?;
    (resumed.as_str() == stored_thread_id).then_some(resumed)
}
