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

use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;

use artisan_domain::RunId;
use artisan_transport::CancelHandle;

use super::http::{HealthError, HealthSecret};
use super::process::{
    ChildParts, CleanupObservation, LaunchRecipe, LifelineWriter, RetainedEngine, StderrCounter,
    cleanup_after_abort, eventual_wait_once, spawn_engine,
};
use super::readiness::ReadinessError;
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
pub(crate) struct Job {
    /// The target run identity.
    pub(crate) run_id: RunId,
    /// Absolute checked deadline measured from the admission call.
    pub(crate) deadline: Instant,
    /// Abandonment and explicit-cancellation signal installed before admission.
    pub(crate) control: Arc<CancelHandle>,
    /// Single-use response slot back to the caller's future.
    pub(crate) respond: oneshot::Sender<LaunchResult>,
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
    run_owner_with_allocator(
        jobs,
        shutdown,
        health,
        recipe,
        limits,
        bounds,
        GenerationAllocator::new(),
    )
    .await;
}

/// Variant that starts from a caller-supplied allocator (test-seeded).
pub(crate) async fn run_owner_with_allocator(
    mut jobs: mpsc::Receiver<Job>,
    shutdown: Arc<CancelHandle>,
    health: watch::Sender<HealthState>,
    recipe: LaunchRecipe,
    limits: EngineLimits,
    bounds: EngineBounds,
    mut generations: GenerationAllocator,
) {
    loop {
        tokio::select! {
            biased;

            () = shutdown.wait() => break,

            job = jobs.recv() => {
                let Some(job) = job else { break };
                if shutdown.is_cancelled() {
                    let _ = job.respond.send(Err(EngineOperationError::Shutdown));
                    continue;
                }
                if job.control.is_cancelled() {
                    let _ = job.respond.send(Err(EngineOperationError::Cancelled));
                    continue;
                }
                if Instant::now() >= job.deadline {
                    let _ = job.respond.send(Err(EngineOperationError::Deadline));
                    continue;
                }
                let Some(generation) = generations.mint() else {
                    let _ = health.send(HealthState::Quarantined);
                    let _ = job.respond.send(Err(EngineOperationError::GenerationExhausted));
                    quarantine_tail(&mut jobs, None).await;
                    return;
                };

                match execute_job(&recipe, generation, job, &shutdown, limits, bounds).await {
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
        let _ = job.respond.send(Err(EngineOperationError::Shutdown));
    }
}

/// Serves the quarantine tail: closes admission immediately, drains the
/// already-bounded queue, and then resolves custody of the retained engine,
/// if any, exactly once.
async fn quarantine_tail(jobs: &mut mpsc::Receiver<Job>, retained: Option<Box<RetainedEngine>>) {
    jobs.close();
    while let Some(job) = jobs.recv().await {
        let _ = job.respond.send(Err(EngineOperationError::Shutdown));
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

/// Executes one admitted job end to end, including P3 readiness and health.
async fn execute_job(
    recipe: &LaunchRecipe,
    generation: u64,
    job: Job,
    shutdown: &Arc<CancelHandle>,
    limits: EngineLimits,
    bounds: EngineBounds,
) -> Execution {
    let Job {
        run_id: _,
        deadline,
        control,
        respond,
    } = job;
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
