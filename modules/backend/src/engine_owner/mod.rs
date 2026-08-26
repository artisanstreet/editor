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

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use thiserror::Error;

#[cfg(test)]
#[path = "../../../../tests/backend/engine_owner_configuration.rs"]
mod engine_owner_configuration;

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
