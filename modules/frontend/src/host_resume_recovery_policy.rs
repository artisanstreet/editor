//! Pure host-resume recovery decisions.
//!
//! This is the dependency-free value boundary of
//! `modules/frontend/src/lib/runtime/host-resume-recovery.ts`. The host owns
//! the wall-clock reads and the monotonic sleep; this module compares the two
//! observations and returns one decision. It does not read a clock, schedule
//! a heartbeat, call a client, or retry a connection.

#![allow(clippy::module_name_repetitions)]
#![forbid(unsafe_code)]

use std::fmt;
use std::num::NonZeroU64;

/// Default spacing between host-resume checks, in milliseconds.
pub const DEFAULT_HEARTBEAT_MS: u64 = 10_000;

/// Default excess wall-clock gap treated as a host suspend, in milliseconds.
pub const DEFAULT_MINIMUM_GAP_MS: u64 = 30_000;

/// Optional host-resume settings supplied by an embedding host.
///
/// `None` has the same defaulting behavior as an omitted TypeScript option.
/// A supplied zero heartbeat is retained until validation and is rejected
/// rather than becoming a busy loop.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HostResumeRecoveryOptions {
    /// Expected spacing between observations, in milliseconds.
    pub heartbeat_ms: Option<u64>,
    /// Smallest excess gap considered a suspend, in milliseconds.
    pub minimum_gap_ms: Option<u64>,
}

impl HostResumeRecoveryOptions {
    /// Applies production defaults and validates the resulting settings.
    ///
    /// The minimum gap may be zero, which is useful for a caller that wants
    /// every positive excess interval to authorize recovery. The heartbeat
    /// must remain positive because a zero value would make an outer monitor
    /// loop spin without yielding.
    ///
    /// # Errors
    ///
    /// Returns [`HostResumeRecoveryConfigError::ZeroHeartbeat`] when the
    /// supplied heartbeat resolves to zero.
    pub const fn resolve(self) -> Result<HostResumeRecoveryConfig, HostResumeRecoveryConfigError> {
        let heartbeat_ms = match self.heartbeat_ms {
            Some(value) => value,
            None => DEFAULT_HEARTBEAT_MS,
        };
        let minimum_gap_ms = match self.minimum_gap_ms {
            Some(value) => value,
            None => DEFAULT_MINIMUM_GAP_MS,
        };

        HostResumeRecoveryConfig::new(heartbeat_ms, minimum_gap_ms)
    }
}

/// Why host-resume settings could not be validated.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum HostResumeRecoveryConfigError {
    /// A zero heartbeat would not yield between observations.
    ZeroHeartbeat,
}

impl fmt::Display for HostResumeRecoveryConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroHeartbeat => {
                formatter.write_str("host-resume heartbeat must be greater than zero")
            }
        }
    }
}

impl std::error::Error for HostResumeRecoveryConfigError {}

/// Validated host-resume settings.
///
/// The nonzero heartbeat representation makes it impossible for a valid
/// policy to accidentally configure a busy loop. Both settings are retained
/// as `u64`; the decision arithmetic uses saturating subtraction, so even
/// maximum-sized caller values cannot wrap.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct HostResumeRecoveryConfig {
    heartbeat_ms: NonZeroU64,
    minimum_gap_ms: u64,
}

impl HostResumeRecoveryConfig {
    /// Validates and retains host-resume settings.
    ///
    /// Every nonzero heartbeat and every `u64` minimum gap is valid. In
    /// particular, no `heartbeat + minimum_gap` is formed, so large values do
    /// not overflow while configuring a policy.
    ///
    /// # Errors
    ///
    /// Returns [`HostResumeRecoveryConfigError::ZeroHeartbeat`] for a zero
    /// heartbeat.
    pub const fn new(
        heartbeat_ms: u64,
        minimum_gap_ms: u64,
    ) -> Result<Self, HostResumeRecoveryConfigError> {
        let Some(heartbeat_ms) = NonZeroU64::new(heartbeat_ms) else {
            return Err(HostResumeRecoveryConfigError::ZeroHeartbeat);
        };

        Ok(Self {
            heartbeat_ms,
            minimum_gap_ms,
        })
    }

    /// Returns the validated heartbeat spacing in milliseconds.
    #[must_use]
    pub const fn heartbeat_ms(self) -> u64 {
        self.heartbeat_ms.get()
    }

    /// Returns the minimum excess gap in milliseconds.
    #[must_use]
    pub const fn minimum_gap_ms(self) -> u64 {
        self.minimum_gap_ms
    }
}

impl Default for HostResumeRecoveryConfig {
    fn default() -> Self {
        Self::new(DEFAULT_HEARTBEAT_MS, DEFAULT_MINIMUM_GAP_MS)
            .expect("production host-resume defaults must validate")
    }
}

impl TryFrom<HostResumeRecoveryOptions> for HostResumeRecoveryConfig {
    type Error = HostResumeRecoveryConfigError;

    fn try_from(options: HostResumeRecoveryOptions) -> Result<Self, Self::Error> {
        options.resolve()
    }
}

/// The one recovery decision returned for one pair of clock observations.
///
/// `Reconnect` is an intent for the caller to invoke its existing transport
/// capability once. This enum does not perform that invocation or own a
/// retry loop. `ClockMovedBackward` is kept distinct from ordinary drift so a
/// caller can diagnose a wall-clock regression without treating it as a
/// resume.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum HostResumeRecoveryDecision {
    /// The excess wall-clock gap reached the inclusive configured threshold.
    Reconnect {
        /// Wall-clock value from the second observation.
        resumed_at_ms: i64,
        /// Wall-clock time beyond the expected heartbeat.
        excess_gap_ms: u64,
    },
    /// The observation did not reach the configured recovery threshold.
    NoReconnect {
        /// Wall-clock value from the second observation.
        resumed_at_ms: i64,
        /// Saturated excess wall-clock time beyond the expected heartbeat.
        excess_gap_ms: u64,
    },
    /// The second wall-clock observation precedes the first.
    ClockMovedBackward {
        /// Wall-clock value from the first observation.
        started_at_ms: i64,
        /// Wall-clock value from the second observation.
        resumed_at_ms: i64,
    },
}

impl HostResumeRecoveryDecision {
    /// Returns whether the caller may issue one reconnect action.
    #[must_use]
    pub const fn is_reconnect(self) -> bool {
        matches!(self, Self::Reconnect { .. })
    }

    /// Returns whether this observation authorizes recovery.
    #[must_use]
    pub const fn is_recovery_authorized(self) -> bool {
        self.is_reconnect()
    }

    /// Returns the nonnegative excess gap for a non-regressing observation.
    #[must_use]
    pub const fn excess_gap_ms(self) -> Option<u64> {
        match self {
            Self::Reconnect { excess_gap_ms, .. } | Self::NoReconnect { excess_gap_ms, .. } => {
                Some(excess_gap_ms)
            }
            Self::ClockMovedBackward { .. } => None,
        }
    }
}

/// Stateless policy that validates settings and emits one decision per input.
///
/// A policy has no timer, task, retry state, or transport handle. Repeated
/// calls to [`Self::observe`] are independent observations; each call returns
/// exactly one decision for its supplied timestamps.
#[must_use]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct HostResumeRecoveryPolicy {
    config: HostResumeRecoveryConfig,
}

impl HostResumeRecoveryPolicy {
    /// Builds a policy from optional caller settings and production defaults.
    ///
    /// # Errors
    ///
    /// Returns [`HostResumeRecoveryConfigError::ZeroHeartbeat`] when the
    /// supplied heartbeat is zero.
    pub fn new(options: HostResumeRecoveryOptions) -> Result<Self, HostResumeRecoveryConfigError> {
        options.resolve().map(Self::from_config)
    }

    /// Builds a policy from settings that have already been validated.
    pub const fn from_config(config: HostResumeRecoveryConfig) -> Self {
        Self { config }
    }

    /// Returns the validated settings used by this policy.
    #[must_use]
    pub const fn config(self) -> HostResumeRecoveryConfig {
        self.config
    }

    /// Observes one start/resume pair and returns one recovery decision.
    ///
    /// Wall-clock values are signed epoch milliseconds, matching the native
    /// timestamp boundary. A non-regressing pair is widened before its
    /// subtraction, then the expected heartbeat is removed with saturating
    /// arithmetic. Thus early wakeups and ordinary scheduler drift cannot
    /// become a positive gap, and extreme timestamps cannot wrap.
    #[must_use]
    pub fn observe(&self, started_at_ms: i64, resumed_at_ms: i64) -> HostResumeRecoveryDecision {
        host_resume_recovery_decision_with_config(started_at_ms, resumed_at_ms, self.config)
    }
}

impl Default for HostResumeRecoveryPolicy {
    fn default() -> Self {
        Self::from_config(HostResumeRecoveryConfig::default())
    }
}

impl TryFrom<HostResumeRecoveryOptions> for HostResumeRecoveryPolicy {
    type Error = HostResumeRecoveryConfigError;

    fn try_from(options: HostResumeRecoveryOptions) -> Result<Self, Self::Error> {
        Self::new(options)
    }
}

/// Applies one already-validated host-resume configuration to one observation.
///
/// No clock, sleep, transport, or retry behavior is involved. A backward
/// wall-clock movement returns [`HostResumeRecoveryDecision::ClockMovedBackward`]
/// and never authorizes recovery.
#[must_use]
pub fn host_resume_recovery_decision_with_config(
    started_at_ms: i64,
    resumed_at_ms: i64,
    config: HostResumeRecoveryConfig,
) -> HostResumeRecoveryDecision {
    if resumed_at_ms < started_at_ms {
        return HostResumeRecoveryDecision::ClockMovedBackward {
            started_at_ms,
            resumed_at_ms,
        };
    }

    let elapsed_ms = elapsed_wall_time_ms(started_at_ms, resumed_at_ms);
    let heartbeat_ms = config.heartbeat_ms();
    let excess_gap_ms = elapsed_ms.saturating_sub(heartbeat_ms);

    if elapsed_ms >= heartbeat_ms && excess_gap_ms >= config.minimum_gap_ms() {
        HostResumeRecoveryDecision::Reconnect {
            resumed_at_ms,
            excess_gap_ms,
        }
    } else {
        HostResumeRecoveryDecision::NoReconnect {
            resumed_at_ms,
            excess_gap_ms,
        }
    }
}

/// Applies optional caller settings, returning validation failure explicitly.
///
/// This convenience function mirrors one iteration of the legacy monitor. It
/// returns one decision and then stops; the caller owns any later observation
/// and the transport's retry policy.
///
/// # Errors
///
/// Returns [`HostResumeRecoveryConfigError::ZeroHeartbeat`] when the supplied
/// heartbeat resolves to zero.
pub fn host_resume_recovery_decision(
    started_at_ms: i64,
    resumed_at_ms: i64,
    options: HostResumeRecoveryOptions,
) -> Result<HostResumeRecoveryDecision, HostResumeRecoveryConfigError> {
    let config = options.resolve()?;
    Ok(host_resume_recovery_decision_with_config(
        started_at_ms,
        resumed_at_ms,
        config,
    ))
}

/// Computes a nonnegative wall-clock span without signed timestamp overflow.
fn elapsed_wall_time_ms(started_at_ms: i64, resumed_at_ms: i64) -> u64 {
    let elapsed_ms = i128::from(resumed_at_ms).saturating_sub(i128::from(started_at_ms));
    u64::try_from(elapsed_ms).unwrap_or(u64::MAX)
}
