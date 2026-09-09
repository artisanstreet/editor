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

use artisan_domain::{ObservationId, RunId};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;

use artisan_transport::CancelHandle;

use super::catalog::{CatalogError, CatalogResult};
use super::http::{
    CreateSessionInput, HealthError, HealthSecret, PromptError, PromptFile, PromptInput,
    ResumeError, ResumeInput, ResumeSelection, perform_create_session, perform_interrupt,
    perform_prompt, perform_resume,
};
use super::interaction::{
    InteractionDeliveryError, InteractionTarget, TurnInteractionLedger, TurnInteractionOutcome,
};
use super::observation::{
    EngineObservation, SubagentLifecycleRow, SubagentTranscriptRow, TerminalState,
};
use super::process::{
    ChildParts, CleanupObservation, LaunchRecipe, LifelineWriter, RetainedEngine, StderrCounter,
    cleanup_after_abort, eventual_wait_once, spawn_configured_engine, spawn_engine,
};
use super::readiness::ReadinessError;
use super::readiness::ValidatedEndpoint;
use super::stream::{
    StreamError, StreamInput, StreamState, StreamUsageContext, follow_stream_for_run_with_state,
};
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
    /// Runtime model catalog discovery failed after the owner authenticated
    /// the certified engine.
    CatalogFailed(CatalogError),
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
    /// Configured spawn/readiness/health/location-scoped model discovery that
    /// stops before provider session creation.
    Catalog {
        input: Box<super::InternalCatalogInput>,
        control: Arc<CancelHandle>,
        respond: oneshot::Sender<CatalogOperationResult>,
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

/// Single-owner future for one configured-engine runtime model catalog.
pub(crate) struct AcceptedCatalog {
    receiver: oneshot::Receiver<CatalogOperationResult>,
    control: Arc<CancelHandle>,
}

impl AcceptedCatalog {
    /// Creates an accepted catalog request from its owner-only response parts.
    pub(crate) fn from_parts(
        receiver: oneshot::Receiver<CatalogOperationResult>,
        control: Arc<CancelHandle>,
    ) -> Self {
        Self { receiver, control }
    }

    /// Cancels this catalog request explicitly.
    pub(crate) fn cancel(&self) {
        self.control.cancel();
    }
}

impl std::future::Future for AcceptedCatalog {
    type Output = CatalogOperationResult;

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

pub(crate) type CatalogOperationResult = Result<CatalogResult, EngineOperationError>;

impl Drop for AcceptedCatalog {
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
    interactions: TurnInteractionLedger,
}

impl AcceptedTurn {
    pub(crate) fn from_parts(
        run_id: RunId,
        prepared: oneshot::Receiver<Result<PreparedSession, EngineOperationError>>,
        authorize_sender: oneshot::Sender<()>,
        observations: mpsc::Receiver<EngineObservation>,
        receiver: oneshot::Receiver<TurnResult>,
        control: Arc<CancelHandle>,
    ) -> Self {
        Self {
            interactions: TurnInteractionLedger::new(run_id),
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

    /// Notes one pending provider target on this turn's delivery ledger.
    ///
    /// The owning dispatch loop seeds pending targets from durable state
    /// before delivering their responses. Re-noting never regresses a
    /// resolved target.
    pub(crate) fn note_interaction_requested(
        &mut self,
        target_id: &ObservationId,
        target: InteractionTarget,
    ) {
        self.interactions.note_requested(target_id, target);
    }

    /// Delivers one validated mid-turn response onto this turn.
    ///
    /// Applies idempotent command ids and per-target resolution tracking to
    /// the turn ledger with [`CommandTargetError`] on unknown or resolved
    /// ids. Delivery records the decision only and never disturbs control
    /// flow: answering never cancels, interrupts, or steers the turn.
    pub(crate) fn deliver_interaction_response(
        &mut self,
        command_id: &str,
        target_id: &ObservationId,
        target: InteractionTarget,
        intent: &str,
    ) -> Result<TurnInteractionOutcome, InteractionDeliveryError> {
        self.interactions
            .deliver(command_id, target_id, target, intent)
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
                    job @ Job::Catalog { .. } => {
                        Box::pin(execute_catalog_job(job, &shutdown)).await
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
        | Job::Catalog { control, .. }
        | Job::Turn { control, .. } => control,
    }
}

fn job_deadline(job: &Job) -> Instant {
    match job {
        Job::Legacy { deadline, .. } | Job::Turn { deadline, .. } => *deadline,
        Job::Preflight { input, .. } => input.deadlines.admission,
        Job::Catalog { input, .. } => input.deadlines.admission.min(input.catalog_deadline),
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
        Job::Catalog { respond, .. } => {
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
        super::InternalLaunch::Codex(_) => {
            let _ = respond.send(Err(EngineOperationError::Configuration));
            return Err(Execution::Completed);
        }
        super::InternalLaunch::Claude(_) => {
            let _ = respond.send(Err(EngineOperationError::Configuration));
            return Err(Execution::Completed);
        }
        super::InternalLaunch::Grok(_) => {
            let _ = respond.send(Err(EngineOperationError::Configuration));
            return Err(Execution::Completed);
        }
        super::InternalLaunch::Cursor(_) => {
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

struct CatalogRequest {
    input: super::InternalCatalogInput,
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
    scope: super::catalog::CatalogScope,
    expected_version: String,
    bounds: EngineBounds,
    deadlines: super::PreflightDeadlines,
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
        super::InternalLaunch::Verified(verified) => {
            spawn_configured_engine(verified.as_ref(), &input.project_root, secret.as_str())
        }
        #[cfg(test)]
        super::InternalLaunch::Fixture(fixture) => super::process::spawn_configured_fixture_engine(
            &fixture.program,
            fixture.scenario,
            secret.as_str(),
        ),
        super::InternalLaunch::Codex(_) => {
            let _ = respond.send(Err(EngineOperationError::Configuration));
            return Err(Execution::Completed);
        }
        super::InternalLaunch::Claude(_) => {
            let _ = respond.send(Err(EngineOperationError::Configuration));
            return Err(Execution::Completed);
        }
        super::InternalLaunch::Grok(_) => {
        super::InternalLaunch::Cursor(_) => {
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
async fn execute_catalog_job(job: Job, shutdown: &Arc<CancelHandle>) -> Execution {
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
    if let Err(error) = super::http::perform_health(
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
    let result = super::http::perform_catalog(
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
    let selected_profile = match request.input.settings.config().selection() {
        artisan_domain::EngineSelection::OpenCode2(selection) => selection.profile_id().as_str(),
        artisan_domain::EngineSelection::Codex(selection) => selection.profile_id().as_str(),
        artisan_domain::EngineSelection::Claude(selection) => selection.profile_id().as_str(),
        artisan_domain::EngineSelection::Grok(selection) => selection.profile_id().as_str(),
        artisan_domain::EngineSelection::Cursor(selection) => selection.profile_id().as_str(),
        _ => return request.fail(EngineOperationError::Configuration),
    };
    if request.input.launch.profile_id() != selected_profile {
        return request.fail(EngineOperationError::Configuration);
    }
    // Codex, Claude, Grok, and Cursor turns carry no provider continuation in this
    // packet; a mid-turn resume here fails closed instead of executing as
    // another engine.
    if request.input.continuation.is_some()
        && matches!(
            request.input.launch,
            super::InternalLaunch::Codex(_)
                | super::InternalLaunch::Claude(_)
                | super::InternalLaunch::Grok(_)
                | super::InternalLaunch::Cursor(_)
        )
    {
        return request.fail(EngineOperationError::Configuration);
    }
    if shutdown.is_cancelled() {
        return request.fail(EngineOperationError::Shutdown);
    }
    if request.control.is_cancelled() {
        return request.fail(EngineOperationError::Cancelled);
    }

    if matches!(request.input.launch, super::InternalLaunch::Codex(_)) {
        return Box::pin(execute_codex_turn(request, runtime, shutdown)).await;
    }
    if matches!(request.input.launch, super::InternalLaunch::Claude(_)) {
        return Box::pin(execute_claude_turn(request, runtime, shutdown)).await;
    }
    if matches!(request.input.launch, super::InternalLaunch::Grok(_)) {
        return Box::pin(execute_grok_turn(request, runtime, shutdown)).await;
    }
    if matches!(request.input.launch, super::InternalLaunch::Cursor(_)) {
        return Box::pin(execute_cursor_turn(request, runtime, shutdown)).await;
    }
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
    resume: bool,
    stream_after: Option<u64>,
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

/// Executes one finite Codex turn over `codex app-server --stdio`.
///
/// Single-owner match arm beside the OpenCode2 executor: no second task, no
/// second queue. Performs initialize, thread/start, the bind authorization
/// gate, then turn/start plus the streaming pump. Text deltas normalize onto
/// the shared S1a vocabulary; approval/question frames populate the pending
/// tracker with no control-flow side effect; child-thread frames never adopt
/// the root turn. External-kill EOF maps to `Interrupted`, explicit cancel to
/// `Cancelled`, and stall/failure to `Failed`.
async fn execute_codex_turn(
    request: ConfiguredTurnRequest,
    runtime: ConfiguredRuntime,
    shutdown: &Arc<CancelHandle>,
) -> Execution {
    use super::codex as codex_runtime;
    use super::process::spawn_codex_engine;

    let artisan_domain::EngineSelection::Codex(selection) =
        request.input.settings.config().selection()
    else {
        return request.fail(EngineOperationError::Configuration);
    };
    let settings = match codex_runtime::CodexSettings::from_selection(selection) {
        Ok(settings) => settings,
        Err(_) => return request.fail(EngineOperationError::Configuration),
    };
    if settings.profile_id() != request.input.launch.profile_id() {
        return request.fail(EngineOperationError::Configuration);
    }
    let super::InternalLaunch::Codex(launch) = &request.input.launch else {
        return request.fail(EngineOperationError::Configuration);
    };
    let mut child = match spawn_codex_engine(launch.as_ref(), &request.input.project_root) {
        Ok(child) => child,
        Err(_) => return request.fail(EngineOperationError::SpawnFailed),
    };
    let stdin_opt = child.stdin.take();
    let stdout_opt = child.stdout.take();
    let stderr_opt = child.stderr.take();
    let lifeline = LifelineWriter::take(&mut child);
    let stderr_counter = StderrCounter::new(stderr_opt, runtime.bounds.stderr_cap_bytes);
    let (mut stdin, stdout) = match (stdin_opt, stdout_opt) {
        (Some(stdin), Some(stdout)) => (stdin, stdout),
        _ => {
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
        }
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
    if read_codex_line(
        &mut reader,
        &mut line,
        phase_deadline(runtime.limits.prompt, request.deadline),
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
    if !is_codex_result_for(&line, 1) {
        return finish_configured_start(
            request,
            parts,
            EngineOperationError::ProviderRequestFailed,
            runtime.limits.close,
        )
        .await;
    }
    // thread/start --------------------------------------------------------
    let thread_line = codex_runtime::request_line(
        next_id,
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
    line.clear();
    if read_codex_line(
        &mut reader,
        &mut line,
        phase_deadline(runtime.limits.prompt, request.deadline),
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
    let Some(thread_id) = codex_thread_id(&line, 2) else {
        return finish_configured_start(
            request,
            parts,
            EngineOperationError::ProviderRequestFailed,
            runtime.limits.close,
        )
        .await;
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
    let prompt_text = input
        .prompt
        .text()
        .map(|text| text.as_str().to_owned())
        .unwrap_or_default();
    let turn_params = serde_json::json!({ "input": [{ "text": prompt_text, "text_elements": [], "type": "text" }] });
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
    let mut active_turn: Option<String> = None;
    let mut frame_sequence: u64 = 0;
    let mut last_activity = Instant::now();
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
    Terminal(super::observation::TerminalState),
    Failed(EngineOperationError),
}

#[allow(clippy::too_many_arguments)]
async fn codex_pump_loop(
    reader: &mut tokio::io::BufReader<tokio::process::ChildStdout>,
    stdin: &mut tokio::process::ChildStdin,
    parts: &mut ChildParts,
    line: &mut String,
    run_id: &artisan_domain::RunId,
    tracker: &mut super::codex::CodexPendingTracker,
    active_turn: &mut Option<String>,
    frame_sequence: &mut u64,
    last_activity: &mut Instant,
    inactivity: Duration,
    deadline: Instant,
    shutdown: &Arc<CancelHandle>,
    control: &Arc<CancelHandle>,
    observations: &mpsc::Sender<EngineObservation>,
    thread_id: &str,
) -> CodexPumpOutcome {
    use super::codex as codex_runtime;
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
                        match codex_runtime::parse_frame(&trimmed, *frame_sequence) {
                            Ok(event) => {
                                if let Some(terminal) = codex_runtime::apply_event(
                                    event,
                                    run_id,
                                    tracker,
                                    active_turn,
                                    observations,
                                    *frame_sequence,
                                ).await {
                                    return CodexPumpOutcome::Terminal(terminal);
                                }
                            }
                            Err(_) => continue,
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
            Ok(0) => Err(EngineOperationError::ProviderRequestFailed),
            Ok(_) => Ok(()),
            Err(_) => Err(EngineOperationError::ProviderRequestFailed),
        },
    }
}

/// Returns whether one handshake line is the result for the request id.
///
/// Bounds the line before parsing and requires a `result` member; anything
/// else fails the handshake closed without spawning further phases.
pub(crate) fn is_codex_result_for(line: &str, id: u64) -> bool {
    if line.len() > super::codex::CODEX_MAX_FRAME_BYTES {
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
    if line.len() > super::codex::CODEX_MAX_FRAME_BYTES {
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

/// Executes one finite Claude turn over `claude -p --output-format stream-json`.
///
/// Single-owner match arm beside the Codex executor: no second task, no
/// second queue. Writes the first user message over stdin, waits for the
/// `system/init` session identity behind the bind authorization gate, then
/// pumps the stream. Text deltas normalize onto the shared S1a vocabulary
/// with verbatim phases; `AskUserQuestion` frames populate pending questions
/// lifted out of the approval path; child transcript frames never adopt the
/// root turn. EOF before `result` maps to `Interrupted`, explicit cancel to
/// `Cancelled`, and stall/failure to `Failed`.
async fn execute_claude_turn(
    request: ConfiguredTurnRequest,
    runtime: ConfiguredRuntime,
    shutdown: &Arc<CancelHandle>,
) -> Execution {
    use super::claude as claude_runtime;
    use super::process::spawn_claude_engine;
    use tokio::io::AsyncBufReadExt as _;

    let artisan_domain::EngineSelection::Claude(selection) =
        request.input.settings.config().selection()
    else {
        return request.fail(EngineOperationError::Configuration);
    };
    let settings = match claude_runtime::ClaudeSettings::from_selection(selection) {
        Ok(settings) => settings,
        Err(_) => return request.fail(EngineOperationError::Configuration),
    };
    if settings.profile_id() != request.input.launch.profile_id() {
        return request.fail(EngineOperationError::Configuration);
    }
    let super::InternalLaunch::Claude(launch) = &request.input.launch else {
        return request.fail(EngineOperationError::Configuration);
    };
    if request.input.continuation.is_some() {
        return request.fail(EngineOperationError::Configuration);
    }
    let Some(session_id) = claude_runtime::new_session_id() else {
        return request.fail(EngineOperationError::EntropyFailed);
    };
    let session = claude_runtime::ClaudeSession::Start(session_id.clone());
    let args = settings.spawn_args(&session);
    let mut child = match spawn_claude_engine(launch.as_ref(), &request.input.project_root, &args) {
        Ok(child) => child,
        Err(_) => return request.fail(EngineOperationError::SpawnFailed),
    };
    let stdin_opt = child.stdin.take();
    let stdout_opt = child.stdout.take();
    let stderr_opt = child.stderr.take();
    let lifeline = LifelineWriter::take(&mut child);
    let stderr_counter = StderrCounter::new(stderr_opt, runtime.bounds.stderr_cap_bytes);
    let (mut stdin, stdout) = match (stdin_opt, stdout_opt) {
        (Some(stdin), Some(stdout)) => (stdin, stdout),
        _ => {
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
        }
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
    if let Some(prompt_text) = request
        .input
        .prompt
        .text()
        .map(|text| text.as_str().to_owned())
    {
        let line = settings.user_message_line(&session, &prompt_text);
        if claude_runtime::write_line(&mut stdin, &line).await.is_err() {
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
    // init are not replayed; a wrong session fails closed.
    let mut line = String::new();
    loop {
        line.clear();
        if read_claude_line(
            &mut reader,
            &mut line,
            phase_deadline(runtime.limits.prompt, request.deadline),
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
    } = request;
    if prepared
        .send(Ok(PreparedSession::new(session_id.clone())))
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

    // streaming pump ----------------------------------------------------------
    let mut stdin = Some(stdin);
    let mut tracker = claude_runtime::ClaudePendingTracker::new();
    let mut active_turn: Option<String> = None;
    let mut frame_sequence: u64 = 0;
    let mut last_activity = Instant::now();
    let inactivity = runtime.limits.sse;
    let terminal = claude_pump_loop(
        &mut reader,
        &mut stdin,
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
    )
    .await;
    drop(stdin);
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
            finish_turn_result(parts, Err(error), respond, runtime.limits.close).await
        }
    }
}

enum ClaudePumpOutcome {
    Terminal {
        state: super::observation::TerminalState,
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

#[allow(clippy::too_many_arguments)]
async fn claude_pump_loop(
    reader: &mut tokio::io::BufReader<tokio::process::ChildStdout>,
    stdin: &mut Option<tokio::process::ChildStdin>,
    parts: &mut ChildParts,
    line: &mut String,
    run_id: &artisan_domain::RunId,
    expected_session: &str,
    tracker: &mut super::claude::ClaudePendingTracker,
    active_turn: &mut Option<String>,
    frame_sequence: &mut u64,
    last_activity: &mut Instant,
    inactivity: Duration,
    deadline: Instant,
    shutdown: &Arc<CancelHandle>,
    control: &Arc<CancelHandle>,
    observations: &mpsc::Sender<EngineObservation>,
) -> ClaudePumpOutcome {
    use super::claude as claude_runtime;
    use tokio::io::AsyncBufReadExt as _;

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
            // adapter settles cancel without one); closing stdin is the only
            // turn-side signal before reporting cancellation.
            drop(stdin.take());
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
                drop(stdin.take());
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
            read = reader.read_line(line) => {
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
                        match claude_runtime::parse_frame(&trimmed, *frame_sequence) {
                            Ok(event) => {
                                let outcome = claude_runtime::apply_event(
                                    event,
                                    run_id,
                                    expected_session,
                                    tracker,
                                    active_turn,
                                    observations,
                                    *frame_sequence,
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
                                            drop(stdin.take());
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
                            Err(_) => continue,
                        }
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

async fn read_claude_line(
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
            Ok(0) => Err(EngineOperationError::ProviderRequestFailed),
            Ok(_) => Ok(()),
            Err(_) => Err(EngineOperationError::ProviderRequestFailed),
        },
    }
}

/// Executes one finite Grok turn over `grok agent stdio` through the shared
/// ACP core.
///
/// Single-owner match arm beside the Codex/Claude executors: no second task,
/// no second queue, no loop fork. Performs the bounded `initialize`
/// handshake with the row's auth classifier, `session/new` (continuations
/// fail closed at the shared fence, mirroring X1), the bind authorization
/// gate carrying the exact native session identity, then exactly one prompt
/// with the update pump. Streaming session updates carry no root-text
/// projection in G1 (a later packet); permission and elicitation agent
/// requests normalize through the A2 bridges into the pending table with no
/// control-flow side effect and are never auto-answered. EOF before the
/// prompt result maps to `Interrupted`, explicit cancel to `Cancelled`, and
/// stall/failure to `Failed`. Teardown closes the stdin lifeline first and
/// reaps within the close budget; an unobserved reap reports
/// `UnresolvedReapDuring` without owner quarantine (see `finish_grok_turn`).
#[allow(clippy::too_many_lines)]
async fn execute_grok_turn(
    request: ConfiguredTurnRequest,
    runtime: ConfiguredRuntime,
    shutdown: &Arc<CancelHandle>,
) -> Execution {
    use super::acp as acp_core;
    use super::grok as grok_runtime;

    let artisan_domain::EngineSelection::Grok(selection) =
        request.input.settings.config().selection()
    else {
        return request.fail(EngineOperationError::Configuration);
    };
    let settings = grok_runtime::GrokSettings::from_selection(selection);
    if settings.profile_id() != request.input.launch.profile_id() {
        return request.fail(EngineOperationError::Configuration);
    }
    let super::InternalLaunch::Grok(launch) = &request.input.launch else {
        return request.fail(EngineOperationError::Configuration);
    };
    let definition = settings.definition();
    let argv = (definition.build_args)(&settings.launch_args());
    let mut child = match acp_core::spawn_acp_child(
        launch.executable_path().as_os_str(),
        &argv,
        Some(std::path::Path::new(request.input.project_root.as_str())),
    ) {
        Ok(child) => child,
        Err(_) => return request.fail(EngineOperationError::SpawnFailed),
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
    let bounds = match acp_core::AcpBounds::new(
        runtime.bounds.max_sse_line,
        runtime.bounds.max_json_body,
        grok_runtime::GROK_MAX_SESSION_ID_BYTES,
        runtime.limits.prompt,
        runtime.limits.sse,
        runtime.limits.close,
    ) {
        Ok(bounds) => bounds,
        Err(_) => {
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
        }
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

    // session/new -----------------------------------------------------------
    // Fresh native thread only; continuations fail closed at the shared
    // fence above, mirroring X1.
    let cwd = request.input.project_root.as_str().to_owned();
    let session = tokio::select! {
        biased;
        () = shutdown.wait() => Err(EngineOperationError::Shutdown),
        () = request.control.wait() => Err(EngineOperationError::Cancelled),
        () = tokio::time::sleep_until(handshake_deadline) => Err(EngineOperationError::Deadline),
        result = transport.new_session(cwd.as_str()) => result.map_err(map_grok_acp_error),
    };
    let session = match session {
        Ok(session) => session,
        Err(error) => {
            return finish_grok_start(Some(transport), child, request, error, runtime.limits.close)
                .await;
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
            event = stderr_counter.pump(), if stderr_counter.state() == super::process::StderrState::Open => {
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
    let mut bridges = super::acp_bridges::PendingBridgeTable::new();
    let outcome = grok_pump_loop(
        &mut transport,
        &session,
        &prompt_id,
        &mut bridges,
        deadline,
        shutdown,
        &control,
        &mut stderr_counter,
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
    Terminal(super::observation::TerminalState),
    Failed(EngineOperationError),
}

/// Drives one authorized Grok prompt round to its terminal state.
///
/// Mirrors the Codex pump structure over the ACP update loop: owner
/// shutdown, explicit cancellation (with a best-effort provider cancel),
/// and the attempt deadline preempt the transport; EOF before the prompt
/// result is interruption, a silent window is failure, and only the matching
/// prompt result settles the turn. Agent requests normalize into the pending
/// bridge table; streaming updates await the later projection packet.
#[allow(clippy::too_many_arguments)]
async fn grok_pump_loop(
    transport: &mut super::acp::AcpTransport<
        tokio::process::ChildStdout,
        tokio::process::ChildStdin,
    >,
    session: &super::acp::SessionId,
    prompt: &super::acp::AcpId,
    bridges: &mut super::acp_bridges::PendingBridgeTable,
    deadline: Instant,
    shutdown: &Arc<CancelHandle>,
    control: &Arc<CancelHandle>,
    stderr_counter: &mut StderrCounter,
) -> GrokPumpOutcome {
    use super::acp::AcpError;
    use super::acp::UpdateEvent;

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
            event = stderr_counter.pump(), if stderr_counter.state() == super::process::StderrState::Open => {
                let _ = event;
            }
            update = transport.next_update(session, prompt) => match update {
                Ok(UpdateEvent::SessionUpdate(_)) => {}
                Ok(UpdateEvent::AgentRequest { id, method, params }) => {
                    note_grok_agent_request(bridges, &id, &method, &params);
                }
                Ok(UpdateEvent::PromptResult(outcome)) => {
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
    table: &mut super::acp_bridges::PendingBridgeTable,
    id: &super::acp::AcpId,
    method: &str,
    params: &Value,
) {
    if method == super::grok::GROK_PERMISSION_METHOD {
        if let Ok(pending) = super::acp_bridges::normalize_permission_request(params) {
            let _ = table.insert_approval(super::grok::GROK_MAX_PENDING_REQUESTS, pending);
        }
    } else if method == super::grok::GROK_ELICITATION_METHOD {
        let provider_id = match id {
            super::acp::AcpId::Number(number) => number.to_string(),
            super::acp::AcpId::Text(text) => text.clone(),
        };
        if let Ok(pending) =
            super::acp_bridges::normalize_elicitation_request(provider_id.as_str(), params)
        {
            let _ = table.insert_elicitation(super::grok::GROK_MAX_PENDING_REQUESTS, pending);
        }
    }
}

/// Maps one ACP core failure onto the owner error vocabulary.
///
/// Cancellation stays distinct; a protocol version mismatch surfaces as
/// `IncompatibleVersion`; every other wire failure (including auth,
/// framing, stall, and child errors) is a provider request failure. No
/// provider bytes cross this boundary: the core error is payload-free.
fn map_grok_acp_error(error: super::acp::AcpError) -> EngineOperationError {
    match error {
        super::acp::AcpError::Cancelled => EngineOperationError::Cancelled,
        super::acp::AcpError::UnsupportedVersion => EngineOperationError::IncompatibleVersion,
        _ => EngineOperationError::ProviderRequestFailed,
    }
}

/// Runs the fixed pre-prompt teardown for a faulted Grok start and settles
/// both owner channels: the stdin lifeline closes first so an EOF-clean
/// agent can exit on its own, then the child reaps within the close budget.
async fn finish_grok_start(
    transport: Option<
        super::acp::AcpTransport<tokio::process::ChildStdout, tokio::process::ChildStdin>,
    >,
    child: super::acp::AcpChild,
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
/// carries no observable wait): the retained handle drops — the spawn sets
/// `kill_on_drop`, and ACP children hold no ports or secrets — while the
/// caller still observes `UnresolvedReapDuring`. Full quarantine returns
/// with the verified-launch authority packet.
async fn finish_grok_turn(
    child: super::acp::AcpChild,
    result: TurnResult,
    respond: oneshot::Sender<TurnResult>,
    close_budget: Duration,
) -> Execution {
    match super::acp::shutdown_acp_child(child, close_budget).await {
        super::acp::AcpShutdown::ReapedWithoutKill(_)
        | super::acp::AcpShutdown::ReapedAfterKill(_) => {
            let _ = respond.send(result);
            Execution::Completed
        }
        super::acp::AcpShutdown::Retained(retained) => {
            drop(retained);
            let primary = result
                .err()
                .map_or_else(|| Box::new(EngineOperationError::ReapUnresolved), Box::new);
            let _ = respond.send(Err(EngineOperationError::UnresolvedReapDuring { primary }));
            Execution::Completed
        }
    }
}

/// Executes one finite Cursor turn over the shared ACP core.
///
/// Single-owner match arm beside the Codex and Claude executors: no second
/// task, no second queue. C1 proves admission agreement (durable cursor
/// selection, typed [`CursorSettings`](super::cursor::CursorSettings), and
/// the cursor launch capability carry the same managed profile, and no
/// provider continuation travels with the turn) and then fails closed: the
/// probe authority, live spawn/pump, catalog merge, and frontend selection
/// belong to later packets. The cursor-shaped ACP wire itself is proven by
/// the fixture script tests in `super::cursor`, which drive the exact
/// definition row (`--model` resolution, `--mode ask`, `--force`, `acp`;
/// image-block mode; permission deny-then-allow; plan-approval extensions;
/// resume; cancel/close; malformed frames; `AE-PROVIDER-206`) through the
/// shared transport core without spawning the real CLI.
async fn execute_cursor_turn(
    request: ConfiguredTurnRequest,
    _runtime: ConfiguredRuntime,
    _shutdown: &Arc<CancelHandle>,
) -> Execution {
    use super::cursor as cursor_runtime;

    let artisan_domain::EngineSelection::Cursor(selection) =
        request.input.settings.config().selection()
    else {
        return request.fail(EngineOperationError::Configuration);
    };
    let settings = match cursor_runtime::CursorSettings::from_selection(selection) {
        Ok(settings) => settings,
        Err(_) => return request.fail(EngineOperationError::Configuration),
    };
    if settings.profile_id() != request.input.launch.profile_id() {
        return request.fail(EngineOperationError::Configuration);
    }
    let super::InternalLaunch::Cursor(launch) = &request.input.launch else {
        return request.fail(EngineOperationError::Configuration);
    };
    if launch.profile_id() != settings.profile_id() {
        return request.fail(EngineOperationError::Configuration);
    }
    if request.input.continuation.is_some() {
        return request.fail(EngineOperationError::Configuration);
    }
    let _definition = cursor_runtime::CursorSettings::definition();
    request.fail(EngineOperationError::Configuration)
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
        super::InternalLaunch::Verified(_) => {
            StreamState::for_run(input.run_id.clone(), session.clone(), stream_after)
        }
        super::InternalLaunch::Codex(_) => Err(StreamError::InvalidSession),
        super::InternalLaunch::Claude(_) => Err(StreamError::InvalidSession),
        super::InternalLaunch::Grok(_) => Err(StreamError::InvalidSession),
        super::InternalLaunch::Cursor(_) => Err(StreamError::InvalidSession),
        #[cfg(test)]
        super::InternalLaunch::Fixture(_) => Ok(StreamState::new(stream_after)),
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
            session.input.prompt.text().map(|text| text.as_str()),
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

fn stream_usage_context(input: &super::InternalTurnInput) -> Option<StreamUsageContext> {
    let thread_id = input.thread_id.as_ref()?.clone();
    // Usage attribution is OpenCode2-shaped; other selections carry no
    // usage scope instead of attributing as OpenCode2. Claude usage capture
    // beyond the tracker plumbing in `super::claude` is a later packet.
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

async fn finish_turn_result(
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

fn map_resume_error(error: ResumeError) -> EngineOperationError {
    match error {
        ResumeError::Shutdown => EngineOperationError::Shutdown,
        ResumeError::Cancelled => EngineOperationError::Cancelled,
        ResumeError::Timeout => EngineOperationError::Deadline,
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

fn map_catalog_error(error: CatalogError) -> EngineOperationError {
    match error {
        CatalogError::Shutdown => EngineOperationError::Shutdown,
        CatalogError::Cancelled => EngineOperationError::Cancelled,
        CatalogError::Timeout => EngineOperationError::Deadline,
        other => EngineOperationError::CatalogFailed(other),
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
