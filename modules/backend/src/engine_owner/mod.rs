//! Validated immutable engine-owner configuration.
//!
//! `EngineLimits` and `EngineBounds` are caller-supplied raw literals and are
//! documented **UNVALIDATED** until accepted by [`EngineOwnerConfig::new`].
//! Validation is atomic and without effects: no task, runtime, channel,
//! semaphore, child, HTTP, or filesystem probe is performed here. The stored
//! absolute executable path bytes are preserved verbatim; no canonicalization
//! or `PATH`/runfiles lookup is performed. Zero `Duration`s are representable
//! (they expire before a controlled operation starts) and are not a shipping
//! close policy; real operation deadlines must be rechecked later against a
//! fresh `Instant`. This leaf does not claim total-memory or process/runtime
//! safety; future `cap + 1` uses remain separately checked.
//!
//! This module also hosts the [`EngineOwner`] facade: the one non-Clone
//! parent-side owner of the external engine process, running a single owner
//! task on a caller-supplied Tokio runtime handle. Admission, incarnation
//! numbering, child custody, bounded teardown, quarantine, readiness, and
//! HTTP exist; a pure, independently reviewable SSE framing and wakeable
//! bounded observation delivery leaf (`framing`/`observation` with
//! `reserve_owned` sink) plus a crate-private one-shot `POST /prompt` leaf
//! exists without claiming live `IncomingBody` driving, global/per-run SSE
//! connections, or owner integration which remain P4.
//! The public surface beyond configuration is exactly
//! `start`/`health`/`shutdown`; prompt remains crate-private.

use std::fmt;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use artisan_domain::RunId;
use artisan_transport::CancelHandle;
use thiserror::Error;
use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot, watch};

pub(crate) mod framing;
pub mod http;
pub(crate) mod observation;
mod operation;
mod process;
pub mod readiness;

#[cfg(test)]
#[path = "../../../../tests/backend/engine_owner_configuration.rs"]
mod engine_owner_configuration;

#[cfg(test)]
#[path = "../../../../tests/backend/engine_owner_custody.rs"]
mod engine_owner_custody;

#[cfg(test)]
#[path = "../../../../tests/backend/engine_owner_readiness.rs"]
mod engine_owner_readiness;

#[cfg(test)]
#[path = "../../../../tests/backend/engine_owner_streaming.rs"]
mod engine_owner_streaming;

use operation::{HealthState as OwnerHealth, Job, LaunchAdmissionError, run_owner};
use process::LaunchRecipe;

/// Raw engine time limits.
///
/// **UNVALIDATED** until accepted by [`EngineOwnerConfig::new`]. Callers may
/// construct this with struct literals; validation applies only through the
/// owned configuration constructor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EngineLimits {
    pub readiness: Duration,
    pub health: Duration,
    pub prompt: Duration,
    pub sse: Duration,
    pub close: Duration,
}

/// Raw engine bound limits.
///
/// **UNVALIDATED** until accepted by [`EngineOwnerConfig::new`]. Callers may
/// construct this with struct literals; every field is a byte/count/capacity
/// value and validation applies only through the owned configuration
/// constructor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EngineBounds {
    pub max_json_body: usize,
    pub max_sse_line: usize,
    pub max_sse_event: usize,
    pub max_readiness_line: usize,
    pub max_headers: usize,
    pub max_buf_bytes: usize,
    pub stderr_cap_bytes: usize,
    pub sink_capacity: usize,
    pub control_capacity: usize,
}

/// Typed validation failure for [`EngineOwnerConfig::new`].
///
/// `Debug` and `Display` never leak the caller-supplied executable path or
/// other arbitrary input payloads; only constant field names/kinds are
/// reported.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum EngineOwnerConfigError {
    #[error("engine executable path must be absolute")]
    InvalidExecutable,

    #[error("duration for {field} is not representable as a future instant")]
    UnrepresentableDuration { field: &'static str },

    #[error("bound {field} must be greater than zero")]
    ZeroBound { field: &'static str },

    #[error("http read buffer must be at least 8192 bytes")]
    BufferTooSmall,

    #[error("capacity for {field} exceeds semaphore limit")]
    CapacityTooLarge { field: &'static str },
}

fn check_duration(
    reference: Instant,
    duration: Duration,
    field: &'static str,
) -> Result<(), EngineOwnerConfigError> {
    if reference.checked_add(duration).is_none() {
        return Err(EngineOwnerConfigError::UnrepresentableDuration { field });
    }
    Ok(())
}

fn check_bound(value: usize, field: &'static str) -> Result<(), EngineOwnerConfigError> {
    if value == 0 {
        return Err(EngineOwnerConfigError::ZeroBound { field });
    }
    Ok(())
}

fn check_capacity(value: usize, field: &'static str) -> Result<(), EngineOwnerConfigError> {
    if value > tokio::sync::Semaphore::MAX_PERMITS {
        return Err(EngineOwnerConfigError::CapacityTooLarge { field });
    }
    Ok(())
}

/// Validated immutable engine-owner configuration.
///
/// Fields are private and only immutably observable; no mutable accessor,
/// `Default`, hidden budget, or input rewriting is provided.
pub struct EngineOwnerConfig {
    engine_executable: PathBuf,
    limits: EngineLimits,
    bounds: EngineBounds,
}

impl EngineOwnerConfig {
    /// Validates and constructs a new configuration.
    ///
    /// * `engine_executable` must be absolute as judged by
    ///   [`Path::is_absolute`] only; empty and relative paths are rejected and
    ///   the supplied bytes are otherwise preserved without filesystem probing.
    /// * Every [`EngineLimits`] duration must be representable as a future
    ///   `Instant` via one shared `Instant::now().checked_add(duration)`.
    ///   `Duration::ZERO` is allowed; `Duration::MAX` is rejected.
    /// * Every [`EngineBounds`] field must be `> 0` independently.
    ///   `max_buf_bytes` is an HTTP read-buffer in bytes and must be `>= 8192`,
    ///   the library minimum. `max_headers` is a header count. No hidden
    ///   cross-field ceilings are applied.
    /// * `sink_capacity` and `control_capacity` must each be
    ///   `<= tokio::sync::Semaphore::MAX_PERMITS`, checked numerically without
    ///   allocating a semaphore or channel. The `+1` boundary remains
    ///   representable because `MAX_PERMITS == usize::MAX >> 3`.
    ///
    /// Validation is atomic: on failure nothing is partially accepted.
    ///
    /// # Errors
    ///
    /// Returns [`EngineOwnerConfigError::InvalidExecutable`] when
    /// `engine_executable` is not a nonempty absolute path as judged by
    /// [`Path::is_absolute`].
    /// Returns [`EngineOwnerConfigError::UnrepresentableDuration`] when any
    /// configured duration cannot be represented as a future instant from the
    /// single shared `Instant::now()` reference.
    /// Returns [`EngineOwnerConfigError::ZeroBound`] when any bound required
    /// to be positive is zero.
    /// Returns [`EngineOwnerConfigError::BufferTooSmall`] when
    /// `max_buf_bytes` is below `8192`.
    /// Returns [`EngineOwnerConfigError::CapacityTooLarge`] when
    /// `sink_capacity` or `control_capacity` exceeds
    /// `tokio::sync::Semaphore::MAX_PERMITS`.
    pub fn new(
        engine_executable: PathBuf,
        limits: EngineLimits,
        bounds: EngineBounds,
    ) -> Result<Self, EngineOwnerConfigError> {
        if !engine_executable.is_absolute() {
            return Err(EngineOwnerConfigError::InvalidExecutable);
        }
        let reference = Instant::now();
        check_duration(reference, limits.readiness, "readiness")?;
        check_duration(reference, limits.health, "health")?;
        check_duration(reference, limits.prompt, "prompt")?;
        check_duration(reference, limits.sse, "sse")?;
        check_duration(reference, limits.close, "close")?;
        check_bound(bounds.max_json_body, "max_json_body")?;
        check_bound(bounds.max_sse_line, "max_sse_line")?;
        check_bound(bounds.max_sse_event, "max_sse_event")?;
        check_bound(bounds.max_readiness_line, "max_readiness_line")?;
        check_bound(bounds.max_headers, "max_headers")?;
        check_bound(bounds.max_buf_bytes, "max_buf_bytes")?;
        check_bound(bounds.stderr_cap_bytes, "stderr_cap_bytes")?;
        check_bound(bounds.sink_capacity, "sink_capacity")?;
        check_bound(bounds.control_capacity, "control_capacity")?;
        if bounds.max_buf_bytes < 8192 {
            return Err(EngineOwnerConfigError::BufferTooSmall);
        }
        check_capacity(bounds.sink_capacity, "sink_capacity")?;
        check_capacity(bounds.control_capacity, "control_capacity")?;
        Ok(Self {
            engine_executable,
            limits,
            bounds,
        })
    }

    /// Returns the caller-supplied absolute executable path bytes (no
    /// canonicalization or existence check).
    #[must_use]
    pub fn engine_executable(&self) -> &Path {
        &self.engine_executable
    }

    /// Returns the validated limits snapshot.
    #[must_use]
    pub fn limits(&self) -> &EngineLimits {
        &self.limits
    }

    /// Returns the validated bounds snapshot.
    #[must_use]
    pub fn bounds(&self) -> &EngineBounds {
        &self.bounds
    }
}

impl fmt::Debug for EngineOwnerConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EngineOwnerConfig")
            .field("engine_executable", &"<redacted>")
            .field("limits", &self.limits)
            .field("bounds", &self.bounds)
            .finish()
    }
}

/// Payload-free health of one engine owner instance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EngineOwnerHealth {
    /// Admission is open; the owner task is serving work.
    Active,
    /// The owner irreversibly stopped serving new work.
    Quarantined,
}

/// Report of one shutdown attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EngineOwnerShutdown {
    /// The owner task was observed to complete after draining its queue.
    Joined,
    /// The owner task ended without a normal completion (panic).
    TaskLost,
    /// The owner had quarantined itself; the report is deliberately
    /// incomplete, the facade and its stored join handle are not consumed,
    /// and a later call may observe eventual completion honestly.
    Quarantined,
}

/// The one parent-side owner of the external engine process.
///
/// See the module documentation for the ownership contract. Instances are
/// neither `Clone` nor `Copy`.
pub struct EngineOwner {
    jobs: mpsc::Sender<Job>,
    shutdown: Arc<CancelHandle>,
    health: watch::Receiver<OwnerHealth>,
    join: tokio::task::JoinHandle<()>,
    /// Cached completion verdict for the owner task. A completed Tokio
    /// `JoinHandle` consumes its output on the delivering poll and panics if
    /// polled again, so the observation is recorded exactly once and replayed
    /// by later calls.
    observed_join: Option<bool>,
}

impl fmt::Debug for EngineOwner {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("EngineOwner { <payload-free> }")
    }
}

impl EngineOwner {
    /// Starts the single owner task on the supplied runtime handle.
    ///
    /// Creates the bounded job channel with capacity
    /// `config.bounds().control_capacity`, the shutdown [`CancelHandle`], the
    /// health watch channel, and spawns only the owner task on the caller's
    /// handle. No child is spawned here, no probe is performed, and no extra
    /// runtime is created. The config is already validated, so this is
    /// infallible.
    #[must_use]
    pub fn start(config: EngineOwnerConfig, runtime: &Handle) -> Self {
        let EngineOwnerConfig {
            engine_executable,
            limits,
            bounds,
        } = config;
        let capacity = bounds.control_capacity;
        let recipe = LaunchRecipe::Production {
            executable: engine_executable,
        };
        let (jobs, pending) = mpsc::channel::<Job>(capacity);
        let shutdown = Arc::new(CancelHandle::new());
        let (health_sender, health) = watch::channel(OwnerHealth::Active);
        let join = runtime.spawn(run_owner(
            pending,
            Arc::clone(&shutdown),
            health_sender,
            recipe,
            limits,
            bounds,
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
    pub fn health(&self) -> EngineOwnerHealth {
        match *self.health.borrow() {
            OwnerHealth::Active => EngineOwnerHealth::Active,
            OwnerHealth::Quarantined => EngineOwnerHealth::Quarantined,
        }
    }

    /// Shuts the owner down and reports what is actually observed.
    ///
    /// This is deliberately NOT an async method: the shutdown signal is
    /// raised here, before the returned future exists, so admission stops
    /// and orderly teardown starts even if that future is never polled.
    ///
    /// The returned future retains both wake sources across `Pending` polls —
    /// the owned health-watch `changed()` waiter and the owner `JoinHandle` —
    /// so a quarantine arriving while the join is parked wakes it
    /// immediately. Completion wins when both sources settle together, and
    /// the consumed join verdict is cached exactly once, so repeated calls
    /// honestly replay the observed ending. On a quarantined observation the
    /// report is explicitly incomplete: the facade and its join handle are
    /// untouched and later calls may still observe eventual completion.
    pub fn shutdown(&mut self) -> impl Future<Output = EngineOwnerShutdown> + '_ {
        self.shutdown.cancel();
        let read_rx = self.health.clone();
        let mut wait_rx = self.health.clone();
        async move {
            let mut changed = Box::pin(wait_rx.changed());
            loop {
                if let Some(joined_cleanly) = self.observed_join {
                    return if joined_cleanly {
                        EngineOwnerShutdown::Joined
                    } else {
                        EngineOwnerShutdown::TaskLost
                    };
                }
                tokio::select! {
                    biased;

                    joined = &mut self.join => {
                        let joined_cleanly = joined.is_ok();
                        self.observed_join = Some(joined_cleanly);
                        return if joined_cleanly {
                            EngineOwnerShutdown::Joined
                        } else {
                            EngineOwnerShutdown::TaskLost
                        };
                    }
                    _ = &mut changed => {
                        drop(changed);
                        if *read_rx.borrow() == OwnerHealth::Quarantined {
                            return EngineOwnerShutdown::Quarantined;
                        }
                        changed = Box::pin(wait_rx.changed());
                    }
                }
            }
        }
    }

    /// Crate-private admission: controls installed before `try_send`.
    ///
    /// Health and shutdown are checked first; deadline computed once via
    /// `checked_add`; `try_send` full yields `Busy`, closed or quarantined
    /// yields `Unavailable`. No polling loops.
    pub(crate) fn admit(
        &self,
        run_id: RunId,
        budget: Duration,
    ) -> Result<operation::AcceptedLaunch, LaunchAdmissionError> {
        if *self.health.borrow() != OwnerHealth::Active || self.shutdown.is_cancelled() {
            return Err(LaunchAdmissionError::Unavailable);
        }
        if budget == Duration::ZERO {
            return Err(LaunchAdmissionError::InvalidDeadline);
        }
        let Some(deadline) = tokio::time::Instant::now().checked_add(budget) else {
            return Err(LaunchAdmissionError::InvalidDeadline);
        };
        let control = Arc::new(CancelHandle::new());
        let (respond, receiver) = oneshot::channel();
        let job = Job {
            run_id,
            deadline,
            control: Arc::clone(&control),
            respond,
        };
        match self.jobs.try_send(job) {
            Ok(()) => Ok(operation::AcceptedLaunch::from_parts(receiver, control)),
            Err(mpsc::error::TrySendError::Full(_)) => Err(LaunchAdmissionError::Busy),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(LaunchAdmissionError::Unavailable),
        }
    }
}

impl Drop for EngineOwner {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

#[cfg(test)]
impl EngineOwner {
    /// Aborts the owner task for `TaskLost` coverage (test-only).
    pub(crate) fn abort_for_tests(&self) {
        self.join.abort();
    }

    /// Injects a job with an already-expired deadline for deterministic
    /// `Deadline` coverage without wall-clock races (test-only).
    pub(crate) fn inject_expired_for_tests(&self, run_id: RunId) -> operation::AcceptedLaunch {
        let control = Arc::new(CancelHandle::new());
        let (respond, receiver) = oneshot::channel();
        let job = Job {
            run_id,
            deadline: tokio::time::Instant::now() - Duration::from_secs(1),
            control: Arc::clone(&control),
            respond,
        };
        let _ = self.jobs.try_send(job);
        operation::AcceptedLaunch::from_parts(receiver, control)
    }
}

/// Re-exports powering the private test module.
#[cfg(test)]
pub(crate) use operation::GenerationAllocator;
#[cfg(test)]
pub(crate) use process::{reset_witnesses, witness_counts};

#[cfg(test)]
pub(crate) use http::HealthSecret;
#[cfg(test)]
pub(crate) use readiness::ReadinessError;

#[cfg(test)]
pub(crate) fn start_with_exhausted_allocator_for_tests(
    config: EngineOwnerConfig,
    runtime: &Handle,
) -> EngineOwner {
    let EngineOwnerConfig {
        engine_executable,
        limits,
        bounds,
    } = config;
    let capacity = bounds.control_capacity;
    let recipe = LaunchRecipe::Production {
        executable: engine_executable,
    };
    let (jobs, pending) = mpsc::channel::<Job>(capacity);
    let shutdown = Arc::new(CancelHandle::new());
    let (health_sender, health) = watch::channel(OwnerHealth::Active);
    let mut allocator = operation::GenerationAllocator::new();
    allocator.force_next(u64::MAX);
    let _ = allocator.mint();
    let join = runtime.spawn(operation::run_owner_with_allocator(
        pending,
        Arc::clone(&shutdown),
        health_sender,
        recipe,
        limits,
        bounds,
        allocator,
    ));
    EngineOwner {
        jobs,
        shutdown,
        health,
        join,
        observed_join: None,
    }
}

#[cfg(test)]
pub(crate) fn start_with_fixture_for_tests(
    limits: EngineLimits,
    bounds: EngineBounds,
    runtime: &Handle,
    fixture_program: std::path::PathBuf,
    scenario: &'static str,
) -> EngineOwner {
    let capacity = bounds.control_capacity;
    let recipe = LaunchRecipe::Fixture {
        program: fixture_program,
        args: Vec::new(),
        scenario,
    };
    let (jobs, pending) = mpsc::channel::<Job>(capacity);
    let shutdown = Arc::new(CancelHandle::new());
    let (health_sender, health) = watch::channel(OwnerHealth::Active);
    let join = runtime.spawn(operation::run_owner(
        pending,
        Arc::clone(&shutdown),
        health_sender,
        recipe,
        limits,
        bounds,
    ));
    EngineOwner {
        jobs,
        shutdown,
        health,
        join,
        observed_join: None,
    }
}
