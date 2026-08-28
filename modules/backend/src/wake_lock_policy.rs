//! Pure hold-or-release policy for the system wake lock.
//!
//! This is the dependency-free Rust counterpart of
//! `modules/backend/src/host/wake-lock-policy.ts`. The host service owns the
//! clock, database read, and operating-system assertion; this module only
//! classifies the caller-provided unsettled-work snapshot.
//!
//! Epoch timestamps are signed milliseconds, matching the domain timestamp
//! contract. Age arithmetic is widened to `i128` before subtraction, and the
//! derived recheck deadline is also `i128`: two valid signed `i64` values can
//! have a difference or sum outside the signed `i64` range. Work counts are
//! widened to `u128` for the same reason at the `usize` boundary. Neither
//! calculation can panic or wrap for the values accepted by this boundary.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

/// Everything the wake-lock decision needs to know about in-flight work.
///
/// `progressing_count` contains work that can make progress without a human:
/// queued, running, and retry-waiting runs, plus durable graph work that has
/// not produced a lifecycle event yet. Each approval timestamp represents one
/// waiting run's newest pending request.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct UnsettledWorkSnapshot<'a> {
    /// Number of work items that progress without human input.
    pub progressing_count: usize,
    /// Newest pending request instant for each human-blocked run.
    pub approval_requested_at_ms: &'a [i64],
}

impl<'a> UnsettledWorkSnapshot<'a> {
    /// Creates a snapshot from a progressing-work count and request instants.
    #[must_use]
    pub const fn new(progressing_count: usize, approval_requested_at_ms: &'a [i64]) -> Self {
        Self {
            progressing_count,
            approval_requested_at_ms,
        }
    }
}

/// One evaluated hold-or-release decision.
///
/// `recheck_at_ms` is `None` only when no approval is currently inside its
/// grace window. A progressing item alone therefore holds the lock but does
/// not create a timer-driven recheck, matching the stable TypeScript result.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct WakeLockAssessment {
    /// Number of work items currently justifying the hold.
    ///
    /// This is widened from the snapshot's `usize` count so an input at
    /// `usize::MAX` plus one graced approval remains an exact count rather
    /// than wrapping to zero.
    pub held_count: u128,
    /// Whether the host should keep the system awake.
    pub hold: bool,
    /// Earliest approval-grace expiry, or `None` for a stable decision.
    ///
    /// The widened signed timestamp preserves a deadline that is beyond the
    /// representable `i64` epoch range instead of clamping it or dropping the
    /// recheck signal.
    pub recheck_at_ms: Option<i128>,
}

/// Decides whether unsettled work justifies keeping the host awake.
///
/// Progressing work always holds. A human-blocked run holds only while its
/// newest request is strictly younger than `approval_grace_ms`, preserving the
/// TypeScript boundary `now_ms - requested_at_ms < approval_grace_ms`:
/// equality is already expired. An approval that is future-relative to `now`
/// therefore has a negative age and is still inside any nonnegative grace
/// window, just as in the source policy. The signed grace parameter also keeps
/// the pure function total for the unchecked signed integer inputs accepted by
/// the TypeScript function.
///
/// All timestamp arithmetic is performed as `i128`. This makes every pair of
/// signed `i64` timestamps and every signed `i64` grace value explicit and
/// total, including values on opposite integer boundaries. The count sum is
/// widened to `u128` and uses saturating addition as a defensive final guard;
/// a `usize` slice cannot exhaust `u128` on supported Rust targets.
#[must_use]
pub fn assess_unsettled_work(
    snapshot: UnsettledWorkSnapshot<'_>,
    now_ms: i64,
    approval_grace_ms: i64,
) -> WakeLockAssessment {
    let now_ms = i128::from(now_ms);
    let approval_grace_ms = i128::from(approval_grace_ms);
    let mut graced_count = 0_u128;
    let mut earliest_expiry_ms: Option<i128> = None;

    for &requested_at_ms in snapshot.approval_requested_at_ms {
        let requested_at_ms = i128::from(requested_at_ms);
        let age_ms = now_ms - requested_at_ms;

        if age_ms >= approval_grace_ms {
            continue;
        }

        graced_count = graced_count.saturating_add(1);
        let expiry_ms = requested_at_ms + approval_grace_ms;
        earliest_expiry_ms = Some(match earliest_expiry_ms {
            Some(current) => current.min(expiry_ms),
            None => expiry_ms,
        });
    }

    let held_count = (snapshot.progressing_count as u128).saturating_add(graced_count);

    WakeLockAssessment {
        held_count,
        hold: held_count > 0,
        recheck_at_ms: earliest_expiry_ms,
    }
}
