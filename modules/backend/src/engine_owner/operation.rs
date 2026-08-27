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

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;

use artisan_domain::RunId;
use artisan_transport::CancelHandle;

use super::process::{
    ChildParts, CleanupObservation, LaunchRecipe, LifelineWriter, RetainedEngine, StderrCounter,
    cleanup_after_abort, eventual_wait_once, spawn_engine,
};

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
    close_budget: Duration,
    stderr_cap: usize,
) {
    run_owner_with_allocator(
        jobs,
        shutdown,
        health,
        recipe,
        close_budget,
        stderr_cap,
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
    close_budget: Duration,
    stderr_cap: usize,
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

                match execute_job(&recipe, generation, job, &shutdown, close_budget, stderr_cap).await {
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

/// Executes one admitted job end to end.
async fn execute_job(
    recipe: &LaunchRecipe,
    generation: u64,
    job: Job,
    shutdown: &Arc<CancelHandle>,
    close_budget: Duration,
    stderr_cap: usize,
) -> Execution {
    let Job {
        run_id: _,
        deadline,
        control,
        respond,
    } = job;

    // Pre-execution dispositions were already handled by the owner loop;
    // re-check before the spawn attempt in case the job waited in the queue.
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

    let Ok(spawned) = spawn_engine(recipe) else {
        let _ = respond.send(Err(EngineOperationError::SpawnFailed));
        return Execution::Completed;
    };
    let mut child = spawned;
    let lifeline = LifelineWriter::take(&mut child);
    let stderr_counter = StderrCounter::new(child.stderr.take(), stderr_cap);

    let parts = ChildParts {
        child,
        lifeline,
        stderr_counter,
    };

    drive_until_deadline_or_control(
        parts,
        generation,
        deadline,
        &control,
        shutdown,
        respond,
        close_budget,
    )
    .await
}

/// Drives one spawned child until its deadline or a control signal fires,
/// then performs the fixed bounded teardown and settles the response.
async fn drive_until_deadline_or_control(
    mut parts: ChildParts,
    generation: u64,
    deadline: Instant,
    control: &Arc<CancelHandle>,
    shutdown: &Arc<CancelHandle>,
    respond: oneshot::Sender<LaunchResult>,
    close_budget: Duration,
) -> Execution {
    let mut status: Option<std::io::Result<std::process::ExitStatus>> = None;
    let mut fault: Option<EngineOperationError> = None;

    loop {
        if shutdown.is_cancelled() {
            return finish_aborted(parts, EngineOperationError::Shutdown, respond, close_budget)
                .await;
        }
        if control.is_cancelled() {
            return finish_aborted(
                parts,
                EngineOperationError::Cancelled,
                respond,
                close_budget,
            )
            .await;
        }
        if Instant::now() >= deadline {
            return finish_aborted(parts, EngineOperationError::Deadline, respond, close_budget)
                .await;
        }

        tokio::select! {
            biased;

            () = shutdown.wait() => {
                return finish_aborted(parts, EngineOperationError::Shutdown, respond, close_budget).await;
            }
            () = control.wait() => {
                return finish_aborted(parts, EngineOperationError::Cancelled, respond, close_budget).await;
            }
            () = tokio::time::sleep_until(deadline) => {
                return finish_aborted(parts, EngineOperationError::Deadline, respond, close_budget).await;
            }

            event = parts.stderr_counter.pump(), if parts.stderr_counter.state() == crate::engine_owner::process::StderrState::Open => {
                match event {
                    crate::engine_owner::process::StderrEvent::WithinCap
                    | crate::engine_owner::process::StderrEvent::Closed => {}
                    crate::engine_owner::process::StderrEvent::CapExceeded
                    | crate::engine_owner::process::StderrEvent::ReadFailed => {
                        let _ = event;
                    }
                }
            }

            waited = parts.child.wait(), if status.is_none() => {
                match waited {
                    Ok(exit_status) => status = Some(Ok(exit_status)),
                    Err(_) => fault = Some(EngineOperationError::ReapUnresolved),
                }
            }
        }

        if let Some(Ok(exit_status)) = status.as_ref() {
            let success = exit_status.success();
            #[cfg(test)]
            super::process::note_observed_reap_for_tests(*exit_status);
            let outcome = LaunchOutcome::ObservedExit {
                generation,
                success,
            };
            drop(parts);
            let _ = respond.send(Ok(outcome));
            return Execution::Completed;
        }
        if let Some(cause) = fault.take() {
            return finish_aborted(parts, cause, respond, close_budget).await;
        }
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
