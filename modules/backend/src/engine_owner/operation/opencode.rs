//! Configured `OpenCode2` executor seam: process preparation, session
//! creation and authorization, one authorized turn, stream usage scope,
//! and abort-after-session settlement.

use std::sync::Arc;

use artisan_domain::RunId;
use artisan_transport::CancelHandle;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::time::Instant;

use super::super::http::CreateSessionInput;
use super::super::http::HealthSecret;
use super::super::http::PromptFile;
use super::super::http::PromptInput;
use super::super::http::ResumeInput;
use super::super::http::ResumeSelection;
use super::super::http::perform_create_session;
use super::super::http::perform_interrupt;
use super::super::http::perform_prompt;
use super::super::http::perform_resume;
use super::super::observation::EngineObservation;
use super::super::process::ChildParts;
use super::super::process::LifelineWriter;
use super::super::process::StderrCounter;
use super::super::process::spawn_configured_engine;
use super::super::readiness::ReadinessError;
use super::super::readiness::ValidatedEndpoint;
use super::super::stream::StreamError;
use super::super::stream::StreamInput;
use super::super::stream::StreamState;
use super::super::stream::StreamUsageContext;
use super::super::stream::follow_stream_for_run_with_state;
use super::bootstrap::drive_readiness;
use super::core::EngineOperationError;
use super::core::EngineTurnResult;
use super::core::Execution;
use super::core::PreparedSession;
use super::core::TurnResult;
use super::failures::map_health_error;
use super::failures::map_prompt_error;
use super::failures::map_readiness_error;
use super::failures::map_resume_error;
use super::failures::map_stream_error;
use super::turn_common::ConfiguredRuntime;
use super::turn_common::ConfiguredTurnRequest;
use super::turn_common::finish_configured_start;
use super::turn_common::finish_turn_result;
use super::turn_common::phase_deadline;
use super::turn_common::wait_for_authorization;

struct ConfiguredProcess {
    request: ConfiguredTurnRequest,
    runtime: ConfiguredRuntime,
    secret: HealthSecret,
    parts: ChildParts,
    endpoint: ValidatedEndpoint,
}

struct PreparedConfiguredSession {
    input: super::super::InternalTurnInput,
    deadline: Instant,
    control: Arc<CancelHandle>,
    prepared: oneshot::Sender<Result<PreparedSession, EngineOperationError>>,
    authorize: oneshot::Receiver<()>,
    observations: mpsc::Sender<EngineObservation>,
    respond: oneshot::Sender<TurnResult>,
    parts: ChildParts,
    endpoint: ValidatedEndpoint,
    secret: HealthSecret,
    runtime: ConfiguredRuntime,
    session: String,
    resume: bool,
    stream_after: Option<u64>,
}

struct ConfiguredSession {
    input: super::super::InternalTurnInput,
    deadline: Instant,
    control: Arc<CancelHandle>,
    authorize: oneshot::Receiver<()>,
    observations: mpsc::Sender<EngineObservation>,
    respond: oneshot::Sender<TurnResult>,
    parts: ChildParts,
    endpoint: ValidatedEndpoint,
    secret: HealthSecret,
    runtime: ConfiguredRuntime,
    session: String,
    resume: bool,
    stream_after: Option<u64>,
    stream_state: StreamState,
}

impl ConfiguredSession {
    async fn abort(self, shutdown: &Arc<CancelHandle>, cause: EngineOperationError) -> Execution {
        let ConfiguredSession {
            input,
            deadline,
            control: _,
            authorize: _,
            observations,
            respond,
            parts,
            endpoint,
            secret,
            runtime,
            session,
            resume: _,
            stream_after,
            stream_state,
        } = self;
        abort_after_session(AbortAfterSession {
            parts,
            endpoint: &endpoint,
            secret: &secret,
            runtime: &runtime,
            session: &session,
            run_id: &input.run_id,
            stream_after,
            stream_state,
            usage_context: stream_usage_context(&input),
            observations,
            respond,
            shutdown,
            cause,
            attempt_deadline: deadline,
        })
        .await
    }
}

pub(super) async fn execute_configured_turn(
    request: ConfiguredTurnRequest,
    runtime: ConfiguredRuntime,
    shutdown: &Arc<CancelHandle>,
) -> Execution {
    let process = match prepare_configured_process(request, runtime, shutdown).await {
        Ok(process) => process,
        Err(execution) => return execution,
    };
    Box::pin(execute_configured_session(process, shutdown)).await
}

async fn prepare_configured_process(
    request: ConfiguredTurnRequest,
    runtime: ConfiguredRuntime,
    shutdown: &Arc<CancelHandle>,
) -> Result<ConfiguredProcess, Execution> {
    let Ok(secret) = HealthSecret::generate() else {
        return Err(request.fail(EngineOperationError::EntropyFailed));
    };
    let Ok(mut child) = (match &request.input.launch {
        crate::engine_owner::InternalLaunch::Verified(verified) => spawn_configured_engine(
            verified.as_ref(),
            &request.input.project_root,
            secret.as_str(),
        ),
        #[cfg(test)]
        crate::engine_owner::InternalLaunch::Fixture(fixture) => {
            crate::engine_owner::process::spawn_configured_fixture_engine(
                &fixture.program,
                fixture.scenario,
                secret.as_str(),
            )
        }
        crate::engine_owner::InternalLaunch::Codex(_) => {
            return Err(request.fail(EngineOperationError::Configuration));
        }
        crate::engine_owner::InternalLaunch::Claude(_) => {
            return Err(request.fail(EngineOperationError::Configuration));
        }
        crate::engine_owner::InternalLaunch::Grok(_) => {
            return Err(request.fail(EngineOperationError::Configuration));
        }
        crate::engine_owner::InternalLaunch::Cursor(_) => {
            return Err(request.fail(EngineOperationError::Configuration));
        }
    }) else {
        return Err(request.fail(EngineOperationError::SpawnFailed));
    };
    let lifeline = LifelineWriter::take(&mut child);
    let maybe_stdout = child.stdout.take();
    let stderr_counter = StderrCounter::new(child.stderr.take(), runtime.bounds.stderr_cap_bytes);
    let Some(mut stdout) = maybe_stdout else {
        let parts = ChildParts {
            child,
            lifeline,
            stdout: None,
            stderr_counter,
        };
        let error = EngineOperationError::ReadinessFailed(ReadinessError::Io);
        return Err(finish_configured_start(request, parts, error, runtime.limits.close).await);
    };
    let mut parts = ChildParts {
        child,
        lifeline,
        stdout: None,
        stderr_counter,
    };
    let endpoint = match drive_readiness(
        &mut stdout,
        &mut parts,
        phase_deadline(runtime.limits.readiness, request.deadline),
        shutdown,
        &request.control,
        runtime.bounds.max_readiness_line,
    )
    .await
    {
        Ok(endpoint) => endpoint,
        Err(error) => {
            drop(stdout);
            let error = map_readiness_error(error);
            return Err(finish_configured_start(request, parts, error, runtime.limits.close).await);
        }
    };
    drop(stdout);
    if let Err(error) = super::super::http::perform_health(
        &endpoint,
        &secret,
        &runtime.bounds,
        phase_deadline(runtime.limits.health, request.deadline),
        &request.control,
        shutdown,
        Some(request.input.launch.version()),
    )
    .await
    {
        let error = map_health_error(error);
        return Err(finish_configured_start(request, parts, error, runtime.limits.close).await);
    }
    Ok(ConfiguredProcess {
        request,
        runtime,
        secret,
        parts,
        endpoint,
    })
}

async fn execute_configured_session(
    process: ConfiguredProcess,
    shutdown: &Arc<CancelHandle>,
) -> Execution {
    let state = match create_configured_session(process, shutdown).await {
        Ok(state) => state,
        Err(execution) => return execution,
    };
    Box::pin(authorize_configured_session(state, shutdown)).await
}

#[expect(
    clippy::too_many_lines,
    reason = "one linear configured-session bootstrap over the spawned process; extraction would thread the full process state"
)]
async fn create_configured_session(
    process: ConfiguredProcess,
    shutdown: &Arc<CancelHandle>,
) -> Result<PreparedConfiguredSession, Execution> {
    // The configured session below is OpenCode2-shaped end to end. Any other
    // selection fails closed instead of executing as OpenCode2.
    if !matches!(
        process.request.input.settings.config().selection(),
        artisan_domain::EngineSelection::OpenCode2(_)
    ) {
        let ConfiguredProcess { request, .. } = process;
        return Err(request.fail(EngineOperationError::Configuration));
    }
    let ConfiguredProcess {
        request,
        runtime,
        secret,
        parts,
        endpoint,
    } = process;
    let ConfiguredTurnRequest {
        input,
        deadline,
        control,
        prepared,
        authorize,
        observations,
        respond,
        // OpenCode2 carries no steer verb: admit never wires a channel here,
        // so every steer attempt resolves `Unsupported`. The binding passes
        // through the reconstructions below unchanged.
        steer_rx,
    } = request;
    let artisan_domain::EngineSelection::OpenCode2(selection) = input.settings.config().selection()
    else {
        // Unreachable after the guard above, but fails closed without
        // coercing another engine into an OpenCode2 session.
        return Err(finish_configured_start(
            ConfiguredTurnRequest {
                input,
                deadline,
                control,
                prepared,
                authorize,
                observations,
                respond,
                steer_rx,
            },
            parts,
            EngineOperationError::Configuration,
            runtime.limits.close,
        )
        .await);
    };
    let permission = selection.permission();
    let session_details = if let Some(continuation) = input.continuation.as_ref() {
        let resume_selection = ResumeSelection::new(
            continuation.provider_session_id(),
            input.project_root.as_str(),
            permission.agent_id().as_str(),
            selection.model_id().as_str(),
            selection.route_id().as_str(),
            selection
                .variant_id()
                .map(artisan_domain::EngineVariantId::as_str),
        );
        match perform_resume(ResumeInput {
            endpoint: &endpoint,
            secret: &secret,
            bounds: &runtime.bounds,
            deadline: phase_deadline(runtime.limits.prompt, deadline),
            cancel: &control,
            shutdown,
            selection: resume_selection,
        })
        .await
        {
            Ok(receipt) => (receipt.session_id().to_owned(), true, receipt.log_cursor()),
            Err(error) => {
                return Err(finish_configured_start(
                    ConfiguredTurnRequest {
                        input,
                        deadline,
                        control,
                        prepared,
                        authorize,
                        observations,
                        respond,
                        steer_rx,
                    },
                    parts,
                    map_resume_error(error),
                    runtime.limits.close,
                )
                .await);
            }
        }
    } else {
        let create_input = CreateSessionInput {
            directory: input.project_root.as_str(),
            profile_id: selection.profile_id().as_str(),
            model_id: selection.model_id().as_str(),
            route_id: selection.route_id().as_str(),
            variant_id: selection
                .variant_id()
                .map(artisan_domain::EngineVariantId::as_str),
            permission_id: permission.permission_id().as_str(),
            agent_id: permission.agent_id().as_str(),
            approval: permission.approval().as_str(),
            filesystem: permission.filesystem().as_str(),
            network: permission.network().as_str(),
            web_search: permission.web_search().as_str(),
        };
        match perform_create_session(
            &endpoint,
            &secret,
            &runtime.bounds,
            phase_deadline(runtime.limits.prompt, deadline),
            &control,
            shutdown,
            create_input,
        )
        .await
        {
            Ok(receipt) => (
                receipt.session().to_owned(),
                false,
                Some(input.stream_after),
            ),
            Err(error) => {
                return Err(finish_configured_start(
                    ConfiguredTurnRequest {
                        input,
                        deadline,
                        control,
                        prepared,
                        authorize,
                        observations,
                        respond,
                        steer_rx,
                    },
                    parts,
                    map_prompt_error(error),
                    runtime.limits.close,
                )
                .await);
            }
        }
    };
    Ok(PreparedConfiguredSession {
        input,
        deadline,
        control,
        prepared,
        authorize,
        observations,
        respond,
        parts,
        endpoint,
        secret,
        runtime,
        session: session_details.0,
        resume: session_details.1,
        stream_after: session_details.2,
    })
}

async fn authorize_configured_session(
    state: PreparedConfiguredSession,
    shutdown: &Arc<CancelHandle>,
) -> Execution {
    let PreparedConfiguredSession {
        input,
        deadline,
        control,
        prepared,
        authorize,
        observations,
        respond,
        parts,
        endpoint,
        secret,
        runtime,
        session,
        resume,
        stream_after,
    } = state;
    let initial_stream = match &input.launch {
        super::super::InternalLaunch::Verified(_) => {
            StreamState::for_run(input.run_id.clone(), session.clone(), stream_after)
        }
        super::super::InternalLaunch::Codex(_) => Err(StreamError::InvalidSession),
        super::super::InternalLaunch::Claude(_) => Err(StreamError::InvalidSession),
        super::super::InternalLaunch::Grok(_) => Err(StreamError::InvalidSession),
        super::super::InternalLaunch::Cursor(_) => Err(StreamError::InvalidSession),
        #[cfg(test)]
        super::super::InternalLaunch::Fixture(_) => Ok(StreamState::new(stream_after)),
    };
    let stream_state = match initial_stream {
        Ok(state) => state,
        Err(error) => {
            return finish_configured_start(
                ConfiguredTurnRequest {
                    input,
                    deadline,
                    control,
                    prepared,
                    authorize,
                    observations,
                    respond,
                    // PreparedConfiguredSession carries no steer binding:
                    // this OpenCode2 path never wires a channel, so
                    // `None` (typed `Unsupported` downstream), not a
                    // fabricated sender.
                    steer_rx: None,
                },
                parts,
                map_stream_error(error),
                runtime.limits.close,
            )
            .await;
        }
    };
    let session = ConfiguredSession {
        input,
        deadline,
        control,
        authorize,
        observations,
        respond,
        parts,
        endpoint,
        secret,
        runtime,
        session,
        resume,
        stream_after,
        stream_state,
    };
    if prepared
        .send(Ok(PreparedSession::new(session.session.clone())))
        .is_err()
    {
        return session
            .abort(shutdown, EngineOperationError::Cancelled)
            .await;
    }
    execute_authorized_configured_turn(session, shutdown).await
}

async fn execute_authorized_configured_turn(
    mut session: ConfiguredSession,
    shutdown: &Arc<CancelHandle>,
) -> Execution {
    if let Err(error) = wait_for_authorization(
        &mut session.parts,
        &mut session.authorize,
        session.deadline,
        shutdown,
        &session.control,
    )
    .await
    {
        return session.abort(shutdown, error).await;
    }
    let files: Vec<PromptFile> = session
        .input
        .prompt
        .attachments()
        .iter()
        .map(|attachment| {
            PromptFile::from_image(
                attachment.mime_type_str(),
                attachment.bytes(),
                attachment.name().to_owned(),
            )
        })
        .collect();
    if let Err(error) = perform_prompt(
        &session.endpoint,
        &session.secret,
        &session.runtime.bounds,
        phase_deadline(session.runtime.limits.prompt, session.deadline),
        &session.control,
        shutdown,
        PromptInput::new_with_optional_text(
            &session.session,
            &session.input.prompt_delivery,
            &files,
            &session.input.prompt_id,
            session.resume,
            session.input.prompt.text().map(artisan_domain::AuthoredText::as_str),
        ),
    )
    .await
    {
        return session.abort(shutdown, map_prompt_error(error)).await;
    }
    let stream_usage = stream_usage_context(&session.input);
    let stream_input = StreamInput::new((
        &session.endpoint,
        &session.secret,
        &session.runtime.bounds,
        phase_deadline(session.runtime.limits.sse, session.deadline),
        &session.control,
        shutdown,
        &session.session,
        session.input.stream_after,
        session.observations.clone(),
    ))
    .with_after(session.stream_after)
    .with_usage_context_option(stream_usage);
    let stream_result = follow_stream_for_run_with_state(
        stream_input,
        &session.input.run_id,
        &mut session.stream_state,
    )
    .await;
    match stream_result {
        Ok(receipt) => {
            let terminal = receipt.state();
            let ConfiguredSession {
                parts,
                runtime,
                observations,
                respond,
                ..
            } = session;
            drop(observations);
            finish_turn_result(
                parts,
                Ok(EngineTurnResult { terminal }),
                respond,
                runtime.limits.close,
            )
            .await
        }
        Err(error) => session.abort(shutdown, map_stream_error(error)).await,
    }
}

fn stream_usage_context(input: &super::super::InternalTurnInput) -> Option<StreamUsageContext> {
    let thread_id = input.thread_id.as_ref()?.clone();
    // Usage attribution is OpenCode2-shaped; other selections carry no
    // usage scope instead of attributing as OpenCode2. Claude usage travels
    // its own pump scope in `execute_claude_turn` (cumulative reports with a
    // replacing context gauge); the `/usage` CLI buckets stay diagnostics.
    let artisan_domain::EngineSelection::OpenCode2(selection) = input.settings.config().selection()
    else {
        return None;
    };
    Some(StreamUsageContext::new(
        input.run_id.clone(),
        thread_id,
        selection.model_id().clone(),
        selection.route_id().clone(),
        selection.variant_id().cloned(),
    ))
}

struct AbortAfterSession<'a> {
    parts: ChildParts,
    endpoint: &'a ValidatedEndpoint,
    secret: &'a HealthSecret,
    runtime: &'a ConfiguredRuntime,
    session: &'a str,
    run_id: &'a RunId,
    stream_after: Option<u64>,
    stream_state: StreamState,
    usage_context: Option<StreamUsageContext>,
    observations: mpsc::Sender<EngineObservation>,
    respond: oneshot::Sender<TurnResult>,
    shutdown: &'a Arc<CancelHandle>,
    cause: EngineOperationError,
    attempt_deadline: Instant,
}

async fn abort_after_session(input: AbortAfterSession<'_>) -> Execution {
    let AbortAfterSession {
        parts,
        endpoint,
        secret,
        runtime,
        session,
        run_id,
        stream_after,
        observations,
        mut stream_state,
        usage_context,
        respond,
        shutdown,
        cause,
        attempt_deadline,
    } = input;
    let interrupt_cancel = CancelHandle::new();
    let interrupt_deadline = phase_deadline(runtime.limits.close, attempt_deadline);
    let _ = perform_interrupt(
        endpoint,
        secret,
        &runtime.bounds,
        interrupt_deadline,
        &interrupt_cancel,
        shutdown,
        session,
    )
    .await;

    let stream_cancel = CancelHandle::new();
    let stream_deadline = phase_deadline(runtime.limits.sse, attempt_deadline);
    let stream_input = StreamInput::new((
        endpoint,
        secret,
        &runtime.bounds,
        stream_deadline,
        &stream_cancel,
        shutdown,
        session,
        stream_after.unwrap_or(0),
        observations,
    ))
    .with_after(stream_after)
    .with_usage_context_option(usage_context);
    let stream_result =
        follow_stream_for_run_with_state(stream_input, run_id, &mut stream_state).await;
    match stream_result {
        Ok(receipt) => {
            finish_turn_result(
                parts,
                Ok(EngineTurnResult {
                    terminal: receipt.state(),
                }),
                respond,
                runtime.limits.close,
            )
            .await
        }
        Err(_) => finish_turn_result(parts, Err(cause), respond, runtime.limits.close).await,
    }
}
