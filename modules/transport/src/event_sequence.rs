//! Per-session delivery ordering for Forge-originated server events.
//!
//! Every server event carries a one-based per-session cursor: the session's
//! first accepted event carries exactly cursor 1, and every later accepted
//! event carries exactly one more than its predecessor.
//! [`EventSequenceTracker`] admits only such contiguous progress. A gap,
//! duplicate, or regression returns a resnapshot-required failure and leaves
//! the tracker unchanged. Callers must admit an event's cursor before applying
//! its payload and must discard the event and resnapshot on rejection.
//! Exhausting the cursor space is a distinct overflow failure.

use artisan_protocol::EventCursor;
use thiserror::Error;

/// The mandatory first cursor of every session's event sequence.
const FIRST_EVENT_CURSOR: u64 = 1;

/// Failure while admitting one server event into the session sequence.
#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum EventSequenceError {
    /// The event skipped, repeated, or reversed the session sequence, so a
    /// fresh snapshot must be applied before trusting later events.
    #[error("resnapshot required: expected server event cursor {expected}, received {actual}")]
    ResnapshotRequired {
        /// Next contiguous session cursor required.
        expected: u64,
        /// Cursor that actually arrived.
        actual: u64,
    },
    /// The highest accepted cursor was the final representable value, so no
    /// further cursor exists and the sequence can never continue.
    #[error("no server event cursor exists beyond {last}")]
    CursorOverflow {
        /// Highest accepted cursor when advancement failed.
        last: u64,
    },
}

/// Tracks the highest accepted cursor of one session's event sequence.
///
/// Acceptance and rejection are all-or-nothing: only an exactly contiguous
/// cursor mutates this tracker, so a rejected event can never be applied
/// against stale ordering state.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct EventSequenceTracker {
    /// Highest accepted cursor, or `None` before the first accepted event.
    last_accepted: Option<EventCursor>,
}

impl EventSequenceTracker {
    /// Creates a tracker awaiting the session's first event at cursor 1.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            last_accepted: None,
        }
    }

    /// Returns the highest accepted cursor, or `None` before the first one.
    #[must_use]
    pub const fn last_accepted(&self) -> Option<EventCursor> {
        self.last_accepted
    }

    /// Admits one event cursor into the session sequence.
    ///
    /// On success the tracker advances to `cursor`. Callers must invoke this
    /// method before applying the corresponding event payload. On failure,
    /// they must discard that event and resnapshot; the tracker stays intact.
    ///
    /// # Errors
    ///
    /// Returns [`EventSequenceError::ResnapshotRequired`] when `cursor`
    /// skips, repeats, or reverses the sequence, and
    /// [`EventSequenceError::CursorOverflow`] when the sequence already ended
    /// at [`u64::MAX`] so no cursor can follow it.
    pub fn accept(&mut self, cursor: EventCursor) -> Result<(), EventSequenceError> {
        if let Some(previous) = self.last_accepted {
            validate_event_successor(previous, cursor)?;
        } else if cursor.get() != FIRST_EVENT_CURSOR {
            return Err(EventSequenceError::ResnapshotRequired {
                expected: FIRST_EVENT_CURSOR,
                actual: cursor.get(),
            });
        }
        self.last_accepted = Some(cursor);
        Ok(())
    }
}

/// Validates one cursor as the exactly contiguous successor of another.
///
/// This stateless seam is shared by sequence owners that already hold an
/// accepted cursor. Fresh session consumers must use [`EventSequenceTracker`]
/// so the mandatory first cursor remains exactly 1.
///
/// # Errors
///
/// Returns [`EventSequenceError::ResnapshotRequired`] when `cursor` is not the
/// exact successor of `previous`, or [`EventSequenceError::CursorOverflow`]
/// when `previous` is [`u64::MAX`] and therefore has no successor.
pub fn validate_event_successor(
    previous: EventCursor,
    cursor: EventCursor,
) -> Result<(), EventSequenceError> {
    let last = previous.get();
    let expected = last
        .checked_add(1)
        .ok_or(EventSequenceError::CursorOverflow { last })?;
    if cursor.get() != expected {
        return Err(EventSequenceError::ResnapshotRequired {
            expected,
            actual: cursor.get(),
        });
    }
    Ok(())
}
