//! Public parent-side owner of the internal directory-helper conversation.
//!
//! [`DirectoryController`] is the one non-Clone facade over a single owner
//! task running on an explicitly supplied Tokio runtime handle. The task
//! owns at most one active helper child at a time—its exact
//! [`tokio::process::Child`], the taken sole stdin lifeline writer, bounded
//! stdout/stderr reader state, the burned operation generation, and every
//! cleanup and reap decision—and serves a bounded queue of four jobs with
//! immediate backpressure. There is no per-job task, no self-created
//! runtime, no global registry, and no parallel controller.
//!
//! Composition precondition: Forge assembly creates ONE controller for the
//! process lifetime and never recreates it after failure or drop. Public
//! construction cannot mechanically prove process-wide uniqueness; that is
//! a trusted assembly rule, documented here and enforced nowhere else.
//!
//! Every operation is admitted through a non-async method returning one
//! single-owner [`PickOperation`] future. Its private abandonment signal—a
//! shared transport [`CancelHandle`]—is installed before admission, so
//! dropping even an unpolled accepted future notifies the owner and triggers
//! real child cleanup. Dropping, timing out, or cancelling is controller
//! plumbing; it is never reported as the chooser's user-facing `Cancelled`
//! outcome.
//!
//! Success requires all of: an exact valid response frame for this
//! operation's generation, clean stdout end-of-stream proving no trailing
//! bytes, stderr completed within its count-only cap, and an actual exit
//! status of zero observed while the sole stdin writer was still held. Only
//! then are resources released, the result published, and the next job
//! admitted. A locally published result makes no claim about UI consumption
//! or selection-authority registration.
//!
//! Payload hygiene: no public error, Debug, or Display output carries path
//! text, payload bytes, executable paths, stderr content, or raw
//! operating-system error strings.

use std::fmt;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use artisan_domain::{ROOT_PATH_MAX_BYTES, RootPath};
use artisan_transport::CancelHandle;
use thiserror::Error;
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot, watch};

mod operation;
mod process;

#[cfg(test)]
#[path = "../../../tests/backend/directory_controller.rs"]
mod directory_controller_tests;

use operation::{HealthState as OwnerHealth, Job, RequestPayload, run_owner};

/// Fixed bounded queue capacity: one active child plus at most four queued
/// jobs.
const QUEUE_CAPACITY: usize = operation::QUEUE_CAPACITY;

/// Configuration for one controller instance.
///
/// The executable must be the exact absolute trusted Forge path; relative
/// values are rejected at start and nothing ever discovers siblings,
/// consults `PATH`, or falls back to source-tree or runfiles locations.
pub struct DirectoryControllerConfig {
    forge_executable: PathBuf,
}

impl DirectoryControllerConfig {
    /// Creates a configuration around the exact absolute executable path.
    #[must_use]
    pub fn new(forge_executable: PathBuf) -> Self {
        Self { forge_executable }
    }

    /// Returns the configured exact executable path.
    #[must_use]
    pub fn forge_executable(&self) -> &Path {
        &self.forge_executable
    }
}

impl fmt::Debug for DirectoryControllerConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The executable path is never exposed through Debug output.
        formatter.write_str("DirectoryControllerConfig { forge_executable: <withheld> }")
    }
}

/// Why a controller could not start.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum ControllerStartError {
    /// The supplied executable path was not absolute; nothing was spawned.
    #[error("the forge executable path must be absolute")]
    RelativeExecutable,
    /// The supplied runtime handle refused the owner task spawn.
    #[error("the supplied tokio runtime refused the owner task")]
    RuntimeUnavailable,
}

/// Why an operation was not admitted to the queue.
///
/// Admission is immediate: there is no async waiting for queue space.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum AdmissionError {
    /// The controller has been shut down or quarantined.
    #[error("the directory controller is no longer accepting work")]
    Unavailable,
    /// One active child plus the full bounded queue already occupy the
    /// controller.
    #[error("the directory controller queue is full")]
    Busy,
    /// The caller-supplied budget cannot form a representable deadline.
    #[error("the operation budget cannot form a representable deadline")]
    InvalidDeadline,
    /// The validate-path text was empty; nothing was queued.
    #[error("the validate path text must not be empty")]
    EmptyPath,
    /// The validate-path text exceeded the shared root-path byte ceiling.
    #[error("the validate path text exceeds the shared byte ceiling")]
    PathTooLong,
}

/// Typed, payload-free failure of one directory-helper operation.
///
/// Variants carry no filesystem text, payload bytes, executable paths, or
/// operating-system error strings; raw I/O errors stay private to the owner
/// task and are reduced to these causes.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum HelperOperationError {
    /// The future was dropped or explicitly cancelled; this is controller
    /// plumbing and never the chooser's user-facing cancellation.
    #[error("the operation was cancelled or abandoned by its caller")]
    Cancelled,
    /// The caller-supplied budget elapsed, queue waiting included.
    #[error("the operation did not settle within its budget")]
    Deadline,
    /// The controller shut down before the operation settled.
    #[error("the directory controller shut down during the operation")]
    Shutdown,
    /// The owner task or its channels closed unexpectedly.
    #[error("the directory controller owner task was lost")]
    TaskLost,
    /// The checked generation space is exhausted; no further child can be
    /// launched by this controller instance.
    #[error("the generation space is exhausted")]
    GenerationExhausted,
    /// The child could not be spawned.
    #[error("the helper child could not be spawned")]
    SpawnFailed,
    /// An out-of-contract request reached the wire stage; admission
    /// validation should have refused it earlier.
    #[error("the request frame violated its own admission invariants")]
    InvalidRequest,
    /// The request frame could not be written to the child's stdin.
    #[error("the request frame could not be written to the helper")]
    WriteFailed,
    /// A pipe read failed while acquiring the response.
    #[error("a pipe read failed while acquiring the response")]
    ReadFailed,
    /// The response header violated the v1 structural rules.
    #[error("the response frame was malformed")]
    MalformedFrame,
    /// End of stream arrived mid-frame.
    #[error("the response frame was truncated")]
    TruncatedFrame,
    /// Bytes followed the complete response frame.
    #[error("trailing output followed the response frame")]
    TrailingOutput,
    /// The response echoed another generation and was discarded.
    #[error("the response carried a stale generation")]
    StaleGeneration,
    /// The declared response payload exceeded the shared bound.
    #[error("the response payload exceeded the shared bound")]
    OversizedOutput,
    /// Stderr crossed its count-only cap; its content stays unknown.
    #[error("helper stderr exceeded the count-only cap")]
    StderrCapExceeded,
    /// The child exited nonzero after an otherwise well-formed exchange.
    #[error("the helper exited nonzero")]
    ExitFailure,
    /// A primary failure whose separately observed cleanup could not confirm
    /// the child's reap within the fixed budgets: both facts travel together,
    /// neither erasing the other.
    #[error("{primary}; the child's reap stayed unobserved and custody was retained")]
    UnresolvedReapDuring {
        /// The original typed cause of the abnormal ending.
        primary: Box<HelperOperationError>,
    },
    /// Cleanup could not observe the child's death and no primary operation
    /// cause existed to preserve alongside it.
    #[error("the child's reap could not be observed; the controller retained custody")]
    ReapUnresolved,
}

/// Typed outcome of one successful helper conversation.
#[derive(Clone, Eq, PartialEq)]
pub enum DirectoryPickOutcome {
    /// The helper produced this canonical directory text (validated for
    /// lossless UTF-8, domain bounds, and absolute supported form).
    Selected {
        /// Canonical directory text within the shared byte ceiling.
        canonical_path: String,
    },
    /// The user dismissed the native chooser.
    Cancelled,
    /// The candidate path cannot serve as a project root.
    InvalidPath,
    /// A required string could not cross losslessly as UTF-8.
    UnsupportedEncoding,
    /// This platform offers no supported chooser implementation.
    UnsupportedPlatform,
    /// The native dialog failed before producing a decision.
    DialogFailed,
}

impl fmt::Debug for DirectoryPickOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Selected { .. } => formatter.write_str("Selected { canonical_path: <withheld> }"),
            Self::Cancelled => formatter.write_str("Cancelled"),
            Self::InvalidPath => formatter.write_str("InvalidPath"),
            Self::UnsupportedEncoding => formatter.write_str("UnsupportedEncoding"),
            Self::UnsupportedPlatform => formatter.write_str("UnsupportedPlatform"),
            Self::DialogFailed => formatter.write_str("DialogFailed"),
        }
    }
}

impl fmt::Display for DirectoryPickOutcome {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Selected { .. } => formatter.write_str("selected (<path withheld>)"),
            Self::Cancelled => formatter.write_str("cancelled"),
            Self::InvalidPath => formatter.write_str("invalid path"),
            Self::UnsupportedEncoding => formatter.write_str("unsupported encoding"),
            Self::UnsupportedPlatform => formatter.write_str("unsupported platform"),
            Self::DialogFailed => formatter.write_str("dialog failed"),
        }
    }
}

/// Result carried by one [`PickOperation`] future.
pub type OperationResult = Result<DirectoryPickOutcome, HelperOperationError>;

/// Single-owner future resolving to exactly one helper conversation result.
///
/// Deliberately not `Clone`: two owners must never believe they each observe
/// the one delivery. Dropping the future cancels its private signal before
/// admission ordering guarantees the owner learns of the abandonment and
/// runs real cleanup; the drop is never interpreted as the chooser's
/// `Cancelled` outcome.
pub struct PickOperation {
    receiver: oneshot::Receiver<OperationResult>,
    control: Arc<CancelHandle>,
}

impl PickOperation {
    /// Cancels this operation explicitly.
    ///
    /// The owner observes the signal at its next scheduling point, closes
    /// the lifeline, terminates when required, and waits for the actual
    /// reap before releasing the single-operation slot.
    pub fn cancel(&self) {
        self.control.cancel();
    }
}

impl Future for PickOperation {
    type Output = OperationResult;

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        match Pin::new(&mut self.receiver).poll(context) {
            Poll::Ready(Ok(result)) => Poll::Ready(result),
            // The sender half vanished without sending: the owner task or
            // its channel closed. That is a failure, never a silent success.
            Poll::Ready(Err(_)) => Poll::Ready(Err(HelperOperationError::TaskLost)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for PickOperation {
    fn drop(&mut self) {
        // Installed-before-admission abandonment: even an unpolled future
        // dropping notifies the owner, which performs lifeline close,
        // termination when required, and the actual reap.
        self.control.cancel();
    }
}

/// Payload-free health of one controller instance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HealthState {
    /// Admission is open; the owner task is serving work.
    Active,
    /// The controller irreversibly stopped serving new work (unobserved
    /// reap quarantine or generation-space exhaustion).
    Quarantined,
}

/// Report of one shutdown attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShutdownReport {
    /// The owner task was observed to complete after draining its queue.
    Joined,
    /// The owner task ended without a normal completion (panic); containment
    /// remains best-effort only.
    TaskLost,
    /// The controller had quarantined itself; the report is deliberately
    /// incomplete, the facade and its stored join handle are not consumed,
    /// and a later call may observe eventual completion honestly.
    Quarantined,
}

/// The one parent-side owner of the directory-helper conversation.
///
/// See the module documentation for the ownership contract. Instances are
/// neither `Clone` nor `Copy`; the composition precondition above forbids
/// constructing replacements after failure or drop.
pub struct DirectoryController {
    jobs: mpsc::Sender<Job>,
    shutdown: Arc<CancelHandle>,
    health: watch::Receiver<OwnerHealth>,
    join: tokio::task::JoinHandle<()>,
    /// Cached completion verdict for the owner task. A completed Tokio
    /// `JoinHandle` consumes its output on the delivering poll and panics if
    /// polled again (pinned 1.53.1 `runtime/task/harness.rs` takes the
    /// output; `core.rs` marks the task `Consumed`), so the observation is
    /// recorded exactly once and replayed by later shutdown calls.
    observed_join: Option<bool>,
}

impl fmt::Debug for DirectoryController {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DirectoryController { <payload-free> }")
    }
}

impl DirectoryController {
    /// Starts the single owner task on the supplied runtime handle.
    ///
    /// No runtime is created here; the caller's handle owns all scheduling.
    ///
    /// # Errors
    ///
    /// Returns [`ControllerStartError::RelativeExecutable`] for a non-
    /// absolute executable path.
    pub fn start(
        config: DirectoryControllerConfig,
        runtime: &Handle,
    ) -> Result<Self, ControllerStartError> {
        // The trusted-executable rule is enforced at the production entry:
        // relative paths are refused before any recipe or runtime exists.
        if !config.forge_executable.is_absolute() {
            return Err(ControllerStartError::RelativeExecutable);
        }
        let recipe = process::LaunchRecipe::Production {
            executable: config.forge_executable,
        };
        Ok(Self::start_with_recipe(recipe, runtime))
    }

    /// Shared wiring for production and the private test recipe.
    fn start_with_recipe(recipe: process::LaunchRecipe, runtime: &Handle) -> Self {
        let (jobs, pending) = mpsc::channel::<Job>(QUEUE_CAPACITY);
        let shutdown = Arc::new(CancelHandle::new());
        let (health_sender, health) = watch::channel(OwnerHealth::Active);
        let join = runtime.spawn(run_owner(
            pending,
            Arc::clone(&shutdown),
            health_sender,
            recipe,
        ));
        Self {
            jobs,
            shutdown,
            health,
            join,
            observed_join: None,
        }
    }

    /// Returns the current payload-free health state.
    #[must_use]
    pub fn health(&self) -> HealthState {
        match *self.health.borrow() {
            OwnerHealth::Active => HealthState::Active,
            OwnerHealth::Quarantined => HealthState::Quarantined,
        }
    }

    /// Admits one native chooser operation.
    ///
    /// The returned future is the single owner of the outcome; dropping or
    /// explicitly cancelling it triggers real child cleanup. `budget` is
    /// measured from this call and includes queue waiting; a zero budget
    /// expires before any child is launched. There is no default timeout.
    ///
    /// # Errors
    ///
    /// Returns [`AdmissionError`] immediately when the controller is
    /// unavailable, the bounded queue is full, the deadline is
    /// unrepresentable, or (for [`Self::validate_directory`]) the path text
    /// fails its pre-admission bounds.
    pub fn pick_directory(&self, budget: Duration) -> Result<PickOperation, AdmissionError> {
        self.admit(budget, RequestPayload::Pick)
    }

    /// Admits one validate operation carrying the exact absolute path text.
    ///
    /// The text travels verbatim; nothing trims or rewrites it. Bounds are
    /// checked before admission so doomed payloads never occupy the queue.
    ///
    /// # Errors
    ///
    /// See [`Self::pick_directory`].
    pub fn validate_directory(
        &self,
        budget: Duration,
        path_text: &str,
    ) -> Result<PickOperation, AdmissionError> {
        if path_text.is_empty() {
            return Err(AdmissionError::EmptyPath);
        }
        if path_text.len() > ROOT_PATH_MAX_BYTES {
            return Err(AdmissionError::PathTooLong);
        }
        self.admit(budget, RequestPayload::Validate(path_text.to_owned()))
    }

    /// Common admission path: controls installed first, then deadline check,
    /// then the immediate bounded `try_send`.
    fn admit(
        &self,
        budget: Duration,
        request: RequestPayload,
    ) -> Result<PickOperation, AdmissionError> {
        if *self.health.borrow() != OwnerHealth::Active || self.shutdown.is_cancelled() {
            return Err(AdmissionError::Unavailable);
        }
        let Some(deadline) = tokio::time::Instant::now().checked_add(budget) else {
            return Err(AdmissionError::InvalidDeadline);
        };

        // The private abandonment signal exists before the job is queued so
        // a dropped-but-never-polled future still notifies the owner.
        let control = Arc::new(CancelHandle::new());
        let (respond, receiver) = oneshot::channel();
        let job = Job {
            request,
            deadline,
            control: Arc::clone(&control),
            respond,
        };
        match self.jobs.try_send(job) {
            Ok(()) => Ok(PickOperation { receiver, control }),
            Err(mpsc::error::TrySendError::Full(_)) => Err(AdmissionError::Busy),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(AdmissionError::Unavailable),
        }
    }

    /// Shuts the controller down and reports what is actually observed.
    ///
    /// This is deliberately NOT an async method: the shutdown signal is
    /// raised here, before the returned future exists, so admission stops
    /// and orderly teardown starts even if that future is never polled.
    ///
    /// The returned future RETAINS both wake sources across Pending polls —
    /// the owned health-watch `changed()` waiter and the owner `JoinHandle` —
    /// so a quarantine arriving while the join is parked wakes it
    /// immediately; nothing is dropped between polls. Completion wins when
    /// both sources settle together, and the consumed join verdict is
    /// cached exactly once (a completed Tokio `JoinHandle` panics if polled
    /// again), so repeated calls honestly replay the observed ending. On a
    /// quarantined observation the report is explicitly incomplete: the
    /// facade and its join handle are untouched and later calls may still
    /// observe eventual completion.
    pub fn shutdown(&mut self) -> impl Future<Output = ShutdownReport> + '_ {
        self.shutdown.cancel();
        // Two INDEPENDENT receiver clones move into the future: `wait_rx`
        // is the sole mutable source for the watch waiter, `read_rx`
        // serves lock-free state checks. Nothing borrows out of `self`,
        // so join/verdict access stays conflict-free.
        let read_rx = self.health.clone();
        let mut wait_rx = self.health.clone();
        async move {
            let mut changed = Box::pin(wait_rx.changed());
            loop {
                // 1. A previously observed completion is authoritative.
                if let Some(joined_cleanly) = self.observed_join {
                    return if joined_cleanly {
                        ShutdownReport::Joined
                    } else {
                        ShutdownReport::TaskLost
                    };
                }
                // 2. COMPLETION FIRST, on every call: an already-settled join
                //    is consumed and cached before any quarantine verdict,
                //    so later calls can always observe eventual task
                //    completion even once health stays Quarantined.
                tokio::select! {
                    biased;

                    joined = &mut self.join => {
                        let joined_cleanly = joined.is_ok();
                        self.observed_join = Some(joined_cleanly);
                        return if joined_cleanly {
                            ShutdownReport::Joined
                        } else {
                            ShutdownReport::TaskLost
                        };
                    }
                    _ = &mut changed => {
                        // Consume the delivered update, then DROP the spent
                        // waiter entirely so its exclusive receiver borrow
                        // ends before a fresh one is armed. Recreation is
                        // lossless: `changed()` marks its value seen upon
                        // delivery, and the receiver's mark surfaces any
                        // unseen update immediately on the next wait.
                        drop(changed);
                        if *read_rx.borrow() == OwnerHealth::Quarantined {
                            return ShutdownReport::Quarantined;
                        }
                        changed = Box::pin(wait_rx.changed());
                    }
                }
            }
        }
    }
}

impl Drop for DirectoryController {
    fn drop(&mut self) {
        // Signal shutdown and release the sender; the stored join handle is
        // dropped, detaching the owner task, which retains cleanup and
        // quarantine custody while the caller's runtime lives.
        self.shutdown.cancel();
    }
}

/// Validates a `Selected` payload against the unchanged domain rules and the
/// absolute supported path form, without touching the filesystem.
///
/// The bytes must be lossless UTF-8, non-empty, accepted by the unchanged
/// domain [`RootPath`] (which preserves input bytes exactly), and absolute
/// in a supported Windows prefix form where applicable. No canonicalization
/// or metadata is performed here; the helper already resolved the path.
pub(crate) fn validate_selected_payload(payload: &[u8]) -> Result<String, ()> {
    let Ok(text) = std::str::from_utf8(payload) else {
        return Err(());
    };
    if text.is_empty() {
        return Err(());
    }
    if RootPath::parse(text).is_err() {
        return Err(());
    }
    let path = Path::new(text);
    if !path.is_absolute() {
        return Err(());
    }
    #[cfg(target_os = "windows")]
    if !has_supported_windows_prefix(path) {
        return Err(());
    }
    Ok(text.to_owned())
}

/// Whether a Windows path leads with a supported prefix (disk, verbatim
/// disk, UNC, or verbatim UNC), mirroring the helper's accepted forms
/// without any filesystem access.
#[cfg(target_os = "windows")]
fn has_supported_windows_prefix(path: &Path) -> bool {
    use std::path::Component;
    matches!(
        path.components().next(),
        Some(Component::Prefix(prefix))
            if matches!(
                prefix.kind(),
                std::path::Prefix::Disk(_)
                    | std::path::Prefix::VerbatimDisk(_)
                    | std::path::Prefix::UNC(..)
                    | std::path::Prefix::VerbatimUNC(..)
            )
    )
}

/// Re-exports powering the private headless test module: the test-only
/// launch recipe and the bounded phase witnesses. Nothing here exists in
/// production builds.
#[cfg(test)]
pub(crate) use operation::GenerationAllocator;
#[cfg(test)]
pub(crate) use process::{LaunchRecipe, reset_witnesses, witness_counts};
