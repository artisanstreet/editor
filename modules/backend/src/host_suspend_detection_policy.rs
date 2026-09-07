//! Dependency-free host-suspend detection policy.
//!
//! The host runtime owns wall-clock reads, monotonic sleeping, scheduling, and
//! publication. This module only resolves the monitor's options and evaluates
//! one pair of caller-supplied wall-clock observations. It does not read a
//! clock, sleep, log, allocate a channel, spawn work, or publish an event.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use std::{fmt, num::NonZeroU64};

/// Default spacing between host-suspend observations, in milliseconds.
pub const DEFAULT_HEARTBEAT_MS: u64 = 10_000;

/// Default excess wall-clock duration treated as a host suspend, in
/// milliseconds.
pub const DEFAULT_MINIMUM_GAP_MS: u64 = 30_000;

/// Optional host-suspend settings supplied by an embedding runtime.
///
/// `None` has the same nullish-default behavior as an omitted TypeScript
/// option. A supplied zero heartbeat is retained through default resolution
/// and rejected by validation rather than being turned into a busy loop.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HostSuspendDetectionOptions {
    /// Expected spacing between observations, in milliseconds.
    pub heartbeat_ms: Option<u64>,
    /// Smallest excess wall-clock duration considered a suspend, in
    /// milliseconds.
    pub minimum_gap_ms: Option<u64>,
}

impl HostSuspendDetectionOptions {
    /// Applies production defaults and validates the resulting settings.
    ///
    /// A zero minimum gap is valid: it authorizes every nonnegative excess
    /// duration. The heartbeat must be positive because a zero heartbeat
    /// would make an outer monitor loop spin without yielding.
    ///
    /// # Errors
    ///
    /// Returns [`HostSuspendDetectionConfigError::ZeroHeartbeat`] when the
    /// supplied or defaulted heartbeat is zero.
    pub const fn resolve(
        self,
    ) -> Result<HostSuspendDetectionConfig, HostSuspendDetectionConfigError> {
        let heartbeat_ms = match self.heartbeat_ms {
            Some(value) => value,
            None => DEFAULT_HEARTBEAT_MS,
        };
        let minimum_gap_ms = match self.minimum_gap_ms {
            Some(value) => value,
            None => DEFAULT_MINIMUM_GAP_MS,
        };

        HostSuspendDetectionConfig::new(heartbeat_ms, minimum_gap_ms)
    }
}

/// Why host-suspend settings could not be validated.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum HostSuspendDetectionConfigError {
    /// A zero heartbeat would not yield between observations.
    ZeroHeartbeat,
}

impl fmt::Display for HostSuspendDetectionConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroHeartbeat => {
                formatter.write_str("host-suspend heartbeat must be greater than zero")
            }
        }
    }
}

impl std::error::Error for HostSuspendDetectionConfigError {}

/// Validated host-suspend settings.
///
/// The heartbeat is represented as [`NonZeroU64`] so a valid policy cannot
/// accidentally become a busy loop. No sum of the two settings is formed;
/// maximum-sized values therefore remain valid without configuration-time
/// overflow.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct HostSuspendDetectionConfig {
    heartbeat_ms: NonZeroU64,
    minimum_gap_ms: u64,
}

impl HostSuspendDetectionConfig {
    /// Validates and retains host-suspend settings.
    ///
    /// Every nonzero heartbeat and every `u64` minimum gap is valid. In
    /// particular, the minimum gap may be zero and the two values are never
    /// added together.
    ///
    /// # Errors
    ///
    /// Returns [`HostSuspendDetectionConfigError::ZeroHeartbeat`] for a zero
    /// heartbeat.
    pub const fn new(
        heartbeat_ms: u64,
        minimum_gap_ms: u64,
    ) -> Result<Self, HostSuspendDetectionConfigError> {
        let Some(heartbeat_ms) = NonZeroU64::new(heartbeat_ms) else {
            return Err(HostSuspendDetectionConfigError::ZeroHeartbeat);
        };

        Ok(Self {
            heartbeat_ms,
            minimum_gap_ms,
        })
    }

    /// Returns the validated heartbeat spacing, in milliseconds.
    #[must_use]
    pub const fn heartbeat_ms(self) -> u64 {
        self.heartbeat_ms.get()
    }

    /// Returns the validated minimum excess gap, in milliseconds.
    #[must_use]
    pub const fn minimum_gap_ms(self) -> u64 {
        self.minimum_gap_ms
    }
}

impl Default for HostSuspendDetectionConfig {
    fn default() -> Self {
        Self::new(DEFAULT_HEARTBEAT_MS, DEFAULT_MINIMUM_GAP_MS)
            .expect("production host-suspend defaults must validate")
    }
}

impl TryFrom<HostSuspendDetectionOptions> for HostSuspendDetectionConfig {
    type Error = HostSuspendDetectionConfigError;

    fn try_from(options: HostSuspendDetectionOptions) -> Result<Self, Self::Error> {
        options.resolve()
    }
}

/// One exact resume observation that passed the configured suspend threshold.
///
/// Both fields are copied from the pure decision: `resumed_at_ms` is the
/// second wall-clock observation and `suspended_ms` is exactly
/// `resumed_at_ms - started_at_ms - heartbeat_ms`. The host that owns event
/// publication may carry this value onward without recomputing either field.
#[must_use]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct HostResumeObservation {
    /// Exact wall-clock value from the second observation, in epoch
    /// milliseconds.
    pub resumed_at_ms: i64,
    /// Exact computed wall-clock duration beyond the expected heartbeat, in
    /// milliseconds.
    pub suspended_ms: i128,
}

impl HostResumeObservation {
    /// Creates an observation while preserving both supplied values exactly.
    pub const fn new(resumed_at_ms: i64, suspended_ms: i128) -> Self {
        Self {
            resumed_at_ms,
            suspended_ms,
        }
    }

    /// Returns the exact resumed wall-clock instant.
    #[must_use]
    pub const fn resumed_at_ms(self) -> i64 {
        self.resumed_at_ms
    }

    /// Returns the exact computed suspended duration.
    #[must_use]
    pub const fn suspended_ms(self) -> i128 {
        self.suspended_ms
    }
}

/// Alias matching the legacy monitor's exported resume value name.
pub type HostResume = HostResumeObservation;

/// Result of evaluating one pair of wall-clock observations.
///
/// The policy returns exactly one decision for each call. Only [`Self::Resume`]
/// contains a publishable resume observation. A backward clock is kept
/// separate from ordinary drift, and checked arithmetic has an explicit
/// defensive failure outcome so neither case can look like a suspend.
#[must_use = "a host-suspend decision must be handled by the caller"]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum HostSuspendDetectionDecision {
    /// The computed duration reached the inclusive configured threshold.
    Resume(HostResumeObservation),
    /// The computed duration stayed below the configured threshold.
    NoResume {
        /// Exact wall-clock value from the second observation.
        resumed_at_ms: i64,
        /// Exact computed duration, which may be negative for an early wake.
        suspended_ms: i128,
    },
    /// The wall clock moved backwards between observations.
    ClockMovedBackward {
        /// Exact wall-clock value from the first observation.
        started_at_ms: i64,
        /// Exact wall-clock value from the second observation.
        resumed_at_ms: i64,
    },
    /// A checked wide arithmetic operation could not be represented.
    ///
    /// Current public inputs (`i64` instants and `u64` durations) fit in the
    /// `i128` intermediate, so this is defensive against a future type-range
    /// change rather than an expected result today.
    ArithmeticOverflow,
}

impl HostSuspendDetectionDecision {
    /// Returns whether this observation produced a publishable resume.
    #[must_use]
    pub const fn is_resume(self) -> bool {
        matches!(self, Self::Resume(_))
    }

    /// Returns whether this observation authorizes a host-resume publication.
    #[must_use]
    pub const fn is_suspend_detected(self) -> bool {
        self.is_resume()
    }

    /// Returns the exact resume observation, if the threshold was reached.
    #[must_use]
    pub const fn resume_observation(self) -> Option<HostResumeObservation> {
        match self {
            Self::Resume(observation) => Some(observation),
            Self::NoResume { .. } | Self::ClockMovedBackward { .. } | Self::ArithmeticOverflow => {
                None
            }
        }
    }

    /// Returns the exact computed duration for a non-regressing arithmetic
    /// outcome. Backward clocks and checked arithmetic failures have none.
    #[must_use]
    pub const fn suspended_ms(self) -> Option<i128> {
        match self {
            Self::Resume(observation) => Some(observation.suspended_ms),
            Self::NoResume { suspended_ms, .. } => Some(suspended_ms),
            Self::ClockMovedBackward { .. } | Self::ArithmeticOverflow => None,
        }
    }
}

/// Stateless evaluator for the host-suspend detection policy.
///
/// The policy owns only validated configuration. Every call to [`Self::observe`]
/// is independent; timers, clocks, and publication remain with the caller.
#[must_use]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct HostSuspendDetectionPolicy {
    config: HostSuspendDetectionConfig,
}

impl HostSuspendDetectionPolicy {
    /// Builds a policy from optional settings after applying production
    /// defaults.
    ///
    /// # Errors
    ///
    /// Returns [`HostSuspendDetectionConfigError::ZeroHeartbeat`] when the
    /// supplied heartbeat is zero.
    pub fn new(
        options: HostSuspendDetectionOptions,
    ) -> Result<Self, HostSuspendDetectionConfigError> {
        options.resolve().map(Self::from_config)
    }

    /// Builds a policy from settings that have already been validated.
    pub const fn from_config(config: HostSuspendDetectionConfig) -> Self {
        Self { config }
    }

    /// Returns the validated settings used by this policy.
    #[must_use]
    pub const fn config(self) -> HostSuspendDetectionConfig {
        self.config
    }

    /// Evaluates one start/resume pair and returns exactly one decision.
    ///
    /// Wall-clock values are signed epoch milliseconds. A resumed value below
    /// its start is reported as [`HostSuspendDetectionDecision::ClockMovedBackward`].
    /// Otherwise the subtraction is widened to `i128` before evaluating the
    /// exact formula `resumed - started - heartbeat`; a negative result is
    /// ordinary early-wake drift, never a wrapped positive suspend.
    #[must_use = "the host-suspend decision must be handled by the caller"]
    pub fn observe(&self, started_at_ms: i64, resumed_at_ms: i64) -> HostSuspendDetectionDecision {
        host_suspend_detection_decision_with_config(started_at_ms, resumed_at_ms, self.config)
    }
}

impl Default for HostSuspendDetectionPolicy {
    fn default() -> Self {
        Self::from_config(HostSuspendDetectionConfig::default())
    }
}

impl TryFrom<HostSuspendDetectionOptions> for HostSuspendDetectionPolicy {
    type Error = HostSuspendDetectionConfigError;

    fn try_from(options: HostSuspendDetectionOptions) -> Result<Self, Self::Error> {
        Self::new(options)
    }
}

/// Applies one validated configuration to one pair of wall-clock observations.
///
/// This function performs no clock read, sleep, task scheduling, or
/// publication. It exists for callers that validate and retain configuration
/// separately from the observation boundary.
#[must_use = "the host-suspend decision must be handled by the caller"]
pub fn host_suspend_detection_decision_with_config(
    started_at_ms: i64,
    resumed_at_ms: i64,
    config: HostSuspendDetectionConfig,
) -> HostSuspendDetectionDecision {
    if resumed_at_ms < started_at_ms {
        return HostSuspendDetectionDecision::ClockMovedBackward {
            started_at_ms,
            resumed_at_ms,
        };
    }

    let Some(elapsed_ms) = i128::from(resumed_at_ms).checked_sub(i128::from(started_at_ms)) else {
        return HostSuspendDetectionDecision::ArithmeticOverflow;
    };
    let Some(suspended_ms) = elapsed_ms.checked_sub(i128::from(config.heartbeat_ms())) else {
        return HostSuspendDetectionDecision::ArithmeticOverflow;
    };
    let minimum_gap_ms = i128::from(config.minimum_gap_ms());

    if suspended_ms >= minimum_gap_ms {
        HostSuspendDetectionDecision::Resume(HostResumeObservation::new(
            resumed_at_ms,
            suspended_ms,
        ))
    } else {
        HostSuspendDetectionDecision::NoResume {
            resumed_at_ms,
            suspended_ms,
        }
    }
}

/// Resolves optional settings and evaluates one pair of wall-clock
/// observations.
///
/// # Errors
///
/// Returns [`HostSuspendDetectionConfigError::ZeroHeartbeat`] when the
/// supplied heartbeat is zero.
#[must_use = "the host-suspend decision must be handled by the caller"]
pub fn host_suspend_detection_decision(
    started_at_ms: i64,
    resumed_at_ms: i64,
    options: HostSuspendDetectionOptions,
) -> Result<HostSuspendDetectionDecision, HostSuspendDetectionConfigError> {
    let config = options.resolve()?;
    Ok(host_suspend_detection_decision_with_config(
        started_at_ms,
        resumed_at_ms,
        config,
    ))
}
