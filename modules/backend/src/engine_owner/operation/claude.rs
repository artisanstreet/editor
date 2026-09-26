//! Claude executor seam: one finite `claude -p` stream-json turn, its
//! pump loop, subagent forwarding, and steer delivery plumbing.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use artisan_transport::CancelHandle;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::time::Instant;

use super::super::bounded_line::BoundedLineError;
use super::super::bounded_line::read_bounded_line;
use super::super::claude::CLAUDE_MAX_FRAME_BYTES;
use super::super::observation::EngineObservation;
use super::super::observation::SubagentLifecycleRow;
use super::super::observation::SubagentTranscriptRow;
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
use super::turn_common::wait_for_authorization;

/// Executes one finite Claude turn over `claude -p --output-format stream-json`.
///
/// Single-owner match arm beside the Codex executor: no second task, no
/// second queue. Writes the first user message over stdin (or reopens the
/// stored native session through `--resume` for a gated continuation),
/// waits for the `system/init` session identity behind the bind authorization
/// gate, then pumps the stream. Text deltas normalize onto the shared S1a
/// vocabulary with verbatim phases; usage frames project best-effort to
/// cumulative usage observations without blocking the turn; the generated
/// title is captured best-effort at the terminal fence;
/// `AskUserQuestion` frames populate pending questions lifted out of the
/// approval path; child transcript frames never adopt the root turn. EOF
/// before `result` maps to `Interrupted`, explicit cancel to `Cancelled`,
/// and stall/failure to `Failed`. Teardown terminates the whole process
/// group (no orphaned claude grandchildren holding pipes) and quarantines on
/// unobserved reaps.
#[expect(
    clippy::too_many_lines,
    reason = "one configured turn is a single linear protocol sequence over the spawned child; extraction would thread the full child state"
)]
pub(super) async fn execute_claude_turn(
    request: ConfiguredTurnRequest,
    runtime: ConfiguredRuntime,
    shutdown: &Arc<CancelHandle>,
) -> Execution {
    use super::super::claude as claude_runtime;
    use super::super::process::spawn_claude_engine;

    let artisan_domain::EngineSelection::Claude(selection) =
        request.input.settings.config().selection()
    else {
        return request.fail(EngineOperationError::Configuration);
    };
    let Ok(settings) = claude_runtime::ClaudeSettings::from_selection(selection) else {
        return request.fail(EngineOperationError::Configuration);
    };
    if settings.profile_id() != request.input.launch.profile_id() {
        return request.fail(EngineOperationError::Configuration);
    }
    let super::super::InternalLaunch::Claude(launch) = &request.input.launch else {
        return request.fail(EngineOperationError::Configuration);
    };
    // The effective thinking display resolves once from the verified launch
    // capability and applies identically to fresh starts and resumes; older
    // CLIs keep their existing arguments.
    let settings = settings.with_thinking_display(
        claude_runtime::ClaudeThinkingDisplay::for_support(launch.thinking_display()),
    );
    // L3 continuation gate: same-engine is fenced by the dispatcher (claude
    // bindings only); the owner additionally requires an explicit target
    // model and CLI >= 2.1.220. Anything else is typed incompatible — never
    // a silent fresh start and never a cross-engine resume.
    let resume_stored_session_id: Option<String> = match &request.input.continuation {
        None => None,
        Some(continuation) => {
            let gate = claude_runtime::check_claude_native_continuation(
                &claude_runtime::ClaudeContinuationGateInput {
                    cli_version: launch.version(),
                    target_model: selection
                        .model_id()
                        .map(artisan_domain::EngineModelId::as_str),
                    advertised_models: None,
                    same_engine: true,
                },
            );
            if !matches!(gate, claude_runtime::ClaudeContinuationDecision::Compatible) {
                return request.fail(EngineOperationError::Configuration);
            }
            Some(continuation.provider_session_id().to_owned())
        }
    };
    // A gated continuation reopens the stored native session (`--resume`
    // over the same flags a fresh start would use); fresh turns mint exactly
    // one session. Either way provider-owned state is resumed, never
    // invented, and a restart never duplicates provider effects with a second
    // session.
    let session = match resume_stored_session_id.as_deref() {
        Some(stored) => match claude_runtime::claude_resume_session(stored) {
            Some(session) => session,
            None => return request.fail(EngineOperationError::Configuration),
        },
        None => match claude_runtime::new_session_id() {
            Some(fresh) => claude_runtime::ClaudeSession::Start(fresh),
            None => return request.fail(EngineOperationError::EntropyFailed),
        },
    };
    let session_id = session.session_id().to_owned();
    let args = settings.spawn_args(&session);
    let mut child = match spawn_claude_engine(launch.as_ref(), &request.input.project_root, &args) {
        Ok(child) => child,
        Err(error) => {
            let detail = super::super::process::StartDiagnostic::for_spawn_error(&error);
            return request.fail_with_detail(EngineOperationError::SpawnFailed, detail);
        }
    };
    // The sole stdin lifeline is taken before any failure cleanup can run;
    // the prompt and every steer write borrow this exact handle, so cleanup's
    // `close()` is always the one true EOF for the child.
    let lifeline = LifelineWriter::take(&mut child);
    let stdout_opt = child.stdout.take();
    let stderr_counter = StderrCounter::new(child.stderr.take(), runtime.bounds.stderr_cap_bytes);
    let Some(stdout) = stdout_opt else {
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

    // First user message ----------------------------------------------------
    // The prompt travels as the first stdin line; there is no `turn/start`
    // RPC on this transport.
    {
        let line =
            claude_runtime::ClaudeSettings::user_message_payload(&session, &request.input.prompt);
        if claude_runtime::write_line(&mut parts.lifeline, &line)
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
    }

    // init-event gate ---------------------------------------------------------
    // The CLI speaks first: the bind authorization gate opens only after the
    // exact spawned session announces itself. Pre-init lines that are not
    // init are not replayed; a wrong session fails closed. A cold CLI can
    // take well over the prompt budget before `system/init` (plugin sync,
    // credential refresh), so startup is bounded by the attempt deadline and
    // the dispatcher's explicit launch deadline, which cancels this turn.
    let mut line = String::new();
    loop {
        line.clear();
        if read_claude_line(
            &mut reader,
            &mut line,
            request.deadline,
            shutdown,
            &request.control,
        )
        .await
        .is_err()
        {
            let error = if shutdown.is_cancelled() {
                EngineOperationError::Shutdown
            } else if request.control.is_cancelled() {
                EngineOperationError::Cancelled
            } else {
                EngineOperationError::ProviderRequestFailed
            };
            return finish_configured_start(request, parts, error, runtime.limits.close).await;
        }
        let trimmed = line.trim_end_matches(['\r', '\n']).to_owned();
        match claude_runtime::parse_frame(&trimmed, 0) {
            Ok(claude_runtime::ClaudeEvent::Init {
                session_id: announced,
            }) if announced == session_id => break,
            Ok(claude_runtime::ClaudeEvent::Init { .. }) => {
                return finish_configured_start(
                    request,
                    parts,
                    EngineOperationError::ProviderRequestFailed,
                    runtime.limits.close,
                )
                .await;
            }
            Ok(_) | Err(_) => {}
        }
    }

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
        mut steer_rx,
    } = request;
    if prepared
        .send(Ok(PreparedSession::new(session_id.clone())))
        .is_err()
    {
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
        return finish_turn_result(parts, Err(error), respond, runtime.limits.close).await;
    }

    // streaming pump ----------------------------------------------------------
    // Run-local tracking: a resumed session never inherits a previous run's
    // thinking stretches.
    let mut tracker =
        claude_runtime::ClaudePendingTracker::with_thinking_display(settings.thinking_display());
    let mut active_turn: Option<String> = None;
    let mut frame_sequence: u64 = 0;
    let mut last_activity = Instant::now();
    let inactivity = runtime.limits.sse;
    // Best-effort usage scope: explicit model plus thread scope, else usage
    // frames stay diagnostics. Usage never blocks the turn.
    let usage_attribution = match (&input.thread_id, input.settings.config().selection()) {
        (Some(thread_id), artisan_domain::EngineSelection::Claude(selection)) => selection
            .model_id()
            .map(|model| claude_runtime::ClaudeUsageAttribution {
                thread_id: thread_id.clone(),
                model_id: model.clone(),
            }),
        _ => None,
    };
    let usage_scope =
        usage_attribution
            .as_ref()
            .map(|attribution| claude_runtime::ClaudeUsageScope {
                thread_id: &attribution.thread_id,
                model_id: &attribution.model_id,
                provider_session_id: session_id.as_str(),
            });
    let terminal = claude_pump_loop(
        &mut reader,
        &mut parts,
        &mut line,
        &input.run_id,
        &session_id,
        &mut tracker,
        &mut active_turn,
        &mut frame_sequence,
        &mut last_activity,
        inactivity,
        deadline,
        shutdown,
        &control,
        &observations,
        usage_scope.as_ref(),
        &mut steer_rx,
    )
    .await;
    match terminal {
        ClaudePumpOutcome::Terminal {
            state,
            subagent_rows,
        } => {
            // Rows traversed the pump loop beside the text channel; forward
            // them through the owner channel in emission order ahead of
            // terminal settlement, exactly like text deltas flow. A closed
            // sink or a non-subagent row ends forwarding without disturbing
            // the turn result: only lifecycle and transcript rows ever
            // accumulate in the pump buffer.
            forward_subagent_rows(&observations, subagent_rows).await;
            // Terminal fence: capture the generated title best-effort and
            // carry it on the terminal observation beside settlement, exactly
            // like text deltas flow. A closed sink ends the send without
            // disturbing the turn result.
            settle_claude_terminal_title(&input, &session_id, &mut tracker);
            let terminal_observation = super::super::observation::TerminalObservation::new(
                input.run_id.clone(),
                frame_sequence,
                state,
                None,
                None,
            )
            .with_summary_title(tracker.summary_title().map(str::to_owned));
            let _ = observations
                .send(EngineObservation::Terminal(terminal_observation))
                .await;
            drop(observations);
            finish_turn_result(
                parts,
                Ok(EngineTurnResult { terminal: state }),
                respond,
                runtime.limits.close,
            )
            .await
        }
        ClaudePumpOutcome::Failed {
            error,
            subagent_rows,
        } => {
            forward_subagent_rows(&observations, subagent_rows).await;
            // The fence still captures the title on failure paths and carries
            // it on a Failed terminal observation; dispatcher precedence
            // (forced flags, then observation state, then the owner result)
            // settles exactly as the owner result alone would have.
            settle_claude_terminal_title(&input, &session_id, &mut tracker);
            let terminal_observation = super::super::observation::TerminalObservation::new(
                input.run_id.clone(),
                frame_sequence,
                TerminalState::Failed,
                None,
                None,
            )
            .with_summary_title(tracker.summary_title().map(str::to_owned));
            let _ = observations
                .send(EngineObservation::Terminal(terminal_observation))
                .await;
            finish_turn_result(parts, Err(error), respond, runtime.limits.close).await
        }
    }
}

/// Captures the CLI's generated session title at the terminal fence.
///
/// Best-effort beside settlement: any failure means "no title yet" and the
/// turn settles exactly as it would have without the read.
fn settle_claude_terminal_title(
    input: &super::super::InternalTurnInput,
    session_id: &str,
    tracker: &mut super::super::claude::ClaudePendingTracker,
) {
    if let Some(title) =
        super::super::claude::claude_transcript_title_for_session(&input.project_root, session_id)
    {
        tracker.note_summary_title(title);
    }
}

enum ClaudePumpOutcome {
    Terminal {
        state: super::super::observation::TerminalState,
        subagent_rows: Vec<artisan_domain::Observation>,
    },
    Failed {
        error: EngineOperationError,
        subagent_rows: Vec<artisan_domain::Observation>,
    },
}

/// Forwards buffered subagent rows through the owner observation channel.
///
/// Wraps each domain row into its channel event in buffer order and sends it
/// ahead of terminal settlement. A closed sink or an unexpected row kind
/// ends forwarding without disturbing the turn result; the caller settles
/// the turn exactly as it would have without rows.
async fn forward_subagent_rows(
    observations: &mpsc::Sender<EngineObservation>,
    subagent_rows: Vec<artisan_domain::Observation>,
) {
    for row in subagent_rows {
        let event = match row {
            artisan_domain::Observation::Subagent(observation) => {
                EngineObservation::Subagent(SubagentLifecycleRow::new(observation))
            }
            artisan_domain::Observation::SubagentTranscript(observation) => {
                EngineObservation::SubagentTranscript(SubagentTranscriptRow::new(observation))
            }
            _ => break,
        };
        if observations.send(event).await.is_err() {
            break;
        }
    }
}

/// Services one claude steer delivery from the pump's owned lifeline.
///
/// Writes the stream-input fold line and resolves the delivery from the
/// transport-write outcome: `Ok(())` means the exact bytes reached the
/// provider stdin, never mere channel enqueue. The fold has no correlated
/// provider result — fold timing stays CLI-owned — so the write is the
/// ack. No observation emission precedes it. A closed lifeline rejects
/// typed. Exactly one write attempt is made per delivery, never a retry
/// of an ambiguous write.
pub(crate) async fn service_claude_steer_delivery<W: tokio::io::AsyncWrite + Unpin>(
    stdin: &mut W,
    session_id: &str,
    delivery: SteerDelivery,
) {
    use super::super::claude as claude_runtime;

    let wrote = claude_runtime::steer_live_turn(stdin, session_id, &delivery.text)
        .await
        .is_ok();
    let _ = delivery.ack.send(if wrote {
        Ok(())
    } else {
        Err(SteerError::DeliveryFailed)
    });
}

#[allow(clippy::too_many_arguments)]
async fn claude_pump_loop(
    reader: &mut tokio::io::BufReader<tokio::process::ChildStdout>,
    parts: &mut ChildParts,
    line: &mut String,
    run_id: &artisan_domain::RunId,
    expected_session: &str,
    tracker: &mut super::super::claude::ClaudePendingTracker,
    active_turn: &mut Option<String>,
    frame_sequence: &mut u64,
    last_activity: &mut Instant,
    inactivity: Duration,
    deadline: Instant,
    shutdown: &Arc<CancelHandle>,
    control: &Arc<CancelHandle>,
    observations: &mpsc::Sender<EngineObservation>,
    usage: Option<&super::super::claude::ClaudeUsageScope<'_>>,
    steer_rx: &mut Option<mpsc::Receiver<SteerDelivery>>,
) -> ClaudePumpOutcome {
    let outcome = claude_pump_loop_inner(
        reader,
        parts,
        line,
        run_id,
        expected_session,
        tracker,
        active_turn,
        frame_sequence,
        last_activity,
        inactivity,
        deadline,
        shutdown,
        control,
        observations,
        usage,
        steer_rx,
    )
    .await;
    // Every exit settles queued steers typed: stop/cancel interrupts
    // pending steer requests deterministically instead of leaving
    // `steer_text` on an indefinite ack await.
    let mut buffered: Vec<SteerDelivery> = Vec::new();
    let mut pending: HashMap<u64, oneshot::Sender<Result<(), SteerError>>> = HashMap::new();
    settle_steers_closed(steer_rx, &mut buffered, &mut pending);
    outcome
}

#[expect(
    clippy::too_many_lines,
    reason = "one pump-loop body keeps cancellation, reader, and steer state in one local scope; extraction would thread the whole child state"
)]
#[allow(clippy::too_many_arguments)]
async fn claude_pump_loop_inner(
    reader: &mut tokio::io::BufReader<tokio::process::ChildStdout>,
    parts: &mut ChildParts,
    line: &mut String,
    run_id: &artisan_domain::RunId,
    expected_session: &str,
    tracker: &mut super::super::claude::ClaudePendingTracker,
    active_turn: &mut Option<String>,
    frame_sequence: &mut u64,
    last_activity: &mut Instant,
    inactivity: Duration,
    deadline: Instant,
    shutdown: &Arc<CancelHandle>,
    control: &Arc<CancelHandle>,
    observations: &mpsc::Sender<EngineObservation>,
    usage: Option<&super::super::claude::ClaudeUsageScope<'_>>,
    steer_rx: &mut Option<mpsc::Receiver<SteerDelivery>>,
) -> ClaudePumpOutcome {
    use super::super::claude as claude_runtime;

    let mut exited: Option<std::process::ExitStatus> = None;
    // Emission buffer beside the text channel: drained per applied frame so
    // rows traverse the loop in emission order instead of accumulating in
    // the tracker. Delivery beyond the loop awaits the owner-channel
    // follow-up; see the handoff marker where the pump settles.
    let mut subagent_rows: Vec<artisan_domain::Observation> = Vec::new();
    loop {
        if shutdown.is_cancelled() {
            return ClaudePumpOutcome::Failed {
                error: EngineOperationError::Shutdown,
                subagent_rows: std::mem::take(&mut subagent_rows),
            };
        }
        if control.is_cancelled() {
            // No provider interrupt verb exists on this transport (the
            // adapter settles cancel without one); closing the lifeline is
            // the only turn-side signal before reporting cancellation.
            parts.lifeline.close();
            return ClaudePumpOutcome::Terminal {
                state: TerminalState::Cancelled,
                subagent_rows: std::mem::take(&mut subagent_rows),
            };
        }
        if Instant::now() >= deadline {
            return ClaudePumpOutcome::Failed {
                error: EngineOperationError::Deadline,
                subagent_rows: std::mem::take(&mut subagent_rows),
            };
        }
        if claude_runtime::has_stalled(
            active_turn.is_some(),
            *last_activity,
            inactivity,
            Instant::now(),
        ) {
            return ClaudePumpOutcome::Terminal {
                state: TerminalState::Failed,
                subagent_rows: std::mem::take(&mut subagent_rows),
            };
        }
        let stall_at = last_activity
            .checked_add(inactivity)
            .unwrap_or(deadline)
            .min(deadline);
        line.clear();
        tokio::select! {
            biased;
            () = shutdown.wait() => {
                return ClaudePumpOutcome::Failed {
                    error: EngineOperationError::Shutdown,
                    subagent_rows: std::mem::take(&mut subagent_rows),
                };
            }
            () = control.wait() => {
                parts.lifeline.close();
                return ClaudePumpOutcome::Terminal {
                    state: TerminalState::Cancelled,
                    subagent_rows: std::mem::take(&mut subagent_rows),
                };
            }
            () = tokio::time::sleep_until(deadline) => {
                return ClaudePumpOutcome::Failed {
                    error: EngineOperationError::Deadline,
                    subagent_rows: std::mem::take(&mut subagent_rows),
                };
            }
            () = tokio::time::sleep_until(stall_at) => {
                if claude_runtime::has_stalled(
                    active_turn.is_some(),
                    *last_activity,
                    inactivity,
                    Instant::now(),
                ) {
                    return ClaudePumpOutcome::Terminal {
                        state: TerminalState::Failed,
                        subagent_rows: std::mem::take(&mut subagent_rows),
                    };
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
                // Servicing resolves from the transport write alone and
                // touches neither the observation channel nor the provider
                // read side, so a suspended dispatch drain cannot wedge it.
                service_claude_steer_delivery(&mut parts.lifeline, expected_session, delivery)
                    .await;
            }
            status = parts.child.wait(), if exited.is_none() => {
                match status {
                    Ok(status) => exited = Some(status),
                    Err(_) => {
                        return ClaudePumpOutcome::Failed {
                            error: EngineOperationError::StreamFailed,
                            subagent_rows: std::mem::take(&mut subagent_rows),
                        };
                    }
                }
            }
            read = read_bounded_line(reader, line, CLAUDE_MAX_FRAME_BYTES) => {
                match read {
                    Ok(0) => {
                        // EOF is the observed close: a `result` before it is
                        // a clean turn (modulo exit failure and semantic
                        // failure); EOF before `result` is an external kill,
                        // never cancel and never failure-by-code.
                        let clean_exit = match exited {
                            None => true,
                            Some(status) => status.success(),
                        };
                        let subagent_rows = std::mem::take(&mut subagent_rows);
                        if tracker.result_seen() && !tracker.semantic_failure() && clean_exit {
                            return ClaudePumpOutcome::Terminal {
                                state: TerminalState::Completed,
                                subagent_rows,
                            };
                        }
                        if tracker.result_seen() {
                            return ClaudePumpOutcome::Terminal {
                                state: TerminalState::Failed,
                                subagent_rows,
                            };
                        }
                        return ClaudePumpOutcome::Terminal {
                            state: TerminalState::Interrupted,
                            subagent_rows,
                        };
                    }
                    Ok(_) => {
                        *last_activity = Instant::now();
                        *frame_sequence += 1;
                        let trimmed = line.trim_end_matches(['\r', '\n']).to_owned();
                        if let Ok(event) = claude_runtime::parse_frame(&trimmed, *frame_sequence) {
                            let outcome = claude_runtime::apply_event(
                                event,
                                run_id,
                                expected_session,
                                tracker,
                                active_turn,
                                observations,
                                *frame_sequence,
                                usage,
                            )
                            .await;
                            // Drain beside the text channel: rows traverse
                            // the loop in emission order.
                            subagent_rows.extend(tracker.take_subagent_rows());
                            match outcome {
                                claude_runtime::ClaudeApplyOutcome::Continue { end_input } => {
                                    if end_input {
                                        // `result` seen: EndInput
                                        // equivalent, then the CLI exits
                                        // and EOF classifies the turn.
                                        parts.lifeline.close();
                                    }
                                }
                                claude_runtime::ClaudeApplyOutcome::Terminal(state) => {
                                    return ClaudePumpOutcome::Terminal {
                                        state,
                                        subagent_rows: std::mem::take(&mut subagent_rows),
                                    };
                                }
                            }
                        }
                    }
                    Err(BoundedLineError::LineTooLong) => {
                        return ClaudePumpOutcome::Failed {
                            error: EngineOperationError::FrameTooLarge,
                            subagent_rows: std::mem::take(&mut subagent_rows),
                        };
                    }
                    Err(_) => {
                        return ClaudePumpOutcome::Failed {
                            error: EngineOperationError::StreamFailed,
                            subagent_rows: std::mem::take(&mut subagent_rows),
                        };
                    }
                }
            }
        }
    }
}

pub(crate) async fn read_claude_line<R>(
    reader: &mut R,
    line: &mut String,
    deadline: Instant,
    shutdown: &Arc<CancelHandle>,
    control: &Arc<CancelHandle>,
) -> Result<(), EngineOperationError>
where
    R: tokio::io::AsyncBufRead + Unpin,
{
    line.clear();
    tokio::select! {
        biased;
        () = shutdown.wait() => Err(EngineOperationError::Shutdown),
        () = control.wait() => Err(EngineOperationError::Cancelled),
        () = tokio::time::sleep_until(deadline) => Err(EngineOperationError::Deadline),
        read = read_bounded_line(reader, line, CLAUDE_MAX_FRAME_BYTES) => match read {
            Ok(0) | Err(BoundedLineError::Io | BoundedLineError::InvalidUtf8) => {
                Err(EngineOperationError::ProviderRequestFailed)
            }
            Ok(_) => Ok(()),
            Err(BoundedLineError::LineTooLong) => Err(EngineOperationError::FrameTooLarge),
        },
    }
}
