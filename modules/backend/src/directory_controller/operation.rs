//! Private orchestration for the directory controller: the single owner
//! task, generation allocation, per-operation execution, and the quarantine
//! tail.
//!
//! One task owns at most one active child plus its taken sole stdin writer,
//! bounded stdout/stderr reader state, the burned generation, and every
//! cleanup decision. Work arrives through a bounded queue and is processed
//! strictly sequentially; there is no per-job task, no parallel controller,
//! and no replacement child before an observed reap.
//!
//! Fixed precedence, re-checked at the top of every scheduling cycle:
//! controller shutdown or terminal state, abandonment or explicit
//! cancellation, the operation deadline, and only then completion sources.
//! Once a cleanup sequence starts it runs to completion regardless of those
//! signals.

use tokio::sync::{mpsc, oneshot, watch};
use tokio::time::Instant;

use artisan_transport::CancelHandle;

use crate::directory_helper_codec::{
    RESPONSE_TAG_SELECTED, RequestEncodeFault, RequestKind, Response, encode_request,
};

use super::process::{
    BoundedStdoutReader, ChildParts, CleanupObservation, LaunchRecipe, LifelineWriter,
    RetainedHelper, StderrCounter, StderrEvent, StderrState, StdoutEvent, cleanup_after_abort,
    eventual_wait_once, spawn_helper,
};
use super::{
    DirectoryPickOutcome, HelperOperationError, OperationResult, validate_selected_payload,
};

/// Fixed bounded queue capacity: one active child plus at most four queued
/// jobs. Pending caller-held completed results live outside this bound.
pub(crate) const QUEUE_CAPACITY: usize = 4;

/// What the parent asked the helper to do, with its bounded payload.
#[derive(Debug)]
pub(crate) enum RequestPayload {
    /// Show the native chooser.
    Pick,
    /// Validate the supplied absolute path text (exact bytes preserved).
    Validate(String),
}

/// One admitted operation travelling from the facade to the owner task.
pub(crate) struct Job {
    /// The requested operation.
    pub(crate) request: RequestPayload,
    /// Absolute checked deadline measured from the operation call, queue
    /// waiting included.
    pub(crate) deadline: Instant,
    /// Abandonment/explicit-cancellation signal installed before admission.
    pub(crate) control: std::sync::Arc<CancelHandle>,
    /// Single-use response slot back to the caller's future.
    pub(crate) respond: oneshot::Sender<OperationResult>,
}

/// Payload-free controller health observable by the facade.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HealthState {
    /// Admission is open and the owner task is serving work.
    Active,
    /// The controller irreversibly stopped serving new work.
    Quarantined,
}

/// Checked nonzero generation allocator.
///
/// Values start at 1, are handed out immediately before each spawn attempt,
/// and are burned whether or not the spawn succeeds. Allocation stops
/// permanently at [`u64::MAX`]; values never wrap, reset, or repeat within
/// the process. Non-reuse claims do not extend across process restarts.
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

/// How one executed job ended for the owner loop.
pub(crate) enum Execution {
    /// The response slot was settled and no custody remains.
    Completed,
    /// Cleanup could not observe a reap; exact child-and-pipe custody was
    /// returned (boxed, keeping the owner-loop verdict small) for
    /// quarantine retention.
    Quarantined(Box<RetainedHelper>),
}

/// Runs the owner task until shutdown, channel closure, or quarantine.
///
/// On ordinary termination every still-queued job is rejected with
/// [`HelperOperationError::Shutdown`]. On quarantine the health notification
/// fires first, queued work is rejected while the channel lives, and the
/// retained child receives exactly one deadline-free eventual wait; a failed
/// wait parks the task forever while retaining custody.
pub(crate) async fn run_owner(
    mut jobs: mpsc::Receiver<Job>,
    shutdown: std::sync::Arc<CancelHandle>,
    health: watch::Sender<HealthState>,
    recipe: LaunchRecipe,
) {
    let mut generations = GenerationAllocator::new();

    loop {
        tokio::select! {
            biased;

            () = shutdown.wait() => break,

            job = jobs.recv() => {
                let Some(job) = job else { break };
                if shutdown.is_cancelled() {
                    let _ = job.respond.send(Err(HelperOperationError::Shutdown));
                    continue;
                }
                // Skip abandoned, cancelled, or expired work before spawn.
                if job.control.is_cancelled() {
                    let _ = job.respond.send(Err(HelperOperationError::Cancelled));
                    continue;
                }
                if Instant::now() >= job.deadline {
                    let _ = job.respond.send(Err(HelperOperationError::Deadline));
                    continue;
                }
                // Mint immediately before the spawn attempt; the value is
                // burned even when encoding or spawning fails below.
                let Some(generation) = generations.mint() else {
                    let _ = health.send(HealthState::Quarantined);
                    let _ = job.respond.send(Err(HelperOperationError::GenerationExhausted));
                    quarantine_tail(&mut jobs, None).await;
                    return;
                };

                match execute_job(&recipe, generation, job, &shutdown).await {
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

    // Ordinary termination: drain whatever remains queued.
    while let Ok(job) = jobs.try_recv() {
        let _ = job.respond.send(Err(HelperOperationError::Shutdown));
    }
}

/// Serves the quarantine tail: closes admission immediately, drains the
/// already-bounded queue, and then resolves custody of the retained child,
/// if any, exactly once.
///
/// Closing the receiver first guarantees termination without requiring the
/// facade to be dropped. When the retained helper's single deadline-free
/// wait errors, the task parks forever while keeping the exact child and
/// pipes alive in scope: no retry loop, no new child, no generation reset,
/// and no successful cleanup report.
async fn quarantine_tail(jobs: &mut mpsc::Receiver<Job>, retained: Option<Box<RetainedHelper>>) {
    // Admission closes here regardless of facade lifetime; recv() then
    // yields every already-buffered job before its final `None`.
    jobs.close();
    while let Some(job) = jobs.recv().await {
        let _ = job.respond.send(Err(HelperOperationError::Shutdown));
    }

    // Channel closed and fully drained: settle custody alone.
    if let Some(helper) = retained {
        match eventual_wait_once(helper).await {
            Ok(_status) => {}
            Err(retained) => {
                // The one deadline-free wait errored: park forever,
                // retaining the exact child and pipes.
                let _custody = retained;
                std::future::pending::<()>().await;
                unreachable!("pending never resolves");
            }
        }
    }
}

/// Why the write phase stopped before the request frame was delivered.
enum WriteStop {
    Shutdown,
    Cancelled,
    Deadline,
}

/// Executes one admitted job end to end.
///
/// The generation was already minted (and thereby burned) by the owner. Any
/// abnormal ending funnels into the fixed cleanup sequence; only a fully
/// observed success releases the resources without it.
async fn execute_job(
    recipe: &LaunchRecipe,
    generation: u64,
    job: Job,
    shutdown: &std::sync::Arc<CancelHandle>,
) -> Execution {
    let Job {
        request,
        deadline,
        control,
        respond,
    } = job;

    let (kind, payload_bytes): (RequestKind, &[u8]) = match &request {
        RequestPayload::Pick => (RequestKind::Pick, &[]),
        RequestPayload::Validate(path_text) => (RequestKind::Validate, path_text.as_bytes()),
    };

    let frame = match encode_request(generation, kind, payload_bytes) {
        Ok(frame) => frame,
        // Admission validation makes this unreachable in practice; the
        // refusal stays typed instead of writing an out-of-contract frame.
        Err(
            RequestEncodeFault::PickCarriesPayload
            | RequestEncodeFault::EmptyValidatePayload
            | RequestEncodeFault::ValidatePayloadBeyondBound { .. },
        ) => {
            let _ = respond.send(Err(HelperOperationError::InvalidRequest));
            return Execution::Completed;
        }
    };

    let Ok(mut child) = spawn_helper(recipe) else {
        // The generation was already burned above.
        let _ = respond.send(Err(HelperOperationError::SpawnFailed));
        return Execution::Completed;
    };

    // Take stdin immediately so no implicit close can ever occur, and split
    // the pipes out so the single owner task can borrow them disjointly.
    // The stdout reader binds this operation's expected generation so stale
    // headers are rejected before any payload allocation.
    let mut lifeline = LifelineWriter::take(&mut child);
    let stdout_reader = BoundedStdoutReader::new(child.stdout.take(), generation);
    let stderr_counter = StderrCounter::new(child.stderr.take());

    // ---- write phase -----------------------------------------------------
    let written: Result<(), WriteStop> = tokio::select! {
        biased;

        () = shutdown.wait() => Err(WriteStop::Shutdown),
        () = control.wait() => Err(WriteStop::Cancelled),
        () = tokio::time::sleep_until(deadline) => Err(WriteStop::Deadline),
        written = lifeline.write_frame(&frame) => if written.is_ok() {
            Ok(())
        } else {
            // The pipe rejected the request; the helper observes its own
            // lifeline ending. Run the full cleanup either way.
            let parts = ChildParts {
                child,
                lifeline,
                stdout_reader,
                stderr_counter,
            };
            return finish_aborted(
                parts,
                HelperOperationError::WriteFailed,
                respond,
            )
            .await;
        },
    };

    let parts = ChildParts {
        child,
        lifeline,
        stdout_reader,
        stderr_counter,
    };
    let parts = match written {
        Ok(()) => parts,
        Err(stop) => {
            let cause = match stop {
                WriteStop::Shutdown => HelperOperationError::Shutdown,
                WriteStop::Cancelled => HelperOperationError::Cancelled,
                WriteStop::Deadline => HelperOperationError::Deadline,
            };
            return finish_aborted(parts, cause, respond).await;
        }
    };

    // ---- main phase ------------------------------------------------------
    run_main_phase(parts, deadline, &control, shutdown, respond).await
}

/// Drives the post-write conversation: concurrent bounded stdout framing,
/// count-only stderr draining, and the owned child wait, all under the fixed
/// control precedence, followed by evaluation and cleanup.
async fn run_main_phase(
    mut parts: ChildParts,
    deadline: Instant,
    control: &std::sync::Arc<CancelHandle>,
    shutdown: &std::sync::Arc<CancelHandle>,
    respond: oneshot::Sender<OperationResult>,
) -> Execution {
    let mut frame: Option<(u8, Vec<u8>)> = None;
    let mut eof_clean: Option<bool> = None;
    let mut status: Option<std::io::Result<std::process::ExitStatus>> = None;
    let mut stdout_settled = false;
    let mut fault: Option<HelperOperationError> = None;

    let outcome = loop {
        // Fixed precedence, re-checked BEFORE any completion assembly and
        // again inside the biased select below: when an I/O observation and
        // a control become ready together, the control still wins.
        if shutdown.is_cancelled() {
            return finish_aborted(parts, HelperOperationError::Shutdown, respond).await;
        }
        if control.is_cancelled() {
            return finish_aborted(parts, HelperOperationError::Cancelled, respond).await;
        }
        if Instant::now() >= deadline {
            return finish_aborted(parts, HelperOperationError::Deadline, respond).await;
        }

        if let Some(evaluated) = evaluate_operation(
            frame.as_ref(),
            eof_clean,
            parts.stderr_counter.state(),
            status.as_ref(),
        ) {
            break evaluated;
        }
        if fault.is_some() {
            break Err(fault.unwrap_or(HelperOperationError::ReadFailed));
        }

        tokio::select! {
            biased;

            () = shutdown.wait() => {
                return finish_aborted(parts, HelperOperationError::Shutdown, respond).await;
            }
            () = control.wait() => {
                return finish_aborted(parts, HelperOperationError::Cancelled, respond).await;
            }
            () = tokio::time::sleep_until(deadline) => {
                return finish_aborted(parts, HelperOperationError::Deadline, respond).await;
            }

            event = parts.stdout_reader.pump(), if !stdout_settled => {
                note_stdout_event(
                    event,
                    &mut frame,
                    &mut eof_clean,
                    &mut stdout_settled,
                    &mut fault,
                );
            }

            event = parts.stderr_counter.pump(), if parts.stderr_counter.state() == StderrState::Open => {
                match event {
                    StderrEvent::WithinCap | StderrEvent::Closed => {}
                    StderrEvent::CapExceeded => {
                        fault = Some(HelperOperationError::StderrCapExceeded);
                    }
                    StderrEvent::ReadFailed => {
                        fault = Some(HelperOperationError::ReadFailed);
                    }
                }
            }

            waited = parts.child.wait(), if status.is_none() => {
                match waited {
                    Ok(exit_status) => status = Some(Ok(exit_status)),
                    // The operating-system wait itself failed; classify it
                    // and let the fixed cleanup sequence re-observe.
                    Err(_) => fault = Some(HelperOperationError::ReadFailed),
                }
            }
        }
    };

    match outcome {
        Ok(payload) => {
            // Success: the exit was already observed with stdin still open.
            // Record the observed reap against its real exit code, then
            // release lifeline and pipe resources, publish the result, and
            // let the owner loop admit the next job.
            if let Some(Ok(exit_status)) = &status {
                #[cfg(test)]
                super::process::note_observed_reap_for_tests(*exit_status);
                let _ = exit_status;
            }
            drop(parts);
            let _ = respond.send(Ok(payload));
            Execution::Completed
        }
        Err(cause) => finish_aborted(parts, cause, respond).await,
    }
}

/// Applies one bounded stdout observation to the accumulated conversation
/// facts, reducing structural faults to typed payload-free causes.
///
/// Split out of [`run_main_phase`] so the owner loop stays within bounds;
/// the classification behavior is unchanged.
fn note_stdout_event(
    event: StdoutEvent,
    frame: &mut Option<(u8, Vec<u8>)>,
    eof_clean: &mut Option<bool>,
    stdout_settled: &mut bool,
    fault: &mut Option<HelperOperationError>,
) {
    match event {
        StdoutEvent::NeedMore => {}
        StdoutEvent::FrameReady { tag, payload } => {
            // The reader already proved the echoed generation equals this
            // operation's own before allocating.
            *frame = Some((tag, payload));
        }
        StdoutEvent::EofClean => {
            *eof_clean = Some(true);
            *stdout_settled = true;
        }
        StdoutEvent::Truncated => {
            *fault = Some(HelperOperationError::TruncatedFrame);
            *stdout_settled = true;
        }
        StdoutEvent::Trailing => {
            *fault = Some(HelperOperationError::TrailingOutput);
            *stdout_settled = true;
        }
        StdoutEvent::StaleGeneration => {
            *fault = Some(HelperOperationError::StaleGeneration);
            *stdout_settled = true;
        }
        StdoutEvent::Malformed(response_fault) => {
            *fault = Some(match response_fault {
                crate::directory_helper_codec::ResponseHeaderFault::PayloadBeyondBound {
                    ..
                } => HelperOperationError::OversizedOutput,
                _ => HelperOperationError::MalformedFrame,
            });
            *stdout_settled = true;
        }
        StdoutEvent::ReadFailed => {
            *fault = Some(HelperOperationError::ReadFailed);
            *stdout_settled = true;
        }
    }
}

/// Assembles the operation verdict once enough observations exist.
///
/// Success requires every gate: an exact valid frame for this generation,
/// clean end-of-stream proving no trailing bytes, stderr finished within
/// its cap, and an actual exit status of zero. Any shortfall becomes the
/// highest-precedence typed cause (stderr faults before nonzero exits).
fn evaluate_operation(
    frame: Option<&(u8, Vec<u8>)>,
    eof_clean: Option<bool>,
    stderr_state: StderrState,
    status: Option<&std::io::Result<std::process::ExitStatus>>,
) -> Option<Result<DirectoryPickOutcome, HelperOperationError>> {
    let (tag, payload) = frame?;
    let Some(true) = eof_clean else {
        return None;
    };
    let status_result = status?;

    // Success demands an explicitly settled stderr within its cap; `Open`
    // simply means not every observation has landed yet.
    match stderr_state {
        StderrState::ClosedWithinCap => {}
        StderrState::Capped => return Some(Err(HelperOperationError::StderrCapExceeded)),
        StderrState::Failed => return Some(Err(HelperOperationError::ReadFailed)),
        StderrState::Open => return None,
    }

    let Ok(exit_status) = status_result else {
        return Some(Err(HelperOperationError::ReadFailed));
    };
    if !exit_status.success() {
        return Some(Err(HelperOperationError::ExitFailure));
    }

    if *tag != RESPONSE_TAG_SELECTED {
        // Tags 2 through 6 are empty-payload outcomes verified by codec
        // parsing; map them straight onto the public vocabulary.
        let outcome = match *tag {
            found if found == Response::Cancelled.tag() => DirectoryPickOutcome::Cancelled,
            found if found == Response::InvalidPath.tag() => DirectoryPickOutcome::InvalidPath,
            found if found == Response::UnsupportedEncoding.tag() => {
                DirectoryPickOutcome::UnsupportedEncoding
            }
            found if found == Response::UnsupportedPlatform.tag() => {
                DirectoryPickOutcome::UnsupportedPlatform
            }
            _ => DirectoryPickOutcome::DialogFailed,
        };
        return Some(Ok(outcome));
    }

    match validate_selected_payload(payload) {
        Ok(canonical_path) => Some(Ok(DirectoryPickOutcome::Selected { canonical_path })),
        Err(()) => Some(Err(HelperOperationError::MalformedFrame)),
    }
}

/// Runs the fixed cleanup for an aborted/faulted operation and settles the
/// response slot honestly.
///
/// The primary cause is preserved verbatim; the separately observed cleanup
/// status never overwrites it. An unobserved reap converts the ending into
/// [`HelperOperationError::ReapUnresolved`] and hands custody back to the
/// owner loop for quarantine.
async fn finish_aborted(
    parts: ChildParts,
    cause: HelperOperationError,
    respond: oneshot::Sender<OperationResult>,
) -> Execution {
    match cleanup_after_abort(parts).await {
        CleanupObservation::ReapedWithoutKill(_status)
        | CleanupObservation::ReapedAfterKill(_status) => {
            let _ = respond.send(Err(cause));
            Execution::Completed
        }
        CleanupObservation::Retained(helper) => {
            // Both facts travel together, typed and payload-free: the
            // primary cause is preserved, never replaced.
            let _ = respond.send(Err(HelperOperationError::UnresolvedReapDuring {
                primary: Box::new(cause),
            }));
            Execution::Quarantined(helper)
        }
    }
}
