//! Legacy readiness/health bootstrap and catalog lanes: bounded
//! readiness parsing, the authenticated health handshake, preflight
//! admission, and one location-scoped catalog attempt.

use std::sync::Arc;
use std::time::Duration;

use artisan_transport::CancelHandle;
use tokio::sync::oneshot;
use tokio::time::Instant;

use super::super::EngineBounds;
use super::super::EngineLimits;
use super::super::catalog::CatalogResult;
use super::super::http::HealthError;
use super::super::http::HealthSecret;
use super::super::process::ChildParts;
use super::super::process::CleanupObservation;
use super::super::process::LifelineWriter;
use super::super::process::StderrCounter;
use super::super::process::cleanup_after_abort;
use super::super::process::spawn_configured_engine;
use super::super::readiness::ReadinessError;
use super::core::CatalogOperationResult;
use super::core::EngineOperationError;
use super::core::Execution;
use super::core::Job;
use super::core::LaunchResult;
use super::core::PreflightReap;
use super::core::PreflightReceipt;
use super::core::PreflightResult;
use super::failures::map_catalog_error;
use super::failures::map_health_error;
use super::failures::map_readiness_error;
use super::lifecycle::finish_aborted;
use super::lifecycle::finish_success;

pub(super) async fn drive_readiness(
    stdout: &mut tokio::process::ChildStdout,
    parts: &mut ChildParts,
    deadline: Instant,
    shutdown: &Arc<CancelHandle>,
    control: &Arc<CancelHandle>,
    max_line: usize,
) -> Result<super::super::readiness::ValidatedEndpoint, ReadinessError> {
    let readiness_fut =
        super::super::readiness::read_readiness(stdout, max_line, deadline, shutdown, control);
    tokio::pin!(readiness_fut);
    loop {
        if shutdown.is_cancelled() {
            break Err(ReadinessError::Shutdown);
        }
        if control.is_cancelled() {
            break Err(ReadinessError::Cancelled);
        }
        if Instant::now() >= deadline {
            break Err(ReadinessError::Deadline);
        }
        tokio::select! {
            biased;
            () = shutdown.wait() => break Err(ReadinessError::Shutdown),
            () = control.wait() => break Err(ReadinessError::Cancelled),
            () = tokio::time::sleep_until(deadline) => break Err(ReadinessError::Deadline),
            event = parts.stderr_counter.pump(), if parts.stderr_counter.state() == super::super::process::StderrState::Open => {
                let _ = event;
            }
            waited = parts.child.wait() => {
                match waited {
                    Ok(_) => break Err(ReadinessError::EofBeforeNewline),
                    Err(_) => break Err(ReadinessError::Io),
                }
            }
            res = &mut readiness_fut => break res,
        }
    }
}

pub(super) struct HealthPhaseCtx<'a> {
    pub(super) limits: EngineLimits,
    pub(super) bounds: EngineBounds,
    pub(super) deadline: Instant,
    pub(super) control: &'a Arc<CancelHandle>,
    pub(super) shutdown: &'a Arc<CancelHandle>,
}

pub(super) async fn handle_health_phase(
    parts: ChildParts,
    generation: u64,
    endpoint: super::super::readiness::ValidatedEndpoint,
    secret: HealthSecret,
    respond: oneshot::Sender<LaunchResult>,
    ctx: HealthPhaseCtx<'_>,
) -> Execution {
    let health_deadline = std::cmp::min(
        Instant::now()
            .checked_add(ctx.limits.health)
            .unwrap_or(ctx.deadline),
        ctx.deadline,
    );
    #[cfg(test)]
    let expected: Option<&str> = Some(super::super::http::FIXTURE_EXPECTED_VERSION);
    #[cfg(not(test))]
    let expected: Option<&str> = None;
    let health_result = super::super::http::perform_health(
        &endpoint,
        &secret,
        &ctx.bounds,
        health_deadline,
        ctx.control,
        ctx.shutdown,
        expected,
    )
    .await;
    match health_result {
        Ok(_version) => finish_success(parts, generation, respond, ctx.limits.close).await,
        Err(health_err) => {
            let mapped = map_health_error(health_err);
            finish_aborted(parts, mapped, respond, ctx.limits.close).await
        }
    }
}

struct PreflightRequest {
    input: super::super::InternalPreflightInput,
    control: Arc<CancelHandle>,
    respond: oneshot::Sender<PreflightResult>,
}

impl PreflightRequest {
    fn fail(self, error: EngineOperationError) -> Execution {
        let _ = self.respond.send(Err(error));
        Execution::Completed
    }
}

struct PreflightContext {
    profile_id: String,
    expected_version: String,
    bounds: EngineBounds,
    deadlines: super::super::PreflightDeadlines,
    control: Arc<CancelHandle>,
    respond: oneshot::Sender<PreflightResult>,
    secret: HealthSecret,
    parts: ChildParts,
    stdout: Option<tokio::process::ChildStdout>,
}

fn preflight_admission_error(
    request: &PreflightRequest,
    shutdown: &Arc<CancelHandle>,
) -> Option<EngineOperationError> {
    if shutdown.is_cancelled() {
        return Some(EngineOperationError::Shutdown);
    }
    if request.control.is_cancelled() {
        return Some(EngineOperationError::Cancelled);
    }
    if Instant::now() >= request.input.deadlines.admission {
        return Some(EngineOperationError::Deadline);
    }
    if !preflight_bounds_are_valid(&request.input.bounds) {
        return Some(EngineOperationError::Configuration);
    }
    None
}

fn prepare_preflight_context(request: PreflightRequest) -> Result<PreflightContext, Execution> {
    let PreflightRequest {
        input,
        control,
        respond,
    } = request;
    let profile_id = input.launch.profile_id().to_owned();
    let expected_version = input.launch.version().to_owned();
    let bounds = input.bounds;
    let deadlines = input.deadlines;
    let secret = match HealthSecret::generate() {
        Ok(secret) => secret,
        Err(HealthError::EntropyFailed) => {
            let _ = respond.send(Err(EngineOperationError::EntropyFailed));
            return Err(Execution::Completed);
        }
        Err(_) => unreachable!("health secret generation has one failure mode"),
    };
    let child_result = match &input.launch {
        super::super::InternalLaunch::Verified(verified) => {
            spawn_configured_engine(verified.as_ref(), &input.project_root, secret.as_str())
        }
        #[cfg(test)]
        super::super::InternalLaunch::Fixture(fixture) => {
            super::super::process::spawn_configured_fixture_engine(
                &fixture.program,
                fixture.scenario,
                secret.as_str(),
            )
        }
        super::super::InternalLaunch::Codex(_) => {
            let _ = respond.send(Err(EngineOperationError::Configuration));
            return Err(Execution::Completed);
        }
        super::super::InternalLaunch::Claude(_) => {
            let _ = respond.send(Err(EngineOperationError::Configuration));
            return Err(Execution::Completed);
        }
        super::super::InternalLaunch::Grok(_) => {
            let _ = respond.send(Err(EngineOperationError::Configuration));
            return Err(Execution::Completed);
        }
        super::super::InternalLaunch::Cursor(_) => {
            let _ = respond.send(Err(EngineOperationError::Configuration));
            return Err(Execution::Completed);
        }
    };
    let Ok(mut child) = child_result else {
        let _ = respond.send(Err(EngineOperationError::SpawnFailed));
        return Err(Execution::Completed);
    };
    let lifeline = LifelineWriter::take(&mut child);
    let stdout = child.stdout.take();
    let stderr_counter = StderrCounter::new(child.stderr.take(), bounds.stderr_cap_bytes);
    Ok(PreflightContext {
        profile_id,
        expected_version,
        bounds,
        deadlines,
        control,
        respond,
        secret,
        parts: ChildParts {
            child,
            lifeline,
            stdout: None,
            stderr_counter,
        },
        stdout,
    })
}

/// Executes the bounded configured-engine preflight. This branch deliberately
/// stops after authenticated version health and observed child cleanup; the
/// configured-turn session, prompt, and stream executors are not reachable.
pub(super) async fn execute_preflight_job(job: Job, shutdown: &Arc<CancelHandle>) -> Execution {
    let Job::Preflight {
        input,
        control,
        respond,
    } = job
    else {
        unreachable!("preflight executor received a non-preflight job");
    };
    let request = PreflightRequest {
        input: *input,
        control,
        respond,
    };
    if let Some(error) = preflight_admission_error(&request, shutdown) {
        return request.fail(error);
    }
    let context = match prepare_preflight_context(request) {
        Ok(context) => context,
        Err(execution) => return execution,
    };
    execute_preflight_context(context, shutdown).await
}

async fn execute_preflight_context(
    context: PreflightContext,
    shutdown: &Arc<CancelHandle>,
) -> Execution {
    let PreflightContext {
        profile_id,
        expected_version,
        bounds,
        deadlines,
        control,
        respond,
        secret,
        mut parts,
        stdout,
    } = context;
    let Some(mut stdout) = stdout else {
        drop(secret);
        return finish_preflight_failure(
            parts,
            EngineOperationError::ReadinessFailed(ReadinessError::Io),
            respond,
            deadlines.close,
        )
        .await;
    };
    let endpoint = match drive_readiness(
        &mut stdout,
        &mut parts,
        deadlines.readiness.min(deadlines.admission),
        shutdown,
        &control,
        bounds.max_readiness_line,
    )
    .await
    {
        Ok(endpoint) => endpoint,
        Err(error) => {
            drop(stdout);
            drop(secret);
            return finish_preflight_failure(
                parts,
                map_readiness_error(error),
                respond,
                deadlines.close,
            )
            .await;
        }
    };
    drop(stdout);
    let health_version = match super::super::http::perform_health(
        &endpoint,
        &secret,
        &bounds,
        deadlines.health.min(deadlines.admission),
        &control,
        shutdown,
        Some(&expected_version),
    )
    .await
    {
        Ok(health_version) => health_version,
        Err(error) => {
            drop(secret);
            return finish_preflight_failure(
                parts,
                map_health_error(error),
                respond,
                deadlines.close,
            )
            .await;
        }
    };
    drop(secret);
    finish_preflight_success(parts, profile_id, health_version, respond, deadlines.close).await
}

fn preflight_bounds_are_valid(bounds: &EngineBounds) -> bool {
    bounds.max_json_body > 0
        && bounds.max_readiness_line > 0
        && bounds.max_headers > 0
        && bounds.max_buf_bytes >= 8192
        && bounds.stderr_cap_bytes > 0
}

fn remaining_until(deadline: Instant) -> Duration {
    deadline
        .checked_duration_since(Instant::now())
        .unwrap_or(Duration::ZERO)
}

async fn finish_preflight_failure(
    parts: ChildParts,
    cause: EngineOperationError,
    respond: oneshot::Sender<PreflightResult>,
    close_deadline: Instant,
) -> Execution {
    match cleanup_after_abort(parts, remaining_until(close_deadline)).await {
        CleanupObservation::ReapedWithoutKill(_) | CleanupObservation::ReapedAfterKill(_) => {
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

async fn finish_preflight_success(
    parts: ChildParts,
    profile_id: String,
    version: String,
    respond: oneshot::Sender<PreflightResult>,
    close_deadline: Instant,
) -> Execution {
    let reap = match cleanup_after_abort(parts, remaining_until(close_deadline)).await {
        CleanupObservation::ReapedWithoutKill(_) => PreflightReap::WithoutKill,
        CleanupObservation::ReapedAfterKill(_) => PreflightReap::AfterKill,
        CleanupObservation::Retained(engine) => {
            let _ = respond.send(Err(EngineOperationError::ReapUnresolved));
            return Execution::Quarantined(engine);
        }
    };
    let _ = respond.send(Ok(PreflightReceipt::new(profile_id, version, reap)));
    Execution::Completed
}

struct CatalogRequest {
    input: super::super::InternalCatalogInput,
    control: Arc<CancelHandle>,
    respond: oneshot::Sender<CatalogOperationResult>,
}

impl CatalogRequest {
    fn fail(self, error: EngineOperationError) -> Execution {
        let _ = self.respond.send(Err(error));
        Execution::Completed
    }
}

struct CatalogContext {
    scope: super::super::catalog::CatalogScope,
    expected_version: String,
    bounds: EngineBounds,
    deadlines: super::super::PreflightDeadlines,
    catalog_deadline: Instant,
    control: Arc<CancelHandle>,
    respond: oneshot::Sender<CatalogOperationResult>,
    secret: HealthSecret,
    parts: ChildParts,
    stdout: Option<tokio::process::ChildStdout>,
}

fn catalog_admission_error(
    request: &CatalogRequest,
    shutdown: &Arc<CancelHandle>,
) -> Option<EngineOperationError> {
    if shutdown.is_cancelled() {
        return Some(EngineOperationError::Shutdown);
    }
    if request.control.is_cancelled() {
        return Some(EngineOperationError::Cancelled);
    }
    if Instant::now()
        >= request
            .input
            .deadlines
            .admission
            .min(request.input.catalog_deadline)
    {
        return Some(EngineOperationError::Deadline);
    }
    if !preflight_bounds_are_valid(&request.input.bounds) {
        return Some(EngineOperationError::Configuration);
    }
    if !request.input.scope.matches_launch(
        request.input.launch.profile_id(),
        request.input.project_root.as_str(),
    ) {
        return Some(EngineOperationError::Configuration);
    }
    None
}

fn prepare_catalog_context(request: CatalogRequest) -> Result<CatalogContext, Execution> {
    let CatalogRequest {
        input,
        control,
        respond,
    } = request;
    let expected_version = input.launch.version().to_owned();
    let bounds = input.bounds;
    let deadlines = input.deadlines;
    let catalog_deadline = input.catalog_deadline;
    let scope = input.scope;
    let secret = match HealthSecret::generate() {
        Ok(secret) => secret,
        Err(HealthError::EntropyFailed) => {
            let _ = respond.send(Err(EngineOperationError::EntropyFailed));
            return Err(Execution::Completed);
        }
        Err(_) => unreachable!("health secret generation has one failure mode"),
    };
    let child_result = match &input.launch {
        super::super::InternalLaunch::Verified(verified) => {
            spawn_configured_engine(verified.as_ref(), &input.project_root, secret.as_str())
        }
        #[cfg(test)]
        super::super::InternalLaunch::Fixture(fixture) => {
            super::super::process::spawn_configured_fixture_engine(
                &fixture.program,
                fixture.scenario,
                secret.as_str(),
            )
        }
        super::super::InternalLaunch::Codex(_) => {
            let _ = respond.send(Err(EngineOperationError::Configuration));
            return Err(Execution::Completed);
        }
        super::super::InternalLaunch::Claude(_) => {
            let _ = respond.send(Err(EngineOperationError::Configuration));
            return Err(Execution::Completed);
        }
        super::super::InternalLaunch::Grok(_) => {
            let _ = respond.send(Err(EngineOperationError::Configuration));
            return Err(Execution::Completed);
        }
        super::super::InternalLaunch::Cursor(_) => {
            let _ = respond.send(Err(EngineOperationError::Configuration));
            return Err(Execution::Completed);
        }
    };
    let Ok(mut child) = child_result else {
        let _ = respond.send(Err(EngineOperationError::SpawnFailed));
        return Err(Execution::Completed);
    };
    let lifeline = LifelineWriter::take(&mut child);
    let stdout = child.stdout.take();
    let stderr_counter = StderrCounter::new(child.stderr.take(), bounds.stderr_cap_bytes);
    Ok(CatalogContext {
        scope,
        expected_version,
        bounds,
        deadlines,
        catalog_deadline,
        control,
        respond,
        secret,
        parts: ChildParts {
            child,
            lifeline,
            stdout: None,
            stderr_counter,
        },
        stdout,
    })
}

/// Executes certified spawn/readiness/health and one location-scoped model
/// catalog read. This branch deliberately has no session, assistant, prompt,
/// or stream path.
pub(super) async fn execute_catalog_job(job: Job, shutdown: &Arc<CancelHandle>) -> Execution {
    let Job::Catalog {
        input,
        control,
        respond,
    } = job
    else {
        unreachable!("catalog executor received a non-catalog job");
    };
    let request = CatalogRequest {
        input: *input,
        control,
        respond,
    };
    if let Some(error) = catalog_admission_error(&request, shutdown) {
        return request.fail(error);
    }
    let context = match prepare_catalog_context(request) {
        Ok(context) => context,
        Err(execution) => return execution,
    };
    execute_catalog_context(context, shutdown).await
}

async fn execute_catalog_context(
    context: CatalogContext,
    shutdown: &Arc<CancelHandle>,
) -> Execution {
    let CatalogContext {
        scope,
        expected_version,
        bounds,
        deadlines,
        catalog_deadline,
        control,
        respond,
        secret,
        mut parts,
        stdout,
    } = context;
    let phase_deadline =
        |deadline: Instant| deadline.min(deadlines.admission).min(catalog_deadline);
    let Some(mut stdout) = stdout else {
        drop(secret);
        return finish_catalog_failure(
            parts,
            EngineOperationError::ReadinessFailed(ReadinessError::Io),
            respond,
            deadlines.close,
        )
        .await;
    };
    let endpoint = match drive_readiness(
        &mut stdout,
        &mut parts,
        phase_deadline(deadlines.readiness),
        shutdown,
        &control,
        bounds.max_readiness_line,
    )
    .await
    {
        Ok(endpoint) => endpoint,
        Err(error) => {
            drop(stdout);
            drop(secret);
            return finish_catalog_failure(
                parts,
                map_readiness_error(error),
                respond,
                deadlines.close,
            )
            .await;
        }
    };
    drop(stdout);
    if let Err(error) = super::super::http::perform_health(
        &endpoint,
        &secret,
        &bounds,
        phase_deadline(deadlines.health),
        &control,
        shutdown,
        Some(&expected_version),
    )
    .await
    {
        drop(secret);
        return finish_catalog_failure(parts, map_health_error(error), respond, deadlines.close)
            .await;
    }
    let result = super::super::http::perform_catalog(
        &endpoint,
        &secret,
        &bounds,
        phase_deadline(catalog_deadline),
        &control,
        shutdown,
        &scope,
    )
    .await;
    drop(secret);
    match result {
        Ok(result) => finish_catalog_success(parts, result, respond, deadlines.close).await,
        Err(error) => {
            finish_catalog_failure(parts, map_catalog_error(error), respond, deadlines.close).await
        }
    }
}

async fn finish_catalog_failure(
    parts: ChildParts,
    cause: EngineOperationError,
    respond: oneshot::Sender<CatalogOperationResult>,
    close_deadline: Instant,
) -> Execution {
    match cleanup_after_abort(parts, remaining_until(close_deadline)).await {
        CleanupObservation::ReapedWithoutKill(_) | CleanupObservation::ReapedAfterKill(_) => {
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

async fn finish_catalog_success(
    parts: ChildParts,
    result: CatalogResult,
    respond: oneshot::Sender<CatalogOperationResult>,
    close_deadline: Instant,
) -> Execution {
    match cleanup_after_abort(parts, remaining_until(close_deadline)).await {
        CleanupObservation::ReapedWithoutKill(_) | CleanupObservation::ReapedAfterKill(_) => {
            let _ = respond.send(Ok(result));
            Execution::Completed
        }
        CleanupObservation::Retained(engine) => {
            let _ = respond.send(Err(EngineOperationError::ReapUnresolved));
            Execution::Quarantined(engine)
        }
    }
}
