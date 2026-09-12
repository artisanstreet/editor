//! Bounded coalescing for live assistant text deltas.
//!
//! A provider stream can emit assistant text one token at a time, and every
//! delta previously earned its own SQLite batch commit plus its own
//! subscriber wake. The coalescer buffers byte-exact append fragments and
//! reports a byte/count threshold or a 50 ms deadline, so one
//! commit carries a bounded run of deltas. Buffered bytes live in memory only:
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

/// Maximum delay before a partial burst becomes visible.
pub(super) const MAX_PENDING_DELTA_DELAY: Duration = Duration::from_millis(50);

/// In-memory buffer for one run of byte-exact assistant append fragments.
#[derive(Debug, Default)]
pub(super) struct DeltaCoalescer {
    pending: String,
    fragments: usize,
    deadline: Option<Instant>,
}

impl DeltaCoalescer {
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
            self.deadline = Some(Instant::now() + MAX_PENDING_DELTA_DELAY);
        }
        self.pending.push_str(text);
        self.fragments = self.fragments.saturating_add(1);
        self.pending.len() >= MAX_PENDING_DELTA_BYTES
            || self.fragments >= MAX_PENDING_DELTA_FRAGMENTS
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
