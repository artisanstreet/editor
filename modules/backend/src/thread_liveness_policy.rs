//! Dependency-free policy for admitting durable writes to a thread.
//!
//! The persistence boundary observes three independent facts: whether the
//! thread row exists, whether an erasure claim exists, and whether a tombstone
//! exists. This module only evaluates those observations. It does not query a
//! database, perform erasure, run asynchronous work, or cause persistence
//! side effects.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

/// The result of observing one liveness fact.
///
/// `Absent` is an observed negative result. It is deliberately distinct from
/// `Incomplete` and `Failed`, so missing or failed query evidence cannot be
/// mistaken for a successful query that found no row.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FactObservation {
    /// The queried row or marker exists.
    Present,
    /// The query completed and found no row or marker.
    Absent,
    /// The query did not produce complete evidence.
    Incomplete,
    /// The query failed and cannot establish the fact's value.
    Failed,
}

impl FactObservation {
    /// Converts a completed boolean query result into an observation.
    #[must_use]
    pub const fn observed(present: bool) -> Self {
        if present { Self::Present } else { Self::Absent }
    }

    /// Returns whether this fact has a completed, trustworthy value.
    #[must_use]
    pub const fn is_complete(self) -> bool {
        matches!(self, Self::Present | Self::Absent)
    }
}

/// Identifies one of the three independent facts used by the liveness rule.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThreadLivenessFact {
    /// Whether the durable thread row exists.
    ThreadRow,
    /// Whether a durable erasure claim exists.
    ErasureClaim,
    /// Whether a durable tombstone exists.
    Tombstone,
}

/// The complete set of independently observed liveness facts.
///
/// A thread is eligible for durable-write admission only when all three
/// fields are complete and have the values `Present`, `Absent`, and `Absent`
/// respectively. The fields remain separate so callers cannot accidentally
/// collapse a missing query result into an observed `Absent` value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ThreadLivenessObservation {
    /// Whether the thread row exists.
    pub thread_row: FactObservation,
    /// Whether an erasure claim exists for the thread.
    pub erasure_claim: FactObservation,
    /// Whether a tombstone exists for the thread.
    pub tombstone: FactObservation,
}

impl ThreadLivenessObservation {
    /// Creates an observation from the three independently reported facts.
    #[must_use]
    pub const fn new(
        thread_row: FactObservation,
        erasure_claim: FactObservation,
        tombstone: FactObservation,
    ) -> Self {
        Self {
            thread_row,
            erasure_claim,
            tombstone,
        }
    }

    /// Creates a complete observation from three completed query results.
    #[must_use]
    pub const fn complete(thread_row: bool, erasure_claim: bool, tombstone: bool) -> Self {
        Self::new(
            FactObservation::observed(thread_row),
            FactObservation::observed(erasure_claim),
            FactObservation::observed(tombstone),
        )
    }

    /// Returns whether all three queries supplied complete evidence.
    #[must_use]
    pub const fn is_complete(self) -> bool {
        self.thread_row.is_complete()
            && self.erasure_claim.is_complete()
            && self.tombstone.is_complete()
    }
}

/// The reason a complete observation admits a durable write.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThreadLivenessAdmissionReason {
    /// The row exists, no erasure claim exists, and no tombstone exists.
    AllRequiredFactsConfirmLive,
}

/// The reason a thread is rejected for durable-write admission.
///
/// Incomplete and failed observations are checked before completed blocking
/// facts. When more than one completed blocker exists, the reason follows the
/// source query order: missing thread row, erasure claim, then tombstone.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ThreadLivenessRejectionReason {
    /// The completed thread-row query found no row.
    ThreadRowAbsent,
    /// The completed erasure-claim query found a claim.
    ErasureClaimPresent,
    /// The completed tombstone query found a tombstone.
    TombstonePresent,
    /// The named query did not provide complete evidence.
    ObservationIncomplete { fact: ThreadLivenessFact },
    /// The named query failed and cannot provide evidence.
    ObservationFailed { fact: ThreadLivenessFact },
}

/// The durable-write admission decision for one exact thread identity.
///
/// The decision owns the supplied thread ID and never trims, case-folds,
/// canonicalizes, or otherwise normalizes it. Rejection carries a typed
/// reason so callers can map policy outcomes into their own error vocabulary.
#[must_use = "a thread liveness decision must be enforced before a durable write"]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ThreadLivenessDecision {
    /// Durable-write admission is allowed.
    Admitted {
        /// The exact supplied thread identity.
        thread_id: String,
        /// Why the durable write is admitted.
        reason: ThreadLivenessAdmissionReason,
    },
    /// Durable-write admission is rejected.
    Rejected {
        /// The exact supplied thread identity.
        thread_id: String,
        /// Why the durable write is rejected.
        reason: ThreadLivenessRejectionReason,
    },
}

impl ThreadLivenessDecision {
    /// Returns the exact thread ID carried by this decision.
    #[must_use]
    pub fn thread_id(&self) -> &str {
        match self {
            Self::Admitted { thread_id, .. } | Self::Rejected { thread_id, .. } => thread_id,
        }
    }

    /// Returns whether the decision grants durable-write admission.
    #[must_use]
    pub const fn allows_durable_write(&self) -> bool {
        matches!(self, Self::Admitted { .. })
    }
}

/// Stateless evaluator for the thread durable-write liveness policy.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ThreadLivenessPolicy;

impl ThreadLivenessPolicy {
    /// Creates the stateless policy value.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Evaluates the three observed facts for one exact thread identity.
    #[must_use = "a thread liveness decision must be enforced before a durable write"]
    pub fn evaluate<T>(
        thread_id: T,
        observations: ThreadLivenessObservation,
    ) -> ThreadLivenessDecision
    where
        T: Into<String>,
    {
        evaluate_thread_liveness(thread_id, observations)
    }
}

/// Evaluates whether one thread may receive a durable write.
///
/// A thread is admitted if and only if its row is observed as present, its
/// erasure claim is observed as absent, and its tombstone is observed as
/// absent. Any incomplete or failed observation rejects admission, even when
/// the other facts would otherwise describe a live thread.
#[must_use = "a thread liveness decision must be enforced before a durable write"]
pub fn evaluate_thread_liveness<T>(
    thread_id: T,
    observations: ThreadLivenessObservation,
) -> ThreadLivenessDecision
where
    T: Into<String>,
{
    let thread_id = thread_id.into();
    match rejection_reason(observations) {
        Some(reason) => ThreadLivenessDecision::Rejected { thread_id, reason },
        None => ThreadLivenessDecision::Admitted {
            thread_id,
            reason: ThreadLivenessAdmissionReason::AllRequiredFactsConfirmLive,
        },
    }
}

fn rejection_reason(
    observations: ThreadLivenessObservation,
) -> Option<ThreadLivenessRejectionReason> {
    for (fact, observation) in [
        (ThreadLivenessFact::ThreadRow, observations.thread_row),
        (ThreadLivenessFact::ErasureClaim, observations.erasure_claim),
        (ThreadLivenessFact::Tombstone, observations.tombstone),
    ] {
        match observation {
            FactObservation::Incomplete => {
                return Some(ThreadLivenessRejectionReason::ObservationIncomplete { fact });
            }
            FactObservation::Failed => {
                return Some(ThreadLivenessRejectionReason::ObservationFailed { fact });
            }
            FactObservation::Present | FactObservation::Absent => {}
        }
    }

    match (
        observations.thread_row,
        observations.erasure_claim,
        observations.tombstone,
    ) {
        (FactObservation::Absent, _, _) => Some(ThreadLivenessRejectionReason::ThreadRowAbsent),
        (_, FactObservation::Present, _) => {
            Some(ThreadLivenessRejectionReason::ErasureClaimPresent)
        }
        (_, _, FactObservation::Present) => Some(ThreadLivenessRejectionReason::TombstonePresent),
        (FactObservation::Present, FactObservation::Absent, FactObservation::Absent) => None,
        _ => unreachable!("incomplete and failed observations were handled above"),
    }
}
