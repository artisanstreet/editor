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
use super::core::ProviderResponse;
use super::core::SteerDelivery;
use super::core::SteerError;
use super::core::settle_steers_closed;
use super::owner::ClaudeDisplayRefusals;
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
    claude_displays: &mut ClaudeDisplayRefusals,
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
    // capability plus the owner's observed refusals, and applies identically
    // to fresh starts and resumes; older CLIs keep their existing arguments.
    // Internal `highlights` is the default; a context whose server refused it
    // on an observed prior turn downgrades to public `summarized` (see
    // `ClaudeThinkingDisplay`).
    let refusal_context = (
        settings.profile_id().to_owned(),
        selection.model_id().map(|model| model.as_str().to_owned()),
        launch.version().to_owned(),
    );
    let settings = settings.with_thinking_display(
        claude_runtime::ClaudeThinkingDisplay::for_support(launch.thinking_display())
            .with_refusal(claude_displays.refused(&refusal_context)),
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
    let env = settings.spawn_env();
    let mut child =
        match spawn_claude_engine(launch.as_ref(), &request.input.project_root, &args, &env) {
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
        let line = claude_runtime::ClaudeSettings::user_message_payload(
            &session,
            request.input.engine_prompt_text().as_deref(),
            &request.input.prompt,
        );
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
    let init_line = loop {
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
            }) if announced == session_id => break trimmed,
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
    };
    // What the CLI says it can use, read before the request is consumed.
    let skill_roots = claude_runtime::ClaudeSkillRoots {
        config_dir: claude_config_dir(launch.environment()),
        project_root: std::path::PathBuf::from(request.input.project_root.as_str()),
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

    // Best effort and never blocking the run: the inventory updates what the
    // composer offers, not this turn.
    if let Ok(Some(report)) = tokio::task::spawn_blocking(move || {
        claude_runtime::init_inventory(&init_line, &skill_roots)
    })
    .await
    {
        let _ = observations.try_send(EngineObservation::Inventory(report));
    }

    // streaming pump ----------------------------------------------------------
    // Run-local tracking: a resumed session never inherits a previous run's
    // thinking stretches.
    let mut tracker =
        claude_runtime::ClaudePendingTracker::with_thinking_display(settings.thinking_display());
    tracker.attach_shell_tail(super::super::shell_tail::ShellTail::spawn(
        observations.clone(),
    ));
    // A goal set in an earlier run still stands: the CLI restored it on
    // resume without saying so on the stream, so the run says so itself.
    let start_goal = transcript_goal(&input, &session_id).await;
    if let Some(goal) = start_goal.as_ref().filter(|goal| goal.state.stands()) {
        report_goal(&observations, &input.run_id, 0, goal).await;
    }
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
                window_cap: selection
                    .auto_compact_window()
                    .map(artisan_domain::ClaudeAutoCompactWindow::get),
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
                window_cap: attribution.window_cap,
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
    // Background jobs die with the CLI: read what they wrote last before the
    // turn settles, so the follower's sender is gone when the channel closes.
    if let Some(tail) = tracker.take_shell_tail() {
        tail.close().await;
    }
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
            // A completed turn that observed empty thinking stretches under a
            // highlights request records the server refusal for this
            // context; cancelled, failed, and no-thinking turns never infer
            // capability loss.
            if state == TerminalState::Completed && tracker.highlights_refused() {
                claude_displays.mark_refused(refusal_context);
            }
            // Terminal fence: capture the generated title best-effort and
            // carry it on the terminal observation beside settlement, exactly
            // like text deltas flow. A closed sink ends the send without
            // disturbing the turn result.
            settle_claude_terminal_title(&input, &session_id, &mut tracker).await;
            // Whether the goal was met or given up is written only to the
            // transcript; report it when this run changed it.
            if let Some(goal) = transcript_goal(&input, &session_id).await
                && start_goal.as_ref() != Some(&goal)
            {
                report_goal(&observations, &input.run_id, frame_sequence, &goal).await;
            }
            // Print mode never writes a transcript title, so a completed turn
            // on a still-placeholder thread asks Claude to name it. The model
            // has finished, so the turn settles now and naming follows it.
            let needs_title =
                state == TerminalState::Completed && tracker.summary_title().is_none();
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
            if needs_title {
                spawn_claude_title(input, Arc::clone(shutdown));
            }
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
            settle_claude_terminal_title(&input, &session_id, &mut tracker).await;
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
async fn settle_claude_terminal_title(
    input: &super::super::InternalTurnInput,
    session_id: &str,
    tracker: &mut super::super::claude::ClaudePendingTracker,
) {
    let super::super::InternalLaunch::Claude(launch) = &input.launch else {
        return;
    };
    let environment = launch.environment().to_vec();
    let project_root = input.project_root.clone();
    let session_id = session_id.to_owned();
    if let Some(title) = read_transcript_off_loop(move || {
        super::super::claude::claude_transcript_title_for_session(
            &environment,
            &project_root,
            &session_id,
        )
    })
    .await
    {
        tracker.note_summary_title(title);
    }
}

/// The session goal as its transcript has it, when the launch names one.
async fn transcript_goal(
    input: &super::super::InternalTurnInput,
    session_id: &str,
) -> Option<super::super::claude::ClaudeTranscriptGoal> {
    let super::super::InternalLaunch::Claude(launch) = &input.launch else {
        return None;
    };
    let environment = launch.environment().to_vec();
    let project_root = input.project_root.clone();
    let session_id = session_id.to_owned();
    read_transcript_off_loop(move || {
        super::super::claude::claude_transcript_goal_for_session(
            &environment,
            &project_root,
            &session_id,
        )
    })
    .await
}

/// Reads the session transcript on the blocking pool.
///
/// A transcript can run to tens of MiB, and on a busy disk reading it takes
/// seconds; on the Forge's single event-loop thread that would stall lease
/// renewals, request deadlines and delivery for every thread. Best-effort
/// like its callers: a failed read is no answer.
async fn read_transcript_off_loop<T: Send + 'static>(
    read: impl FnOnce() -> Option<T> + Send + 'static,
) -> Option<T> {
    tokio::task::spawn_blocking(read).await.ok().flatten()
}

/// Reports where the session goal stands. Best-effort: a closed sink ends
/// nothing here; the turn result carries on.
async fn report_goal(
    observations: &mpsc::Sender<EngineObservation>,
    run_id: &artisan_domain::RunId,
    frame_sequence: u64,
    goal: &super::super::claude::ClaudeTranscriptGoal,
) {
    let Some(row) = super::super::claude::goal_row(
        run_id,
        frame_sequence,
        goal.state,
        goal.condition.as_deref(),
        goal.reason.as_deref(),
    ) else {
        return;
    };
    let _ = observations.send(EngineObservation::Activity(row)).await;
}

/// Upper bound for one naming run.
const CLAUDE_TITLE_BUDGET: Duration = Duration::from_secs(15);
/// Upper bound for the naming run's JSON result on stdout.
const CLAUDE_TITLE_MAX_BYTES: u64 = 1024 * 1024;

/// Names the conversation with one bounded, tool-less, unpersisted
/// `claude -p` run after the turn settled, delivering the title through the
/// turn's sink. Best-effort: any failure is reported and the thread keeps its
/// placeholder, to be named after the next completed turn.
///
/// The task owns the launch capability, so its engine-use lease stays held
/// until naming ends rather than ending with the turn.
fn spawn_claude_title(input: super::super::InternalTurnInput, shutdown: Arc<CancelHandle>) {
    let super::super::InternalTurnInput {
        run_id,
        project_root,
        prompt,
        launch,
        title_sink,
        ..
    } = input;
    let (Some(sink), super::super::InternalLaunch::Claude(launch)) = (title_sink, launch) else {
        return;
    };
    let prompt = prompt
        .text()
        .map(|text| text.as_str().chars().take(4000).collect::<String>())
        .unwrap_or_default();
    if prompt.trim().is_empty() {
        return;
    }
    tokio::spawn(async move {
        let outcome = tokio::select! {
            () = shutdown.wait() => return,
            outcome = tokio::time::timeout(
                CLAUDE_TITLE_BUDGET,
                run_claude_title(&launch, &project_root, &prompt),
            ) => outcome,
        };
        match outcome {
            Ok(Ok(title)) => {
                let _ = sink.send(title);
            }
            Ok(Err(reason)) => eprintln!(
                "Claude title could not be generated (run {run_id}): {reason}; retrying after the next successful turn"
            ),
            Err(_) => eprintln!(
                "Claude title could not be generated (run {run_id}): the naming run did not finish within {} s; retrying after the next successful turn",
                CLAUDE_TITLE_BUDGET.as_secs()
            ),
        }
    });
}

async fn run_claude_title(
    launch: &artisan_native_engine::VerifiedClaudeLaunch,
    project_root: &artisan_domain::RootPath,
    prompt: &str,
) -> Result<artisan_domain::ThreadTitle, String> {
    use tokio::io::AsyncReadExt;
    let args = [
        "-p".to_owned(),
        "--output-format".to_owned(),
        "json".to_owned(),
        "--no-session-persistence".to_owned(),
        "--model".to_owned(),
        "haiku".to_owned(),
        "--tools".to_owned(),
        String::new(),
        "--setting-sources".to_owned(),
        String::new(),
        "--system-prompt".to_owned(),
        super::codex::TITLE_INSTRUCTIONS.to_owned(),
        "--json-schema".to_owned(),
        super::codex::title_schema().to_string(),
        super::codex::title_request(prompt),
    ];
    let mut child = super::super::process::spawn_claude_engine(launch, project_root, &args, &[])
        .map_err(|error| format!("spawning the naming run: {error}"))?;
    // The request travels as an argument; closing stdin tells the CLI no
    // further input follows.
    drop(child.stdin.take());
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| "the naming run had no stdout".to_owned())?
        .take(CLAUDE_TITLE_MAX_BYTES);
    let stderr = child.stderr.take();
    let mut output = Vec::new();
    let (read, ()) = tokio::join!(stdout.read_to_end(&mut output), async {
        if let Some(stderr) = stderr {
            let _ = tokio::io::copy(
                &mut stderr.take(CLAUDE_TITLE_MAX_BYTES),
                &mut tokio::io::sink(),
            )
            .await;
        }
    });
    read.map_err(|error| format!("reading the naming run's output: {error}"))?;
    let status = child
        .wait()
        .await
        .map_err(|error| format!("waiting for the naming run: {error}"))?;
    if !status.success() {
        return Err(format!("the naming run exited with {status}"));
    }
    let result: serde_json::Value = serde_json::from_slice(&output)
        .map_err(|error| format!("parsing the naming run's result: {error}"))?;
    let structured = result
        .get("structured_output")
        .ok_or_else(|| "the naming run returned no structured output".to_owned())?;
    super::codex::parse_generated_title(&structured.to_string())
        .ok_or_else(|| "the naming run returned an unusable title".to_owned())
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

/// Services one claude delivery from the pump's owned lifeline: a steer
/// text, or a durable approval or question decision written back onto its
/// pending `control_request`.
///
/// A steer writes the stream-input fold line and resolves the delivery from
/// the transport-write outcome: `Ok(())` means the exact bytes reached the
/// provider stdin, never mere channel enqueue. The fold has no correlated
/// provider result — fold timing stays CLI-owned — so the write is the
/// ack. A question answer is recorded on its open `AskUserQuestion` request
/// and the control response is written once the last question of that
/// request is answered, so the tool sees every answer at once. No
/// observation emission precedes any write. A closed lifeline rejects
/// typed. Exactly one write attempt is made per delivery, never a retry of
/// an ambiguous write.
pub(crate) async fn service_claude_delivery<W: tokio::io::AsyncWrite + Unpin>(
    tracker: &mut super::super::claude::ClaudePendingTracker,
    stdin: &mut W,
    session_id: &str,
    delivery: SteerDelivery,
) {
    use super::super::claude as claude_runtime;

    let result = match &delivery.response {
        Some(ProviderResponse::Approval {
            approval_id,
            approved,
        }) => claude_runtime::answer_approval(stdin, tracker, approval_id, approval_id, *approved)
            .await
            .map_err(|_| SteerError::DeliveryFailed),
        Some(ProviderResponse::Question {
            question_id,
            answers,
        }) => match tracker.record_question_answer(question_id, answers) {
            Ok(None) => Ok(()),
            Ok(Some(reply)) => claude_runtime::write_line(stdin, &reply)
                .await
                .map_err(|_| SteerError::DeliveryFailed),
            Err(_) => Err(SteerError::DeliveryFailed),
        },
        None => {
            let written = claude_runtime::steer_live_turn(
                stdin,
                session_id,
                &delivery.text,
                &delivery.images,
            )
            .await;
            match written {
                Ok(()) => {
                    // The folded message opens (or joins) a root turn.
                    tracker.note_turn_opened();
                    Ok(())
                }
                Err(error) => {
                    eprintln!(
                        "claude steer write failed (session {session_id}, request {}): {}",
                        delivery.request_id,
                        artisan_domain::ErrorChain(&error)
                    );
                    Err(SteerError::DeliveryFailed)
                }
            }
        }
    };
    let _ = delivery.ack.send(result);
}

/// How long a session with nothing left to say keeps stdin open after its
/// last background task ends.
///
/// The ending task queues a follow-up turn whose first frame re-opens the
/// session well inside this window; the grace only bounds the case where no
/// follow-up comes, so the CLI is still told to exit.
const CLAUDE_SETTLED_INPUT_GRACE: Duration = Duration::from_secs(30);

/// The terminal state of a session whose turn reported its `result` with
/// nothing left to wait for.
///
/// The turn is over at that `result`, so the run settles on it instead of
/// on the CLI's exit: with input closed the CLI keeps running while its
/// background shells do (reporting them later in a follow-up turn), and
/// shell jobs are turn-bounded, so teardown ends the process group, shells
/// included.
fn settled_turn_state(tracker: &super::super::claude::ClaudePendingTracker) -> TerminalState {
    if tracker.semantic_failure() {
        TerminalState::Failed
    } else {
        TerminalState::Completed
    }
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
    // `steer_message` on an indefinite ack await.
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
    // Armed while the session is settled with stdin still open; see
    // `CLAUDE_SETTLED_INPUT_GRACE`.
    let mut settled_close_at: Option<Instant> = None;
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
        // A provider waiting on the user (an open approval or question) is
        // not stalled, however long the user takes.
        if claude_runtime::has_stalled(
            active_turn.is_some() && !tracker.waiting_on_user(),
            *last_activity,
            inactivity,
            Instant::now(),
        ) {
            return ClaudePumpOutcome::Terminal {
                state: TerminalState::Failed,
                subagent_rows: std::mem::take(&mut subagent_rows),
            };
        }
        let stall_at = if tracker.waiting_on_user() {
            deadline
        } else {
            last_activity
                .checked_add(inactivity)
                .unwrap_or(deadline)
                .min(deadline)
        };
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
                    active_turn.is_some() && !tracker.waiting_on_user(),
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
            () = async {
                match settled_close_at {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending().await,
                }
            } => {
                // Nothing followed the last background task: the reported
                // turn is the last one, so it settles now.
                settled_close_at = None;
                if tracker.input_settled() {
                    parts.lifeline.close();
                    return ClaudePumpOutcome::Terminal {
                        state: settled_turn_state(tracker),
                        subagent_rows: std::mem::take(&mut subagent_rows),
                    };
                }
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
                service_claude_delivery(tracker, &mut parts.lifeline, expected_session, delivery)
                    .await;
                if !tracker.input_settled() {
                    settled_close_at = None;
                }
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
                                        // `result` seen with nothing
                                        // running behind it: the turn is
                                        // over, so it settles now rather
                                        // than when the CLI exits.
                                        parts.lifeline.close();
                                        return ClaudePumpOutcome::Terminal {
                                            state: settled_turn_state(tracker),
                                            subagent_rows: std::mem::take(&mut subagent_rows),
                                        };
                                    }
                                    settled_close_at = if tracker.input_settled()
                                        && parts.lifeline.is_open()
                                    {
                                        settled_close_at
                                            .or(Some(Instant::now() + CLAUDE_SETTLED_INPUT_GRACE))
                                    } else {
                                        None
                                    };
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

/// `CLAUDE_CONFIG_DIR` from a launch environment.
fn claude_config_dir(
    environment: &[(std::ffi::OsString, std::ffi::OsString)],
) -> Option<std::path::PathBuf> {
    environment
        .iter()
        .find(|(name, _)| name == "CLAUDE_CONFIG_DIR")
        .map(|(_, value)| std::path::PathBuf::from(value))
}

#[cfg(all(test, unix))]
mod closed_input_tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::Duration;

    use artisan_transport::CancelHandle;
    use tokio::sync::mpsc;
    use tokio::time::Instant;

    use super::super::super::claude::ClaudePendingTracker;
    use super::super::super::observation::{EngineObservation, TerminalState};
    use super::super::super::process::{
        ChildParts, LifelineWriter, StderrCounter, spawn_script_engine,
    };
    use super::{ClaudePumpOutcome, claude_pump_loop};

    const SESSION: &str = "session-closed-input";

    /// A stand-in CLI: prints `frames`, then runs `tail` with stdin ignored.
    struct ScriptCli {
        directory: PathBuf,
    }

    impl ScriptCli {
        fn new(frames: &[String], tail: &str) -> Self {
            use std::os::unix::fs::PermissionsExt as _;

            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time after epoch")
                .as_nanos();
            let directory = std::env::temp_dir().join(format!(
                "artisan-claude-closed-input-{}-{nonce}",
                std::process::id()
            ));
            std::fs::create_dir_all(&directory).expect("script dir");
            std::fs::write(directory.join("frames.jsonl"), frames.join("\n") + "\n")
                .expect("frames");
            let script = format!("#!/bin/sh\ncat \"$(dirname \"$0\")/frames.jsonl\"\n{tail}\n");
            let path = directory.join("cli.sh");
            std::fs::write(&path, script).expect("script");
            let mut permissions = std::fs::metadata(&path).expect("meta").permissions();
            permissions.set_mode(0o700);
            std::fs::set_permissions(&path, permissions).expect("chmod");
            Self { directory }
        }

        fn program(&self) -> PathBuf {
            self.directory.join("cli.sh")
        }
    }

    impl Drop for ScriptCli {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }

    fn reply(id: &str, text: &str) -> String {
        format!(
            r#"{{"type":"assistant","message":{{"id":"{id}","stop_reason":"end_turn","content":[{{"type":"text","text":"{text}"}}]}}}}"#
        )
    }

    fn result() -> String {
        format!(
            r#"{{"type":"result","subtype":"success","session_id":"{SESSION}","is_error":false}}"#
        )
    }

    /// Pumps one turn of `program` and returns its terminal state, how long
    /// the pump took, and every text the turn emitted.
    async fn pump(program: &Path) -> (Option<TerminalState>, Duration, Vec<String>) {
        let mut child = spawn_script_engine(program).expect("script spawns");
        let lifeline = LifelineWriter::take(&mut child);
        let stdout = child.stdout.take().expect("stdout");
        let stderr_counter = StderrCounter::new(child.stderr.take(), 4096);
        let mut parts = ChildParts {
            child,
            lifeline,
            stdout: None,
            stderr_counter,
        };
        let mut reader = tokio::io::BufReader::new(stdout);
        let run_id = artisan_domain::RunId::parse("closed-input-run").expect("run id");
        let mut tracker = ClaudePendingTracker::new();
        let (observations, mut received) = mpsc::channel(64);
        let started = Instant::now();
        let outcome = claude_pump_loop(
            &mut reader,
            &mut parts,
            &mut String::new(),
            &run_id,
            SESSION,
            &mut tracker,
            &mut None,
            &mut 0,
            &mut Instant::now(),
            Duration::from_secs(60),
            Instant::now() + Duration::from_secs(60),
            &Arc::new(CancelHandle::new()),
            &Arc::new(CancelHandle::new()),
            &observations,
            None,
            &mut None,
        )
        .await;
        let elapsed = started.elapsed();
        let _ = parts.child.start_kill();
        let _ = parts.child.wait().await;
        drop(observations);
        let mut texts = Vec::new();
        while let Some(observation) = received.recv().await {
            match observation {
                EngineObservation::TextSnapshot(snapshot) => texts.push(snapshot.text().to_owned()),
                EngineObservation::TextDelta(delta) => texts.push(delta.delta().to_owned()),
                _ => {}
            }
        }
        let terminal = match outcome {
            ClaudePumpOutcome::Terminal { state, .. } => Some(state),
            ClaudePumpOutcome::Failed { .. } => None,
        };
        (terminal, elapsed, texts)
    }

    #[tokio::test]
    async fn the_turn_settles_at_its_result_while_background_shells_hold_the_cli() {
        // CLI 2.1.296 answers, reports `result`, then keeps running while a
        // background shell does instead of exiting once input closes. The
        // turn is over at the `result`, so the reply settles there.
        let cli = ScriptCli::new(
            &[reply("msg-1", "Want me to implement that?"), result()],
            "sleep 60",
        );
        let (terminal, elapsed, texts) = pump(&cli.program()).await;
        assert_eq!(terminal, Some(TerminalState::Completed));
        assert!(
            elapsed < Duration::from_secs(2),
            "the turn settles at its result, not when the shell ends: {elapsed:?}"
        );
        assert!(texts.concat().contains("Want me to implement that?"));
    }

    #[tokio::test]
    async fn a_result_with_an_agent_still_running_keeps_the_run_open() {
        // A background agent reports back in a follow-up turn, so the first
        // `result` is not the end; the follow-up's is.
        let agent = r#"{"type":"system","subtype":"background_tasks_changed","tasks":[{"task_id":"a1","task_type":"local_agent","description":"Implement"}]}"#;
        let done = r#"{"type":"system","subtype":"background_tasks_changed","tasks":[]}"#;
        let cli = ScriptCli::new(
            &[
                agent.to_owned(),
                reply("msg-1", "Waiting on the agent."),
                result(),
            ],
            &format!(
                "sleep 1\nprintf '%s\\n' '{done}' '{}' '{}'\nsleep 60",
                reply("msg-2", "The agent finished."),
                result()
            ),
        );
        let (terminal, elapsed, texts) = pump(&cli.program()).await;
        assert_eq!(terminal, Some(TerminalState::Completed));
        assert!(
            elapsed < Duration::from_secs(4),
            "settles at the follow-up's result: {elapsed:?}"
        );
        assert!(texts.concat().contains("The agent finished."));
    }
}
