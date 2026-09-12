//! Hermes executor seam: one finite gateway WebSocket turn, its pump
//! loop, steer delivery, and provider error mappers.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use artisan_transport::CancelHandle;
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
use super::failures::map_readiness_error;
use super::turn_common::ConfiguredRuntime;
use super::turn_common::ConfiguredTurnRequest;
use super::turn_common::finish_configured_start;
use super::turn_common::finish_turn_result;
use super::turn_common::phase_deadline;
use super::turn_common::wait_for_authorization;

/// Executes one finite Hermes turn over the private-service gateway WebSocket.
///
/// Single-owner match arm beside the Codex/Claude executors: no second task,
/// no second queue. Mints the dashboard session token, spawns the verified
/// service, drives `HERMES_BACKEND_READY` readiness, connects the JSON-RPC
/// gateway, validates the live `model.options` inventory against the durable
/// selection, opens (or resumes with original-model enforcement behind the H3
/// gate: same engine, explicit target model, recorded service >= 0.20.0)
/// exactly one session, passes the bind authorization gate, submits one prompt, then
/// pumps the stream. Text deltas normalize onto the shared S1a vocabulary;
/// approval/question frames populate the pending tracker with no
/// control-flow side effect; images fail closed before spawn. Stall, failure,
/// and close-before-terminal map distinctly; teardown reuses the owner
/// process contract and quarantines on unobserved reaps.
#[expect(
    clippy::too_many_lines,
    reason = "one configured turn is a single linear protocol sequence over the spawned child; extraction would thread the full child state"
)]
pub(super) async fn execute_hermes_turn(
    request: ConfiguredTurnRequest,
    runtime: ConfiguredRuntime,
    shutdown: &Arc<CancelHandle>,
) -> Execution {
    use super::super::hermes as hermes_runtime;
    use super::super::process::spawn_hermes_engine;

    let artisan_domain::EngineSelection::Hermes(selection) =
        request.input.settings.config().selection()
    else {
        return request.fail(EngineOperationError::Configuration);
    };
    let settings = hermes_runtime::HermesSettings::from_selection(selection);
    if settings.profile_id() != request.input.launch.profile_id() {
        return request.fail(EngineOperationError::Configuration);
    }
    let super::super::InternalLaunch::Hermes(launch) = &request.input.launch else {
        return request.fail(EngineOperationError::Configuration);
    };
    // H3 continuation gate: same-engine is fenced by the dispatcher (hermes
    // bindings only); the owner additionally requires an explicit target
    // model and service >= 0.20.0. Anything else is typed incompatible —
    // never a silent fresh start and never a cross-engine resume. The live
    // model.options inventory below pre-validates the exact target model
    // before the resume request.
    let resume_stored_session_id: Option<String> = match &request.input.continuation {
        None => None,
        Some(continuation) => {
            let gate = hermes_runtime::check_hermes_native_continuation(
                &hermes_runtime::HermesContinuationGateInput {
                    service_version: launch.version(),
                    target_model: Some(settings.model_id()),
                    advertised_models: None,
                    same_engine: true,
                },
            );
            if !matches!(gate, hermes_runtime::HermesContinuationDecision::Compatible) {
                return request.fail(EngineOperationError::Configuration);
            }
            Some(continuation.provider_session_id().to_owned())
        }
    };
    if let Err(error) = hermes_runtime::reject_image_attachments(&request.input.prompt) {
        return request.fail(map_hermes_turn_error(&error));
    }
    let Some(session_token) = hermes_runtime::new_session_token() else {
        return request.fail(EngineOperationError::EntropyFailed);
    };
    let Ok(mut child) = spawn_hermes_engine(launch, &request.input.project_root, &session_token)
    else {
        return request.fail(EngineOperationError::SpawnFailed);
    };
    let stdin_opt = child.stdin.take();
    let stdout_opt = child.stdout.take();
    let stderr_opt = child.stderr.take();
    let lifeline = LifelineWriter::take(&mut child);
    let stderr_counter = StderrCounter::new(stderr_opt, runtime.bounds.stderr_cap_bytes);
    let (Some(_held_stdin), Some(stdout)) = (stdin_opt, stdout_opt) else {
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
    let mut stdout = stdout;
    let port = match hermes_runtime::drive_service_readiness(
        &mut stdout,
        &mut parts,
        phase_deadline(runtime.limits.readiness, request.deadline),
        shutdown,
        &request.control,
        runtime.bounds.max_readiness_line,
    )
    .await
    {
        Ok(port) => port,
        Err(error) => {
            drop(stdout);
            return finish_configured_start(
                request,
                parts,
                map_readiness_error(error),
                runtime.limits.close,
            )
            .await;
        }
    };
    drop(stdout);
    let address =
        std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), port);
    let connect_scope = hermes_runtime::RequestScope {
        deadline: phase_deadline(runtime.limits.health, request.deadline),
        cancel: &request.control,
        shutdown,
    };
    let mut client =
        match hermes_runtime::GatewayClient::connect(address, &session_token, &connect_scope).await
        {
            Ok(client) => client,
            Err(error) => {
                return finish_configured_start(
                    request,
                    parts,
                    map_hermes_gateway_error(&error),
                    runtime.limits.close,
                )
                .await;
            }
        };
    let provider_scope = hermes_runtime::RequestScope {
        deadline: phase_deadline(runtime.limits.prompt, request.deadline),
        cancel: &request.control,
        shutdown,
    };
    let mut early_events = Vec::new();
    let inventory_value = match client
        .request(
            "model.options",
            serde_json::json!({
                "explicit_only": true,
                "include_unconfigured": false,
                "refresh": false,
            }),
            &provider_scope,
        )
        .await
    {
        Ok((value, events)) => {
            early_events.extend(events);
            value
        }
        Err(error) => {
            client.close().await;
            return finish_configured_start(
                request,
                parts,
                map_hermes_gateway_error(&error),
                runtime.limits.close,
            )
            .await;
        }
    };
    let inventory = match hermes_runtime::validate_model_options_inventory(&inventory_value) {
        Ok(inventory) => inventory,
        Err(error) => {
            client.close().await;
            return finish_configured_start(
                request,
                parts,
                map_hermes_inventory_error(error),
                runtime.limits.close,
            )
            .await;
        }
    };
    if !hermes_runtime::inventory_supports(&inventory, settings.route_id(), settings.model_id()) {
        client.close().await;
        return finish_configured_start(
            request,
            parts,
            EngineOperationError::Configuration,
            runtime.limits.close,
        )
        .await;
    }
    let open_input = hermes_runtime::OpenSessionInput {
        settings: &settings,
        project_root: request.input.project_root.as_str(),
        guidance_sections: &[],
        resume_stored_session_id: resume_stored_session_id.as_deref(),
    };
    let opened = match hermes_runtime::open_session(&mut client, &open_input, &provider_scope).await
    {
        Ok(opened) => opened,
        Err(error) => {
            client.close().await;
            return finish_configured_start(
                request,
                parts,
                map_hermes_session_error(&error),
                runtime.limits.close,
            )
            .await;
        }
    };
    early_events.extend(opened.setup_events);

    // Bind authorization gate: the dispatcher binds the durable stored
    // session before exactly one prompt is authorized. Destructure here so
    // the prepared session carries the exact durable identity.
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
        .send(Ok(PreparedSession::new(opened.durable_session_id.clone())))
        .is_err()
    {
        client.close().await;
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
        client.close().await;
        return finish_turn_result(parts, Err(error), respond, runtime.limits.close).await;
    }

    // Prompt submit ---------------------------------------------------------
    // Images were rejected before spawn; only text travels here.
    let prompt_text = input
        .prompt
        .text()
        .map(|text| text.as_str().to_owned())
        .unwrap_or_default();
    let submit_scope = hermes_runtime::RequestScope {
        deadline: phase_deadline(runtime.limits.prompt, deadline),
        cancel: &control,
        shutdown,
    };
    if client
        .request(
            "prompt.submit",
            serde_json::json!({
                "session_id": opened.runtime_session_id,
                "text": prompt_text,
            }),
            &submit_scope,
        )
        .await
        .is_err()
    {
        client.close().await;
        return finish_turn_result(
            parts,
            Err(EngineOperationError::ProviderRequestFailed),
            respond,
            runtime.limits.close,
        )
        .await;
    }

    // Streaming pump ----------------------------------------------------------
    let mut normalizer = hermes_runtime::HermesNormalizer::new();
    let mut tracker = hermes_runtime::HermesPendingTracker::new();
    let mut active_turn: Option<String> = None;
    let mut frame_sequence: u64 = 0;
    let mut last_activity = Instant::now();
    let terminal = hermes_pump_loop(
        &mut client,
        &mut normalizer,
        &mut tracker,
        &settings,
        &input.run_id,
        input.thread_id.as_ref(),
        &opened.runtime_session_id,
        early_events,
        &mut active_turn,
        &mut frame_sequence,
        &mut last_activity,
        runtime.limits.sse,
        deadline,
        shutdown,
        &control,
        &observations,
        &mut parts,
        &mut steer_rx,
    )
    .await;
    let close_scope = hermes_runtime::RequestScope {
        deadline: phase_deadline(runtime.limits.close, deadline),
        cancel: &control,
        shutdown,
    };
    let _ = client
        .request(
            "session.close",
            serde_json::json!({ "session_id": opened.runtime_session_id }),
            &close_scope,
        )
        .await;
    client.close().await;
    match terminal {
        HermesPumpOutcome::Terminal(state) => {
            drop(observations);
            finish_turn_result(
                parts,
                Ok(EngineTurnResult { terminal: state }),
                respond,
                runtime.limits.close,
            )
            .await
        }
        HermesPumpOutcome::Failed(error) => {
            finish_turn_result(parts, Err(error), respond, runtime.limits.close).await
        }
    }
}

enum HermesPumpOutcome {
    Terminal(super::super::observation::TerminalState),
    Failed(EngineOperationError),
}

/// Services one hermes steer delivery over the pump-owned gateway client.
///
/// Issues the `session.steer` request under the pump's scope and resolves
/// the delivery from the correlated gateway result at once, returning the
/// interleaved events for the pump to project afterwards. The gateway
/// request round-trip buffers interleaved events client-side without
/// touching the observation channel, and the ack precedes projection while
/// the dispatch arm keeps draining, so a provider streaming past channel
/// capacity before answering wedges neither side. A failed or timed-out
/// request resolves typed failure with exactly one attempt — never a
/// retry of an ambiguous write.
pub(crate) async fn service_hermes_steer_delivery(
    client: &mut super::super::hermes::GatewayClient,
    runtime_session: &str,
    delivery: SteerDelivery,
    scope: &super::super::hermes::RequestScope<'_>,
) -> Vec<super::super::hermes::HermesEvent> {
    if let Ok(events) = super::super::hermes::steer_live_turn(client, runtime_session, &delivery.text, scope)
        .await {
        let _ = delivery.ack.send(Ok(()));
        events
    } else {
        let _ = delivery.ack.send(Err(SteerError::DeliveryFailed));
        Vec::new()
    }
}

#[allow(clippy::too_many_arguments)]
async fn hermes_pump_loop(
    client: &mut super::super::hermes::GatewayClient,
    normalizer: &mut super::super::hermes::HermesNormalizer,
    tracker: &mut super::super::hermes::HermesPendingTracker,
    settings: &super::super::hermes::HermesSettings,
    run_id: &artisan_domain::RunId,
    thread_id: Option<&artisan_domain::ThreadId>,
    runtime_session: &str,
    early_events: Vec<super::super::hermes::HermesEvent>,
    active_turn: &mut Option<String>,
    frame_sequence: &mut u64,
    last_activity: &mut Instant,
    inactivity: Duration,
    deadline: Instant,
    shutdown: &Arc<CancelHandle>,
    control: &Arc<CancelHandle>,
    observations: &mpsc::Sender<EngineObservation>,
    parts: &mut ChildParts,
    steer_rx: &mut Option<mpsc::Receiver<SteerDelivery>>,
) -> HermesPumpOutcome {
    let outcome = hermes_pump_loop_inner(
        client,
        normalizer,
        tracker,
        settings,
        run_id,
        thread_id,
        runtime_session,
        early_events,
        active_turn,
        frame_sequence,
        last_activity,
        inactivity,
        deadline,
        shutdown,
        control,
        observations,
        parts,
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
    reason = "one pump-loop body keeps gateway, tracker, and steer state in one local scope; extraction would thread the whole gateway state"
)]
#[allow(clippy::too_many_arguments)]
async fn hermes_pump_loop_inner(
    client: &mut super::super::hermes::GatewayClient,
    normalizer: &mut super::super::hermes::HermesNormalizer,
    tracker: &mut super::super::hermes::HermesPendingTracker,
    settings: &super::super::hermes::HermesSettings,
    run_id: &artisan_domain::RunId,
    thread_id: Option<&artisan_domain::ThreadId>,
    runtime_session: &str,
    early_events: Vec<super::super::hermes::HermesEvent>,
    active_turn: &mut Option<String>,
    frame_sequence: &mut u64,
    last_activity: &mut Instant,
    inactivity: Duration,
    deadline: Instant,
    shutdown: &Arc<CancelHandle>,
    control: &Arc<CancelHandle>,
    observations: &mpsc::Sender<EngineObservation>,
    parts: &mut ChildParts,
    steer_rx: &mut Option<mpsc::Receiver<SteerDelivery>>,
) -> HermesPumpOutcome {
    use super::super::hermes as hermes_runtime;

    for event in &early_events {
        *frame_sequence = frame_sequence.wrapping_add(1);
        *last_activity = Instant::now();
        if let Some(terminal) = hermes_runtime::apply_observations(
            normalizer,
            event,
            hermes_runtime::ApplyContext {
                run_id,
                thread_id,
                settings,
                runtime_session_id: runtime_session,
                tracker,
                active_turn,
                observations,
                frame_sequence: *frame_sequence,
            },
        )
        .await
        {
            return HermesPumpOutcome::Terminal(terminal);
        }
    }
    loop {
        if shutdown.is_cancelled() {
            return HermesPumpOutcome::Failed(EngineOperationError::Shutdown);
        }
        if control.is_cancelled() {
            let scope = hermes_runtime::RequestScope {
                deadline: Instant::now()
                    .checked_add(Duration::from_secs(5))
                    .unwrap_or(deadline)
                    .min(deadline),
                cancel: control,
                shutdown,
            };
            let _ = hermes_runtime::interrupt_live_turn(client, runtime_session, &scope).await;
            return HermesPumpOutcome::Terminal(TerminalState::Cancelled);
        }
        if Instant::now() >= deadline {
            return HermesPumpOutcome::Failed(EngineOperationError::Deadline);
        }
        if hermes_runtime::has_stalled(
            active_turn.is_some(),
            *last_activity,
            inactivity,
            Instant::now(),
        ) {
            return HermesPumpOutcome::Terminal(TerminalState::Failed);
        }
        let stall_at = last_activity
            .checked_add(inactivity)
            .unwrap_or(deadline)
            .min(deadline);
        tokio::select! {
            biased;
            () = shutdown.wait() => return HermesPumpOutcome::Failed(EngineOperationError::Shutdown),
            () = control.wait() => {
                let scope = hermes_runtime::RequestScope {
                    deadline: Instant::now()
                        .checked_add(Duration::from_secs(5))
                        .unwrap_or(deadline)
                        .min(deadline),
                    cancel: control,
                    shutdown,
                };
                let _ = hermes_runtime::interrupt_live_turn(client, runtime_session, &scope).await;
                return HermesPumpOutcome::Terminal(TerminalState::Cancelled);
            }
            () = tokio::time::sleep_until(deadline) => {
                return HermesPumpOutcome::Failed(EngineOperationError::Deadline);
            }
            () = tokio::time::sleep_until(stall_at) => {
                if hermes_runtime::has_stalled(
                    active_turn.is_some(),
                    *last_activity,
                    inactivity,
                    Instant::now(),
                ) {
                    return HermesPumpOutcome::Terminal(TerminalState::Failed);
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
                // Servicing resolves the ack from the gateway round-trip
                // before projecting interleaved events, so a suspended
                // dispatch drain cannot wedge the waiter: the ack is already
                // home while the pump still delivers every event in order.
                let scope = hermes_runtime::RequestScope {
                    deadline,
                    cancel: control,
                    shutdown,
                };
                let events = service_hermes_steer_delivery(
                    client,
                    runtime_session,
                    delivery,
                    &scope,
                )
                .await;
                let mut terminal = None;
                for event in &events {
                    *frame_sequence = frame_sequence.wrapping_add(1);
                    *last_activity = Instant::now();
                    if let Some(state) = hermes_runtime::apply_observations(
                        normalizer,
                        event,
                        hermes_runtime::ApplyContext {
                            run_id,
                            thread_id,
                            settings,
                            runtime_session_id: runtime_session,
                            tracker,
                            active_turn,
                            observations,
                            frame_sequence: *frame_sequence,
                        },
                    )
                    .await
                    {
                        terminal = Some(state);
                        break;
                    }
                }
                if let Some(state) = terminal {
                    return HermesPumpOutcome::Terminal(state);
                }
            }
            event = client.next_event(control, shutdown) => {
                match event {
                    Ok(event) => {
                        *last_activity = Instant::now();
                        *frame_sequence = frame_sequence.wrapping_add(1);
                        if let Some(terminal) = hermes_runtime::apply_observations(
                            normalizer,
                            &event,
                            hermes_runtime::ApplyContext {
                                run_id,
                                thread_id,
                                settings,
                                runtime_session_id: runtime_session,
                                tracker,
                                active_turn,
                                observations,
                                frame_sequence: *frame_sequence,
                            },
                        )
                        .await
                        {
                            return HermesPumpOutcome::Terminal(terminal);
                        }
                    }
                    Err(hermes_runtime::GatewayError::Closed) => {
                        return HermesPumpOutcome::Terminal(TerminalState::Interrupted);
                    }
                    Err(hermes_runtime::GatewayError::Shutdown) => {
                        return HermesPumpOutcome::Failed(EngineOperationError::Shutdown);
                    }
                    Err(hermes_runtime::GatewayError::Cancelled) => {
                        return HermesPumpOutcome::Failed(EngineOperationError::Cancelled);
                    }
                    Err(hermes_runtime::GatewayError::Timeout) => {
                        return HermesPumpOutcome::Failed(EngineOperationError::Deadline);
                    }
                    Err(_) => {
                        return HermesPumpOutcome::Failed(EngineOperationError::StreamFailed);
                    }
                }
            }
        }
    }
}

pub(super) fn map_hermes_turn_error(
    error: &super::super::hermes::HermesTurnError,
) -> EngineOperationError {
    match error {
        super::super::hermes::HermesTurnError::Configuration
        | super::super::hermes::HermesTurnError::ImagesUnsupported => {
            EngineOperationError::Configuration
        }
        super::super::hermes::HermesTurnError::StreamFailed => EngineOperationError::StreamFailed,
    }
}

fn map_hermes_gateway_error(error: &super::super::hermes::GatewayError) -> EngineOperationError {
    match error {
        super::super::hermes::GatewayError::Shutdown => EngineOperationError::Shutdown,
        super::super::hermes::GatewayError::Cancelled => EngineOperationError::Cancelled,
        super::super::hermes::GatewayError::Timeout => EngineOperationError::Deadline,
        _ => EngineOperationError::ProviderRequestFailed,
    }
}

fn map_hermes_session_error(error: &super::super::hermes::SessionError) -> EngineOperationError {
    match error {
        super::super::hermes::SessionError::Shutdown => EngineOperationError::Shutdown,
        super::super::hermes::SessionError::Cancelled => EngineOperationError::Cancelled,
        super::super::hermes::SessionError::Deadline => EngineOperationError::Deadline,
        super::super::hermes::SessionError::IncompatibleVersion => {
            EngineOperationError::IncompatibleVersion
        }
        super::super::hermes::SessionError::Configuration => EngineOperationError::Configuration,
        super::super::hermes::SessionError::ProviderRequestFailed
        | super::super::hermes::SessionError::StreamFailed => {
            EngineOperationError::ProviderRequestFailed
        }
    }
}

fn map_hermes_inventory_error(error: super::super::hermes::InventoryError) -> EngineOperationError {
    match error {
        super::super::hermes::InventoryError::InvalidShape
        | super::super::hermes::InventoryError::DuplicateModel => {
            EngineOperationError::Configuration
        }
    }
}
