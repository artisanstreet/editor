//! Bounded coalescing for live assistant text deltas.
//!
//! A provider stream can emit assistant text one token at a time, and every
//! delta previously earned its own SQLite batch commit plus its own
//! subscriber wake. The coalescer buffers byte-exact append fragments and
//! reports a byte/count threshold or a deadline, so one commit carries a
//! bounded run of deltas. The deadline starts at 50 ms and follows how long
//! commits actually take: on a slow or busy disk a stream commits less often
//! in larger pieces instead of queueing a commit behind every other one, so
//! its share of the disk stays bounded. Buffered bytes live in memory only:
//! every full-body persistence path and the end of the turn flush the buffer,
//! so the durable projection stays byte-identical to the uncoalesced stream.

#![forbid(unsafe_code)]

use std::time::{Duration, Instant};

use artisan_domain::CONVERSATION_TEXT_FRAGMENT_MAX_BYTES;

/// Maximum buffered append bytes before a coalesced commit is due.
///
/// Equal to the durable single-fragment ceiling: a coalesced fragment passes
/// through the same `IncrementalText` validation as one provider delta.
pub(super) const MAX_PENDING_DELTA_BYTES: usize = CONVERSATION_TEXT_FRAGMENT_MAX_BYTES;

/// Maximum buffered delta count before a coalesced commit is due.
///
/// Small enough that a held burst still commits incrementally (subscribers
/// see every byte while the turn is live) and large enough that token-rate
/// streams collapse many deltas into one transaction and one wake.
pub(super) const MAX_PENDING_DELTA_FRAGMENTS: usize = 32;

/// Delay before a partial burst becomes visible while commits are fast.
pub(super) const MAX_PENDING_DELTA_DELAY: Duration = Duration::from_millis(50);

/// Ceiling on the adaptive delay: even on a stalled disk, text a stream has
/// already received is offered for commit at least once a second.
pub(super) const SLOW_DISK_DELTA_DELAY: Duration = Duration::from_secs(1);

/// A partial burst waits this many commit durations, so one stream keeps its
/// disk share near a fifth however slowly commits complete.
const COMMIT_COST_MULTIPLE: u32 = 4;

/// In-memory buffer for one run of byte-exact assistant append fragments.
#[derive(Debug)]
pub(super) struct DeltaCoalescer {
    pending: String,
    fragments: usize,
    deadline: Option<Instant>,
    delay: Duration,
}

impl Default for DeltaCoalescer {
    fn default() -> Self {
        Self {
            pending: String::new(),
            fragments: 0,
            deadline: None,
            delay: MAX_PENDING_DELTA_DELAY,
        }
    }
}

impl DeltaCoalescer {
    /// The delay a fresh burst waits before its deadline fires.
    #[cfg(test)]
    pub(super) const fn delay(&self) -> Duration {
        self.delay
    }

    /// Adapts the delay to how long one coalesced commit just took.
    ///
    /// The delay moves halfway toward [`COMMIT_COST_MULTIPLE`] commit
    /// durations, clamped to the 50 ms floor and the one-second ceiling, so a
    /// single slow outlier does not stall a stream and a recovered disk
    /// returns to the floor within a few commits.
    pub(super) fn record_commit(&mut self, took: Duration) {
        let target = took
            .saturating_mul(COMMIT_COST_MULTIPLE)
            .clamp(MAX_PENDING_DELTA_DELAY, SLOW_DISK_DELTA_DELAY);
        self.delay = (self.delay + target) / 2;
    }

    /// Fragment count that makes a commit due at the current delay.
    ///
    /// Scales with the delay so a slow disk does not get its commits back by
    /// the count threshold that a token-rate stream reaches first.
    fn fragment_threshold(&self) -> usize {
        let scale = self.delay.as_millis() / MAX_PENDING_DELTA_DELAY.as_millis();
        MAX_PENDING_DELTA_FRAGMENTS.saturating_mul(usize::try_from(scale.max(1)).unwrap_or(1))
    }

    /// Whether no delta is currently buffered.
    pub(super) fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// The first buffered fragment fixes the deadline; later arrivals do not extend it.
    pub(super) const fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// Buffered fragments, in arrival order; never above
    /// [`MAX_PENDING_DELTA_BYTES`] after a push returns.
    pub(super) fn pending(&self) -> &str {
        &self.pending
    }

    /// Whether appending `text` would cross the durable fragment ceiling.
    ///
    /// A caller that sees `true` flushes first and then buffers, so no
    /// coalesced commit can exceed one durable fragment.
    pub(super) fn would_overflow(&self, text: &str) -> bool {
        !self.pending.is_empty()
            && self.pending.len().saturating_add(text.len()) > MAX_PENDING_DELTA_BYTES
    }

    /// Buffers one byte-exact append fragment.
    ///
    /// Returns whether a threshold is now reached and the caller must flush
    /// before buffering anything further.
    pub(super) fn push(&mut self, text: &str) -> bool {
        if text.is_empty() {
            return false;
        }
        if self.pending.is_empty() {
            self.deadline = Some(Instant::now() + self.delay);
        }
        self.pending.push_str(text);
        self.fragments = self.fragments.saturating_add(1);
        self.pending.len() >= MAX_PENDING_DELTA_BYTES || self.fragments >= self.fragment_threshold()
    }

    /// Clears the buffer after its own coalesced commit, which took `took`,
    /// and adapts the delay to it.
    pub(super) fn clear_after_commit(&mut self, took: Duration) {
        self.clear();
        self.record_commit(took);
    }

    /// Clears the buffer after its bytes were durably committed.
    pub(super) fn clear(&mut self) {
        self.pending.clear();
        self.fragments = 0;
        self.deadline = None;
    }
}

#[cfg(test)]
mod tests {
    use super::{DeltaCoalescer, MAX_PENDING_DELTA_BYTES, MAX_PENDING_DELTA_FRAGMENTS};

    /// Drains one due flush, returning the exact fragment a commit would carry.
    fn flush(coalescer: &mut DeltaCoalescer) -> String {
        let fragment = coalescer.pending().to_owned();
        assert!(
            fragment.len() <= MAX_PENDING_DELTA_BYTES,
            "one coalesced fragment must stay inside the durable fragment ceiling"
        );
        coalescer.clear();
        fragment
    }

    #[test]
    fn token_burst_flushes_are_bounded_and_reassemble_exactly() {
        let mut coalescer = DeltaCoalescer::default();
        let mut committed = String::new();
        let mut expected = String::new();
        let mut flushes = 0_usize;
        let total = MAX_PENDING_DELTA_FRAGMENTS * 3 + 7;
        for index in 0..total {
            let delta = format!("token-{index} ");
            expected.push_str(&delta);
            if coalescer.would_overflow(&delta) {
                committed.push_str(&flush(&mut coalescer));
                flushes += 1;
            }
            if coalescer.push(&delta) {
                committed.push_str(&flush(&mut coalescer));
                flushes += 1;
            }
            assert!(coalescer.pending().len() <= MAX_PENDING_DELTA_BYTES);
        }
        if !coalescer.is_empty() {
            committed.push_str(&flush(&mut coalescer));
            flushes += 1;
        }
        // The uncoalesced transcript is the exact concatenation of every
        // delta, and the commit count is bounded by the fragment threshold.
        assert_eq!(committed, expected);
        assert!(
            flushes <= total / MAX_PENDING_DELTA_FRAGMENTS + 1,
            "one commit may carry up to {MAX_PENDING_DELTA_FRAGMENTS} deltas, saw {flushes} for {total}"
        );
    }

    #[test]
    fn byte_threshold_bounds_large_fragments() {
        let mut coalescer = DeltaCoalescer::default();
        let mut committed = String::new();
        let mut expected = String::new();
        let mut flushes = 0_usize;
        let chunk = "x".repeat(1_000);
        for _ in 0..10 {
            expected.push_str(&chunk);
            if coalescer.would_overflow(&chunk) {
                committed.push_str(&flush(&mut coalescer));
                flushes += 1;
            }
            if coalescer.push(&chunk) {
                committed.push_str(&flush(&mut coalescer));
                flushes += 1;
            }
            assert!(coalescer.pending().len() <= MAX_PENDING_DELTA_BYTES);
        }
        if !coalescer.is_empty() {
            committed.push_str(&flush(&mut coalescer));
            flushes += 1;
        }
        assert_eq!(committed, expected);
        // 10_000 bytes at a 4_096-byte ceiling needs at most four commits.
        assert!(flushes <= 10_000 / MAX_PENDING_DELTA_BYTES + 1);
    }

    #[test]
    fn overflow_is_exact_at_the_fragment_ceiling() {
        let mut coalescer = DeltaCoalescer::default();
        let filler = "x".repeat(MAX_PENDING_DELTA_BYTES - 1);
        assert!(!coalescer.would_overflow(&filler));
        assert!(!coalescer.push(&filler));
        assert_eq!(coalescer.pending().len(), MAX_PENDING_DELTA_BYTES - 1);
        // One more byte fits exactly; two would cross the ceiling.
        assert!(!coalescer.would_overflow("x"));
        assert!(coalescer.would_overflow("xx"));
        assert!(coalescer.push("x"));
        assert_eq!(coalescer.pending().len(), MAX_PENDING_DELTA_BYTES);
    }

    #[test]
    fn clear_resets_bytes_and_fragment_count_together() {
        let mut coalescer = DeltaCoalescer::default();
        assert!(coalescer.is_empty());
        for _ in 0..(MAX_PENDING_DELTA_FRAGMENTS - 1) {
            assert!(!coalescer.push("d"));
        }
        assert!(!coalescer.is_empty());
        assert!(coalescer.push("d"));
        coalescer.clear();
        assert!(coalescer.is_empty());
        assert_eq!(coalescer.pending().len(), 0);
        // The fragment count reset with the bytes: a fresh burst earns the
        // full count threshold again.
        for _ in 0..(MAX_PENDING_DELTA_FRAGMENTS - 1) {
            assert!(!coalescer.push("d"));
        }
        assert!(coalescer.push("d"));
    }
}

#[cfg(test)]
mod deadline_tests {
    use super::{DeltaCoalescer, MAX_PENDING_DELTA_DELAY};
    use std::time::Instant;

    #[test]
    fn partial_burst_has_a_deadline_that_later_fragments_cannot_extend() {
        let mut buffer = DeltaCoalescer::default();
        assert!(buffer.deadline().is_none());
        let before = Instant::now();
        assert!(!buffer.push("one"));
        let deadline = buffer.deadline().expect("partial burst deadline");
        assert!(deadline >= before + MAX_PENDING_DELTA_DELAY);
        assert!(deadline <= Instant::now() + MAX_PENDING_DELTA_DELAY);
        assert!(!buffer.push("two"));
        assert_eq!(buffer.deadline(), Some(deadline));
        buffer.clear();
        assert!(buffer.deadline().is_none());
        assert!(!buffer.push(""));
        assert!(buffer.deadline().is_none());
    }
}

#[cfg(test)]
mod adaptive_delay_tests {
    use super::{
        DeltaCoalescer, MAX_PENDING_DELTA_DELAY, MAX_PENDING_DELTA_FRAGMENTS, SLOW_DISK_DELTA_DELAY,
    };
    use std::time::Duration;

    #[test]
    fn fast_commits_keep_the_fifty_millisecond_floor() {
        let mut buffer = DeltaCoalescer::default();
        for _ in 0..20 {
            buffer.record_commit(Duration::from_millis(2));
        }
        assert_eq!(buffer.delay(), MAX_PENDING_DELTA_DELAY);
        for _ in 0..(MAX_PENDING_DELTA_FRAGMENTS - 1) {
            assert!(!buffer.push("d"));
        }
        assert!(buffer.push("d"), "the count threshold is unchanged");
    }

    #[test]
    fn slow_commits_stretch_the_delay_and_the_count_threshold_up_to_a_second() {
        let mut buffer = DeltaCoalescer::default();
        // A 500 ms commit (a busy hard disk) heads for the one-second ceiling.
        for _ in 0..12 {
            buffer.record_commit(Duration::from_millis(500));
        }
        assert!(buffer.delay() > Duration::from_millis(900));
        assert!(buffer.delay() <= SLOW_DISK_DELTA_DELAY);
        // A token-rate stream no longer forces a commit every 32 deltas.
        for _ in 0..(MAX_PENDING_DELTA_FRAGMENTS * 4) {
            assert!(!buffer.push("d"));
        }
        // The disk recovers: the delay falls back to the floor.
        buffer.clear();
        for _ in 0..12 {
            buffer.record_commit(Duration::from_millis(1));
        }
        assert!(buffer.delay() < MAX_PENDING_DELTA_DELAY + Duration::from_millis(1));
    }

    #[test]
    fn one_slow_outlier_moves_the_delay_only_halfway() {
        let mut buffer = DeltaCoalescer::default();
        buffer.record_commit(Duration::from_secs(10));
        assert_eq!(
            buffer.delay(),
            (MAX_PENDING_DELTA_DELAY + SLOW_DISK_DELTA_DELAY) / 2
        );
    }
}
