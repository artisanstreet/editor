//! Per-session server-event cursor sequencing coverage.

use std::error::Error;

use artisan_protocol::EventCursor;
use artisan_transport::{EventSequenceError, EventSequenceTracker, validate_event_successor};

fn cursor(value: u64) -> Result<EventCursor, Box<dyn Error>> {
    Ok(EventCursor::new(value)?)
}

#[test]
fn first_accepted_cursor_must_be_exactly_one() -> Result<(), Box<dyn Error>> {
    let mut tracker = EventSequenceTracker::new();
    assert_eq!(tracker.last_accepted(), None);

    let premature = tracker.accept(cursor(2)?);
    assert_eq!(
        premature,
        Err(EventSequenceError::ResnapshotRequired {
            expected: 1,
            actual: 2,
        })
    );
    assert_eq!(tracker.last_accepted(), None);

    tracker.accept(cursor(1)?)?;
    assert_eq!(tracker.last_accepted(), Some(cursor(1)?));
    Ok(())
}

#[test]
fn contiguous_cursors_are_all_admitted_in_order() -> Result<(), Box<dyn Error>> {
    let mut tracker = EventSequenceTracker::new();
    for expected in 1_u64..=5 {
        tracker.accept(cursor(expected)?)?;
        assert_eq!(tracker.last_accepted(), Some(cursor(expected)?));
    }
    Ok(())
}

#[test]
fn gap_requires_a_resnapshot_and_preserves_state() -> Result<(), Box<dyn Error>> {
    let mut tracker = EventSequenceTracker::new();
    tracker.accept(cursor(1)?)?;

    let gapped = tracker.accept(cursor(3)?);
    assert_eq!(
        gapped,
        Err(EventSequenceError::ResnapshotRequired {
            expected: 2,
            actual: 3,
        })
    );
    assert_eq!(tracker.last_accepted(), Some(cursor(1)?));
    Ok(())
}

#[test]
fn duplicate_requires_a_resnapshot_and_preserves_state() -> Result<(), Box<dyn Error>> {
    let mut tracker = EventSequenceTracker::new();
    tracker.accept(cursor(1)?)?;
    tracker.accept(cursor(2)?)?;

    let duplicated = tracker.accept(cursor(2)?);
    assert_eq!(
        duplicated,
        Err(EventSequenceError::ResnapshotRequired {
            expected: 3,
            actual: 2,
        })
    );
    assert_eq!(tracker.last_accepted(), Some(cursor(2)?));
    Ok(())
}

#[test]
fn regression_requires_a_resnapshot_and_preserves_state() -> Result<(), Box<dyn Error>> {
    let mut tracker = EventSequenceTracker::new();
    for value in 1_u64..=3 {
        tracker.accept(cursor(value)?)?;
    }

    let regressed = tracker.accept(cursor(1)?);
    assert_eq!(
        regressed,
        Err(EventSequenceError::ResnapshotRequired {
            expected: 4,
            actual: 1,
        })
    );
    assert_eq!(tracker.last_accepted(), Some(cursor(3)?));
    Ok(())
}

#[test]
fn every_rejection_leaves_the_tracker_unchanged() -> Result<(), Box<dyn Error>> {
    let mut tracker = EventSequenceTracker::new();
    tracker.accept(cursor(1)?)?;
    let before = tracker.last_accepted();

    // None of these probes can ever equal the required successor of the
    // unchanged state, so each must be rejected without mutation.
    for attempt in [1, 3, 9] {
        assert!(matches!(
            tracker.accept(cursor(attempt)?),
            Err(EventSequenceError::ResnapshotRequired { .. })
        ));
    }
    assert_eq!(tracker.last_accepted(), before);
    Ok(())
}

#[test]
fn exhausted_cursor_space_is_a_distinct_typed_overflow() -> Result<(), Box<dyn Error>> {
    // Reaching this state through a fresh tracker would require 2^64 accepted
    // events. Exercise the same successor validator used by the tracker at the
    // only boundary where no next cursor exists.
    let final_cursor = cursor(u64::MAX)?;
    assert_eq!(
        validate_event_successor(final_cursor, final_cursor),
        Err(EventSequenceError::CursorOverflow { last: u64::MAX })
    );
    Ok(())
}
