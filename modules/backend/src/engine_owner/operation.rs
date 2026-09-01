//! Private orchestration for the engine owner: the single owner task,
//! generation allocation, per-operation execution, and the quarantine tail.
//!
//! One task owns at most one active engine child at a time — its exact
//! [`tokio::process::Child`], the taken sole stdin lifeline writer, the
//! stderr counting state, the burned generation, and every cleanup decision.
//! Work arrives through a bounded channel and is processed strictly
//! sequentially; there is no per-job task, no parallel owner, and no
//! replacement child before an observed reap.
//!
//! Fixed precedence, re-checked at the top of every scheduling cycle:
//! owner shutdown or terminal state, abandonment or explicit cancellation,
//! the operation deadline, and only then completion sources. Once a cleanup
//! sequence starts it runs to completion regardless of those signals.
//!
//! P3 adds bounded child readiness parsing and a bounded authenticated
//! HTTP/1 health handshake after spawn. Readiness is exactly one
//! newline-terminated `{"url": "..."}` record capped via `cap + 1`; health
//! is one `GET /api/health` with `Basic base64(opencode:<secret>)` over a
//! Hyper `TokioIo<TcpStream>` connection configured with caller-supplied
//! `max_headers` and `max_buf_bytes` and body-bounded via `Limited`.

use std::sync::Arc;
use std::time::Duration;

use artisan_domain::RunId;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;

use artisan_transport::CancelHandle;

use super::http::{
    CreateSessionInput, HealthError, HealthSecret, PromptError, PromptFile, PromptInput,
    perform_create_session, perform_interrupt, perform_prompt,
};
use super::observation::{EngineObservation, TerminalState};
use super::process::{
    ChildParts, CleanupObservation, LaunchRecipe, LifelineWriter, RetainedEngine, StderrCounter,
    cleanup_after_abort, eventual_wait_once, spawn_configured_engine, spawn_engine,
};
use super::readiness::ReadinessError;
use super::readiness::ValidatedEndpoint;
use super::stream::{StreamError, StreamInput, follow_stream_for_run};
use super::{EngineBounds, EngineLimits};

/// Payload-free engine health observable by the facade.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HealthState {
    /// Admission is open and the owner task is serving work.
    Active,
    /// The owner irreversibly stopped serving new work.
    Quarantined,
}

/// Checked nonzero generation allocator.
///
/// Values start at 1, are handed out immediately before each spawn attempt,
/// and are burned whether or not the spawn succeeds. Allocation stops
/// permanently at [`u64::MAX`]; values never wrap, reset, or repeat within
/// the process. This is engine-owner incarnation numbering, NOT the durable
/// E1 run generation; it scopes only the live child identity for this owner
/// instance and has no effect on persisted run lifecycle.
pub(crate) struct GenerationAllocator {
    next: u64,
    exhausted: bool,
}

impl GenerationAllocator {
    /// Creates the allocator beginning at generation 1.
    pub(crate) const fn new() -> Self {
        Self {
            next: 1,
            exhausted: false,
        }
    }

    /// Mints and burns the next generation, if any remain.
    ///
    /// Returning `None` means the checked space is exhausted; the caller
    /// must refuse to launch any further child rather than wrap or reset.
    pub(crate) fn mint(&mut self) -> Option<u64> {
        if self.exhausted {
            return None;
        }
        let generation = self.next;
        match self.next.checked_add(1) {
            Some(next) => self.next = next,
            None => self.exhausted = true,
        }
        Some(generation)
    }

    /// Forces the internal counter for private end-of-space unit tests.
    ///
    /// This exists only under `cfg(test)`; there is deliberately no public
    /// setter for allocator state.
    #[cfg(test)]
    pub(crate) fn force_next(&mut self, value: u64) {
        self.next = value;
        self.exhausted = false;
    }
}

/// Why an admission was refused without queueing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LaunchAdmissionError {
    /// The owner is shut down, quarantined, or its channel is closed.
    Unavailable,
    /// The bounded queue is full.
    Busy,
    /// The caller-supplied budget cannot form a representable deadline.
    InvalidDeadline,
    /// The persisted observation capacity cannot form a bounded channel.
    InvalidCapacity,
}

/// Typed, payload-free failure of one engine operation.
///
/// Variants carry no path, payload, or operating-system strings; raw I/O
/// stays private to the owner task.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum EngineOperationError {
    /// The owner shut down before the operation settled.
    Shutdown,
    /// The operation was cancelled or abandoned by its caller.
    Cancelled,
    /// The caller-supplied budget elapsed, queue waiting included.
    Deadline,
    /// The checked generation space is exhausted.
    GenerationExhausted,
    /// The child could not be spawned.
    SpawnFailed,
    /// Operating-system entropy for the 32-byte secret failed.
    EntropyFailed,
    /// Bounded readiness parsing failed.
    ReadinessFailed(ReadinessError),
    /// Bounded health handshake failed.
    HealthFailed(HealthError),
    /// Health version was incompatible with the expected value.
    IncompatibleVersion,
    /// Persisted turn settings could not be translated into owner limits.
    Configuration,
    /// The provider session could not be created or the prompt could not be
    /// delivered. The exact provider payload remains private to the owner.
    ProviderRequestFailed,
    /// The authenticated observation stream did not settle normally.
    StreamFailed,
    /// Cleanup could not observe the child's death and no primary cause
    /// existed to preserve alongside it.
    ReapUnresolved,
    /// A primary failure whose separately observed cleanup could not confirm
    /// the child's reap within the close budget.
    UnresolvedReapDuring {
        /// The original typed cause.
        primary: Box<EngineOperationError>,
    },
}

/// Observed cleanup mode for a successful configured-engine preflight.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PreflightReap {
    /// The child exited after stdin was closed without a termination request.
    WithoutKill,
    /// The child exited only after the bounded cleanup requested termination.
    AfterKill,
}

/// Small, payload-safe receipt for a completed configured-engine preflight.
///
/// The stable profile and version are available to the caller, while the
/// `Debug` representation redacts their bytes. No endpoint, executable,
/// process identity, secret, headers, or provider payload is retained.
pub(crate) struct PreflightReceipt {
    profile_id: String,
    version: String,
    reap: PreflightReap,
}

impl PreflightReceipt {
    fn new(profile_id: String, version: String, reap: PreflightReap) -> Self {
        Self {
            profile_id,
            version,
            reap,
        }
    }

    pub(crate) fn profile_id(&self) -> &str {
        &self.profile_id
    }

    pub(crate) fn version(&self) -> &str {
        &self.version
    }

    pub(crate) const fn reap(&self) -> PreflightReap {
        self.reap
    }
}

impl std::fmt::Debug for PreflightReceipt {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PreflightReceipt")
            .field("profile_id", &"<redacted>")
            .field("version", &"<redacted>")
            .field("reap", &self.reap)
            .finish()
    }
}

pub(crate) type PreflightResult = Result<PreflightReceipt, EngineOperationError>;

/// Honest spawn and exit observation of one launch.
///
/// Reports only actually observed spawn and exit facts — observed generation
/// and whether the child exited. It does not claim readiness, binding,
/// provider state, or completion semantics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LaunchOutcome {
    /// The child was spawned and its exit was observed with this generation.
    ObservedExit {
        /// The burned generation for this launch.
        generation: u64,
        /// Whether the observed exit status was success.
        success: bool,
    },
}

/// Result carried by one launch response channel.
pub(crate) type LaunchResult = Result<LaunchOutcome, EngineOperationError>;

/// One admitted launch travelling from the facade to the owner task.
pub(crate) enum Job {
    /// Existing readiness/health launch used by the isolated owner tests.
    Legacy {
        run_id: RunId,
        deadline: Instant,
        control: Arc<CancelHandle>,
        respond: oneshot::Sender<LaunchResult>,
    },
    /// Configured spawn/readiness/health/observed-reap preflight that stops
    /// before provider session creation.
    Preflight {
        input: Box<super::InternalPreflightInput>,
        control: Arc<CancelHandle>,
        respond: oneshot::Sender<PreflightResult>,
    },
    /// A fully immutable configured turn handed to the owner after durable
    /// launch. Carries the single internal input so production and `#[cfg(test)]`
    /// fixture admissions share exactly one queued type and one executor.
    Turn {
        input: Box<super::InternalTurnInput>,
        deadline: Instant,
        control: Arc<CancelHandle>,
        prepared: oneshot::Sender<Result<PreparedSession, EngineOperationError>>,
        authorize: oneshot::Receiver<()>,
        observations: mpsc::Sender<EngineObservation>,
        respond: oneshot::Sender<TurnResult>,
    },
}

/// Single-owner future for one admitted launch.
///
/// Deliberately not `Clone`. Dropping the future cancels its private signal
/// before admission ordering guarantees the owner learns of abandonment.
pub(crate) struct AcceptedLaunch {
    receiver: oneshot::Receiver<LaunchResult>,
    control: Arc<CancelHandle>,
}

impl AcceptedLaunch {
    /// Creates an accepted launch from its parts (facade-only).
    pub(crate) fn from_parts(
        receiver: oneshot::Receiver<LaunchResult>,
        control: Arc<CancelHandle>,
    ) -> Self {
        Self { receiver, control }
    }

    /// Cancels this launch explicitly.
    pub fn cancel(&self) {
        self.control.cancel();
    }
}

impl std::future::Future for AcceptedLaunch {
    type Output = LaunchResult;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        use std::pin::Pin;
        use std::task::Poll;
        match Pin::new(&mut self.receiver).poll(context) {
            Poll::Ready(Ok(result)) => Poll::Ready(result),
            Poll::Ready(Err(_)) => Poll::Ready(Err(EngineOperationError::ReapUnresolved)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for AcceptedLaunch {
    fn drop(&mut self) {
        self.control.cancel();
    }
}

/// Single-owner future for one configured-engine preflight.
pub(crate) struct AcceptedPreflight {
    receiver: oneshot::Receiver<PreflightResult>,
    control: Arc<CancelHandle>,
}

impl AcceptedPreflight {
    /// Creates an accepted preflight from its owner-only response parts.
    pub(crate) fn from_parts(
        receiver: oneshot::Receiver<PreflightResult>,
        control: Arc<CancelHandle>,
    ) -> Self {
        Self { receiver, control }
    }

    /// Cancels this preflight explicitly.
    pub(crate) fn cancel(&self) {
        self.control.cancel();
    }
}

impl std::future::Future for AcceptedPreflight {
    type Output = PreflightResult;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        use std::pin::Pin;
        use std::task::Poll;
        match Pin::new(&mut self.receiver).poll(context) {
            Poll::Ready(Ok(result)) => Poll::Ready(result),
            Poll::Ready(Err(_)) => Poll::Ready(Err(EngineOperationError::ReapUnresolved)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for AcceptedPreflight {
    fn drop(&mut self) {
        self.control.cancel();
    }
}

/// Safe session metadata released only after `OpenCode2` `CreateSession`.
pub(crate) struct PreparedSession {
    session: String,
}

impl PreparedSession {
    fn new(session: String) -> Self {
        Self { session }
    }

    pub(crate) fn session(&self) -> &str {
        &self.session
    }
}

impl std::fmt::Debug for PreparedSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("PreparedSession { <redacted> }")
    }
}

/// Result of one configured provider turn after the owner has cleaned up its
/// child and transport drivers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct EngineTurnResult {
    terminal: TerminalState,
}

impl EngineTurnResult {
    pub(crate) const fn terminal(self) -> TerminalState {
        self.terminal
    }
}

pub(crate) type TurnResult = Result<EngineTurnResult, EngineOperationError>;

/// Single-owner handoff for the configured turn phases.
pub(crate) struct AcceptedTurn {
    prepared: oneshot::Receiver<Result<PreparedSession, EngineOperationError>>,
    authorize_sender: Option<oneshot::Sender<()>>,
    observations: mpsc::Receiver<EngineObservation>,
    receiver: Option<oneshot::Receiver<TurnResult>>,
    control: Arc<CancelHandle>,
}

impl AcceptedTurn {
    pub(crate) fn from_parts(
        prepared: oneshot::Receiver<Result<PreparedSession, EngineOperationError>>,
        authorize_sender: oneshot::Sender<()>,
        observations: mpsc::Receiver<EngineObservation>,
        receiver: oneshot::Receiver<TurnResult>,
        control: Arc<CancelHandle>,
    ) -> Self {
        Self {
            prepared,
            authorize_sender: Some(authorize_sender),
            observations,
            receiver: Some(receiver),
            control,
        }
    }

    pub(crate) async fn prepare(&mut self) -> TurnResultPrepared {
        match (&mut self.prepared).await {
            Ok(result) => result,
            Err(_) => Err(EngineOperationError::ReapUnresolved),
        }
    }

    pub(crate) fn authorize(&mut self) -> Result<(), EngineOperationError> {
        self.authorize_sender
            .take()
            .ok_or(EngineOperationError::ProviderRequestFailed)?
            .send(())
            .map_err(|()| EngineOperationError::ProviderRequestFailed)
    }

    pub(crate) async fn next_observation(&mut self) -> Option<EngineObservation> {
        self.observations.recv().await
    }

    pub(crate) async fn finish(mut self) -> TurnResult {
        let Some(receiver) = self.receiver.take() else {
            return Err(EngineOperationError::ReapUnresolved);
        };
        match receiver.await {
            Ok(result) => result,
            Err(_) => Err(EngineOperationError::ReapUnresolved),
        }
    }

    pub(crate) fn cancel(&self) {
        self.control.cancel();
    }
}

impl Drop for AcceptedTurn {
    fn drop(&mut self) {
        self.control.cancel();
    }
}

type TurnResultPrepared = Result<PreparedSession, EngineOperationError>;

/// How one executed job ended for the owner loop.
pub(crate) enum Execution {
    /// The response was settled and no custody remains.
    Completed,
    /// Cleanup could not observe a reap; exact custody is retained for
    /// quarantine.
    Quarantined(Box<RetainedEngine>),
}

/// Runs the owner task until shutdown, channel closure, or quarantine.
pub(crate) async fn run_owner(
    jobs: mpsc::Receiver<Job>,
    shutdown: Arc<CancelHandle>,
    health: watch::Sender<HealthState>,
    recipe: LaunchRecipe,
    limits: EngineLimits,
    bounds: EngineBounds,
) {
    Box::pin(run_owner_with_allocator(
        jobs,
        shutdown,
        health,
        recipe,
        limits,
        bounds,
        GenerationAllocator::new(),
    ))
    .await;
}

/// Variant that starts from a caller-supplied allocator (test-seeded).
pub(crate) async fn run_owner_with_allocator(
    jobs: mpsc::Receiver<Job>,
    shutdown: Arc<CancelHandle>,
    health: watch::Sender<HealthState>,
    recipe: LaunchRecipe,
    limits: EngineLimits,
    bounds: EngineBounds,
    mut generations: GenerationAllocator,
) {
    Box::pin(run_owner_loop(
        jobs,
        shutdown,
        health,
        Some(LegacyOwnerConfig {
            recipe,
            limits,
            bounds,
        }),
        &mut generations,
    ))
    .await;
}

/// Runs the configured owner lane.  Unlike the legacy test lane it has no
/// executable, version, budget, or bound values of its own: each turn carries
/// the immutable persisted snapshot that must govern its attempt.
pub(crate) async fn run_configured_owner(
    jobs: mpsc::Receiver<Job>,
    shutdown: Arc<CancelHandle>,
    health: watch::Sender<HealthState>,
) {
    let mut generations = GenerationAllocator::new();
    Box::pin(run_owner_loop(
        jobs,
        shutdown,
        health,
        None,
        &mut generations,
    ))
    .await;
}

struct LegacyOwnerConfig {
    recipe: LaunchRecipe,
    limits: EngineLimits,
    bounds: EngineBounds,
}

async fn run_owner_loop(
    mut jobs: mpsc::Receiver<Job>,
    shutdown: Arc<CancelHandle>,
    health: watch::Sender<HealthState>,
    legacy: Option<LegacyOwnerConfig>,
    generations: &mut GenerationAllocator,
) {
    loop {
        tokio::select! {
            biased;

            () = shutdown.wait() => break,

            job = jobs.recv() => {
                let Some(job) = job else { break };
                if shutdown.is_cancelled() {
                    reject_job(job, EngineOperationError::Shutdown);
                    continue;
                }
                if job_control(&job).is_cancelled() {
                    reject_job(job, EngineOperationError::Cancelled);
                    continue;
                }
                if Instant::now() >= job_deadline(&job) {
                    reject_job(job, EngineOperationError::Deadline);
                    continue;
                }
                let Some(generation) = generations.mint() else {
                    let _ = health.send(HealthState::Quarantined);
                    reject_job(job, EngineOperationError::GenerationExhausted);
                    quarantine_tail(&mut jobs, None).await;
                    return;
                };

                let execution = match job {
                    job @ Job::Legacy { .. } => {
                        let Some(config) = legacy.as_ref() else {
                            reject_job(job, EngineOperationError::Configuration);
                            continue;
                        };
                        execute_legacy_job(
                            &config.recipe,
                            generation,
                            job,
                            &shutdown,
                            config.limits,
                            config.bounds,
                        )
                        .await
                    }
                    job @ Job::Preflight { .. } => {
                        Box::pin(execute_preflight_job(job, &shutdown)).await
                    }
                    job @ Job::Turn { .. } => {
                        Box::pin(execute_configured_job(job, &shutdown)).await
                    }
                };
                match execution {
                    Execution::Completed => {}
                    Execution::Quarantined(retained) => {
                        let _ = health.send(HealthState::Quarantined);
                        quarantine_tail(&mut jobs, Some(retained)).await;
                        return;
                    }
                }
            }
        }
    }

    while let Ok(job) = jobs.try_recv() {
        reject_job(job, EngineOperationError::Shutdown);
    }
}

fn job_control(job: &Job) -> &Arc<CancelHandle> {
    match job {
        Job::Legacy { control, .. }
        | Job::Preflight { control, .. }
        | Job::Turn { control, .. } => control,
    }
}

fn job_deadline(job: &Job) -> Instant {
    match job {
        Job::Legacy { deadline, .. } | Job::Turn { deadline, .. } => *deadline,
        Job::Preflight { input, .. } => input.deadlines.admission,
    }
}

fn reject_job(job: Job, error: EngineOperationError) {
    match job {
        Job::Legacy { respond, .. } => {
            let _ = respond.send(Err(error));
        }
        Job::Preflight { respond, .. } => {
            let _ = respond.send(Err(error));
        }
        Job::Turn {
            prepared, respond, ..
        } => {
            let _ = prepared.send(Err(error.clone()));
            let _ = respond.send(Err(error));
        }
    }
}

/// Serves the quarantine tail: closes admission immediately, drains the
/// already-bounded queue, and then resolves custody of the retained engine,
/// if any, exactly once.
async fn quarantine_tail(jobs: &mut mpsc::Receiver<Job>, retained: Option<Box<RetainedEngine>>) {
    jobs.close();
    while let Some(job) = jobs.recv().await {
        reject_job(job, EngineOperationError::Shutdown);
    }

    if let Some(engine) = retained {
        match eventual_wait_once(engine).await {
            Ok(_status) => {}
            Err(retained) => {
                let _custody = retained;
                std::future::pending::<()>().await;
                unreachable!("pending never resolves");
            }
        }
    }
}

async fn drive_readiness(
    stdout: &mut tokio::process::ChildStdout,
    parts: &mut ChildParts,
    deadline: Instant,
    shutdown: &Arc<CancelHandle>,
    control: &Arc<CancelHandle>,
    max_line: usize,
) -> Result<super::readiness::ValidatedEndpoint, ReadinessError> {
    let readiness_fut =
        super::readiness::read_readiness(stdout, max_line, deadline, shutdown, control);
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
            event = parts.stderr_counter.pump(), if parts.stderr_counter.state() == super::process::StderrState::Open => {
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

struct HealthPhaseCtx<'a> {
    limits: EngineLimits,
    bounds: EngineBounds,
    deadline: Instant,
    control: &'a Arc<CancelHandle>,
    shutdown: &'a Arc<CancelHandle>,
}

async fn handle_health_phase(
    parts: ChildParts,
    generation: u64,
    endpoint: super::readiness::ValidatedEndpoint,
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
    let expected: Option<&str> = Some(super::http::FIXTURE_EXPECTED_VERSION);
    #[cfg(not(test))]
    let expected: Option<&str> = None;
    let health_result = super::http::perform_health(
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
    input: super::InternalPreflightInput,
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
    deadlines: super::PreflightDeadlines,
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
        super::InternalLaunch::Verified(verified) => {
            spawn_configured_engine(verified.as_ref(), &input.project_root, secret.as_str())
        }
        #[cfg(test)]
        super::InternalLaunch::Fixture(fixture) => super::process::spawn_configured_fixture_engine(
            &fixture.program,
            fixture.scenario,
            secret.as_str(),
        ),
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
async fn execute_preflight_job(job: Job, shutdown: &Arc<CancelHandle>) -> Execution {
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
    let health_version = match super::http::perform_health(
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

/// Executes one legacy readiness/health job end to end.  This path remains
/// available only for the existing owner tests; configured production turns
/// use the immutable snapshot path below.
async fn execute_legacy_job(
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

/// Executes one configured `OpenCode2` turn.  The profile capability and the
/// settings snapshot are moved into this owner call and are never reread from
/// durable state or ambient process configuration.
async fn execute_configured_job(job: Job, shutdown: &Arc<CancelHandle>) -> Execution {
    let Job::Turn {
        input,
        deadline,
        control,
        prepared,
        authorize,
        observations,
        respond,
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
    };
    let runtime = match configured_runtime(&request.input.settings, request.input.control_capacity)
    {
        Ok(runtime) => runtime,
        Err(error) => return request.fail(error),
    };
    let selection = request.input.settings.config().selection().as_opencode2();
    if request.input.launch.profile_id() != selection.profile_id().as_str() {
        return request.fail(EngineOperationError::Configuration);
    }
    if shutdown.is_cancelled() {
        return request.fail(EngineOperationError::Shutdown);
    }
    if request.control.is_cancelled() {
        return request.fail(EngineOperationError::Cancelled);
    }

    Box::pin(execute_configured_turn(request, runtime, shutdown)).await
}

struct ConfiguredTurnRequest {
    input: super::InternalTurnInput,
    deadline: Instant,
    control: Arc<CancelHandle>,
    prepared: oneshot::Sender<Result<PreparedSession, EngineOperationError>>,
    authorize: oneshot::Receiver<()>,
    observations: mpsc::Sender<EngineObservation>,
    respond: oneshot::Sender<TurnResult>,
}

impl ConfiguredTurnRequest {
    fn fail(self, error: EngineOperationError) -> Execution {
        let _ = self.prepared.send(Err(error.clone()));
        let _ = self.respond.send(Err(error));
        Execution::Completed
    }
}

struct ConfiguredProcess {
    request: ConfiguredTurnRequest,
    runtime: ConfiguredRuntime,
    secret: HealthSecret,
    parts: ChildParts,
    endpoint: ValidatedEndpoint,
}

struct PreparedConfiguredSession {
    input: super::InternalTurnInput,
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
}

struct ConfiguredSession {
    input: super::InternalTurnInput,
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
        } = self;
        abort_after_session(AbortAfterSession {
            parts,
            endpoint: &endpoint,
            secret: &secret,
            runtime: &runtime,
            session: &session,
            run_id: &input.run_id,
            stream_after: input.stream_after,
            observations,
            respond,
            shutdown,
            cause,
            attempt_deadline: deadline,
        })
        .await
    }
}

async fn execute_configured_turn(
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
    if let Err(error) = super::http::perform_health(
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

async fn finish_configured_start(
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

async fn execute_configured_session(
    process: ConfiguredProcess,
    shutdown: &Arc<CancelHandle>,
) -> Execution {
    let state = match create_configured_session(process, shutdown).await {
        Ok(state) => state,
        Err(execution) => return execution,
    };
    authorize_configured_session(state, shutdown).await
}

async fn create_configured_session(
    process: ConfiguredProcess,
    shutdown: &Arc<CancelHandle>,
) -> Result<PreparedConfiguredSession, Execution> {
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
    } = request;
    let selection = input.settings.config().selection().as_opencode2();
    let permission = selection.permission();
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
    let session_receipt = match perform_create_session(
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
        Ok(receipt) => receipt,
        Err(error) => {
            let error = map_prompt_error(error);
            return Err(finish_configured_start(
                ConfiguredTurnRequest {
                    input,
                    deadline,
                    control,
                    prepared,
                    authorize,
                    observations,
                    respond,
                },
                parts,
                error,
                runtime.limits.close,
            )
            .await);
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
        session: session_receipt.session().to_owned(),
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
    } = state;
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
    let files: [PromptFile; 0] = [];
    if let Err(error) = perform_prompt(
        &session.endpoint,
        &session.secret,
        &session.runtime.bounds,
        phase_deadline(session.runtime.limits.prompt, session.deadline),
        &session.control,
        shutdown,
        PromptInput::new(
            &session.session,
            &session.input.prompt_delivery,
            &files,
            &session.input.prompt_id,
            false,
            session.input.prompt_text.as_str(),
        ),
    )
    .await
    {
        return session.abort(shutdown, map_prompt_error(error)).await;
    }
    let stream_result = follow_stream_for_run(
        StreamInput::new((
            &session.endpoint,
            &session.secret,
            &session.runtime.bounds,
            phase_deadline(session.runtime.limits.sse, session.deadline),
            &session.control,
            shutdown,
            &session.session,
            session.input.stream_after,
            session.observations.clone(),
        )),
        &session.input.run_id,
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

struct ConfiguredRuntime {
    limits: EngineLimits,
    bounds: EngineBounds,
}

fn configured_runtime(
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

fn checked_usize(value: u64) -> Result<usize, EngineOperationError> {
    usize::try_from(value).map_err(|_| EngineOperationError::Configuration)
}

fn phase_deadline(budget: Duration, attempt_deadline: Instant) -> Instant {
    Instant::now()
        .checked_add(budget)
        .map_or(attempt_deadline, |candidate| {
            candidate.min(attempt_deadline)
        })
}

async fn wait_for_authorization(
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
            event = parts.stderr_counter.pump(), if parts.stderr_counter.state() == super::process::StderrState::Open => {
                let _ = event;
            }
        }
    }
}

struct AbortAfterSession<'a> {
    parts: ChildParts,
    endpoint: &'a ValidatedEndpoint,
    secret: &'a HealthSecret,
    runtime: &'a ConfiguredRuntime,
    session: &'a str,
    run_id: &'a RunId,
    stream_after: u64,
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
    let stream_result = follow_stream_for_run(
        StreamInput::new((
            endpoint,
            secret,
            &runtime.bounds,
            stream_deadline,
            &stream_cancel,
            shutdown,
            session,
            stream_after,
            observations,
        )),
        run_id,
    )
    .await;
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

async fn finish_turn_result(
    parts: ChildParts,
    result: TurnResult,
    respond: oneshot::Sender<TurnResult>,
    close_budget: Duration,
) -> Execution {
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
        super::process::note_observed_reap_for_tests(status);
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
            super::process::note_observed_reap_for_tests(status);
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

fn map_prompt_error(error: PromptError) -> EngineOperationError {
    match error {
        PromptError::Shutdown => EngineOperationError::Shutdown,
        PromptError::Cancelled => EngineOperationError::Cancelled,
        PromptError::Timeout => EngineOperationError::Deadline,
        _ => EngineOperationError::ProviderRequestFailed,
    }
}

fn map_stream_error(error: StreamError) -> EngineOperationError {
    match error {
        StreamError::Shutdown => EngineOperationError::Shutdown,
        StreamError::Cancelled => EngineOperationError::Cancelled,
        StreamError::Timeout => EngineOperationError::Deadline,
        _ => EngineOperationError::StreamFailed,
    }
}

fn map_readiness_error(error: ReadinessError) -> EngineOperationError {
    match error {
        ReadinessError::Deadline => EngineOperationError::Deadline,
        ReadinessError::Cancelled => EngineOperationError::Cancelled,
        ReadinessError::Shutdown => EngineOperationError::Shutdown,
        other => EngineOperationError::ReadinessFailed(other),
    }
}

fn map_health_error(error: HealthError) -> EngineOperationError {
    match error {
        HealthError::Timeout => EngineOperationError::Deadline,
        HealthError::Cancelled => EngineOperationError::Cancelled,
        HealthError::Shutdown => EngineOperationError::Shutdown,
        HealthError::IncompatibleVersion => EngineOperationError::IncompatibleVersion,
        other => EngineOperationError::HealthFailed(other),
    }
}

/// Runs the fixed cleanup for an aborted or faulted launch and settles the
/// response honestly.
async fn finish_aborted(
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
async fn finish_success(
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
        super::process::note_observed_reap_for_tests(status);
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
                super::process::note_observed_reap_for_tests(status);
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
