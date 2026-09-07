//! Coverage for the durable thread unread and attention presentation model.
//!
//! Exercises the public `artisan_frontend::attention` surface only: the
//! acknowledgement conditions (absent, matching, stale in either direction)
//! across every run state (active, idle, completed, failed).

use artisan_domain::UnixMillis;
use artisan_frontend::attention::{RunState, ThreadAttention};

const RUN_STATES: [RunState; 4] = [
    RunState::Active,
    RunState::Idle,
    RunState::Completed,
    RunState::Failed,
];

/// The fixed root-visible reader activity cursor under test.
const READER_MILLIS: i64 = 1_000;

fn cursor(millis: i64) -> UnixMillis {
    UnixMillis::from_millis(millis)
}

#[test]
fn an_absent_acknowledgement_is_unread_in_every_run_state() {
    for run_state in RUN_STATES {
        let attention = ThreadAttention::derive(run_state, cursor(READER_MILLIS), None);
        assert!(
            attention.is_unread(),
            "{run_state:?} without any recorded acknowledgement must present as unread"
        );
    }
}

#[test]
fn exact_cursor_equality_is_read_in_every_run_state() {
    // Signed epoch extremes settle exactly like ordinary instants.
    for millis in [-62_167_219_200_000, READER_MILLIS] {
        for run_state in RUN_STATES {
            let attention =
                ThreadAttention::derive(run_state, cursor(millis), Some(cursor(millis)));
            assert!(
                !attention.is_unread(),
                "{run_state:?} acknowledged at its own cursor {millis} must present as read"
            );
            assert!(
                !attention.needs_attention(),
                "a read thread never asks for attention"
            );
        }
    }
}

#[test]
fn a_stale_acknowledgement_is_unread_without_consulting_ordering() {
    for run_state in RUN_STATES {
        // The stamp lost the race against newer visible activity...
        let behind = ThreadAttention::derive(run_state, cursor(READER_MILLIS), Some(cursor(-5)));
        assert!(
            behind.is_unread(),
            "{run_state:?} behind the cursor is unread"
        );

        // ...and a stamp ahead of the visible cursor is still not a read:
        // only exact equality settles, never relative time.
        let ahead = ThreadAttention::derive(run_state, cursor(READER_MILLIS), Some(cursor(2_000)));
        assert!(
            ahead.is_unread(),
            "{run_state:?} ahead of the cursor is unread"
        );
    }
}

#[test]
fn active_work_keeps_authority_even_while_unread() {
    for acknowledged in [None, Some(cursor(-5)), Some(cursor(2_000))] {
        let attention =
            ThreadAttention::derive(RunState::Active, cursor(READER_MILLIS), acknowledged);
        assert!(
            !attention.needs_attention(),
            "an active run is never a needs-attention outcome"
        );
    }

    // A matching acknowledgement settles the row without touching the run.
    let settled = ThreadAttention::derive(
        RunState::Active,
        cursor(READER_MILLIS),
        Some(cursor(READER_MILLIS)),
    );
    assert!(!settled.is_unread());
    assert!(!settled.needs_attention());
}

#[test]
fn an_inactive_idle_thread_can_be_unread_without_needing_attention() {
    for acknowledged in [None, Some(cursor(-5)), Some(cursor(2_000))] {
        let attention =
            ThreadAttention::derive(RunState::Idle, cursor(READER_MILLIS), acknowledged);
        assert!(attention.is_unread());
        assert!(
            !attention.needs_attention(),
            "an idle thread carries no outcome, however stale its acknowledgement"
        );
    }
}

#[test]
fn only_an_inactive_unread_terminal_outcome_needs_attention() {
    for run_state in [RunState::Completed, RunState::Failed] {
        for acknowledged in [None, Some(cursor(-5)), Some(cursor(2_000))] {
            let attention = ThreadAttention::derive(run_state, cursor(READER_MILLIS), acknowledged);
            assert!(attention.is_unread());
            assert!(
                attention.needs_attention(),
                "a {run_state:?} thread the reader has not seen must ask for attention"
            );
        }

        // Once the acknowledgement catches up to the cursor, the outcome
        // stops asking.
        let settled = ThreadAttention::derive(
            run_state,
            cursor(READER_MILLIS),
            Some(cursor(READER_MILLIS)),
        );
        assert!(!settled.is_unread());
        assert!(!settled.needs_attention());
    }
}

#[test]
fn every_derivation_matches_the_semantic_truth_table() {
    // One independent oracle row per (run state, acknowledgement) input:
    // the expected observations are written out, not recomputed from the
    // implementation. Acknowledgement instants are absolute values against
    // the fixed reader cursor at 1_000.
    let table: [(RunState, Option<i64>, bool, bool); 16] = [
        (RunState::Active, None, true, false),
        (RunState::Active, Some(1_000), false, false),
        (RunState::Active, Some(-5), true, false),
        (RunState::Active, Some(2_000), true, false),
        (RunState::Idle, None, true, false),
        (RunState::Idle, Some(1_000), false, false),
        (RunState::Idle, Some(-5), true, false),
        (RunState::Idle, Some(2_000), true, false),
        (RunState::Completed, None, true, true),
        (RunState::Completed, Some(1_000), false, false),
        (RunState::Completed, Some(-5), true, true),
        (RunState::Completed, Some(2_000), true, true),
        (RunState::Failed, None, true, true),
        (RunState::Failed, Some(1_000), false, false),
        (RunState::Failed, Some(-5), true, true),
        (RunState::Failed, Some(2_000), true, true),
    ];

    for (run_state, acknowledged, unread, needs_attention) in table {
        let derived =
            ThreadAttention::derive(run_state, cursor(READER_MILLIS), acknowledged.map(cursor));
        assert_eq!(
            derived.is_unread(),
            unread,
            "{run_state:?} with acknowledgement {acknowledged:?}"
        );
        assert_eq!(
            derived.needs_attention(),
            needs_attention,
            "{run_state:?} with acknowledgement {acknowledged:?}"
        );
    }
}
