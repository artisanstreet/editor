//! Pure Forge recovery-health reachability policy.
//!
//! This is the synchronous decision boundary behind the TypeScript
//! `recovery-health-probe.ts` effect. It deliberately does not issue a
//! request, enforce a deadline, wait asynchronously, or schedule a retry.
//! Callers provide the one observation produced by those concerns, and this
//! module reports only whether that observation proves reachability.

use std::time::Duration;

/// Exact per-observation deadline used by the recovery-health policy.
///
/// The value is exposed as a typed duration for the eventual caller that
/// owns timing. This pure module does not apply the deadline itself.
pub const FORGE_RECOVERY_HEALTH_DEADLINE: Duration = Duration::from_millis(1_500);

/// One outcome observed by a Forge recovery-health probe.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProbeObservation {
    /// An HTTP response status received from the health endpoint.
    ///
    /// The signed, wide status type keeps this pure boundary total for
    /// unusual or synthetic values; ordinary HTTP status codes remain the
    /// only values that can establish reachability.
    HttpStatus {
        /// Status code returned by the observed response.
        status: i64,
    },
    /// No response was observed before the caller's deadline.
    Timeout,
    /// The request failed without a usable HTTP response status.
    TransportFailure,
}

impl ProbeObservation {
    /// Returns whether this observation proves Forge is reachable.
    ///
    /// Only an inclusive 2xx HTTP status establishes reachability. A timeout
    /// or transport failure is false; this decision makes no stronger claim
    /// about Forge health or the response beyond reachability.
    #[must_use]
    pub const fn is_reachable(self) -> bool {
        match self {
            Self::HttpStatus { status } => matches!(status, 200..=299),
            Self::Timeout | Self::TransportFailure => false,
        }
    }
}

/// Reduces one observed recovery-health outcome to its reachability fact.
///
/// This is the pure equivalent of the TypeScript probe's final boolean
/// mapping after request effects, timeout handling, and failure recovery have
/// been resolved by the caller.
#[must_use]
pub const fn probe_forge_recovery_health(observation: ProbeObservation) -> bool {
    observation.is_reachable()
}
