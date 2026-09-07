//! Focused coverage for the immutable pending-steering-lip state machine.
//!
//! The production module is intentionally generic and value-oriented: these
//! tests cover the visible newest-first stack, generation ownership, missing
//! and duplicate states, and checked end-of-space behavior without involving
//! the composer, effects, or a clock.

use artisan_frontend::steering_pending_lip::{
    BeginPendingSteeringLip, PendingSteeringLip, SteeringPendingLipError, SteeringPendingLipState,
    begin_pending_steering_lip, release_pending_steering_lip,
};

fn lip(generation: u64, submission: &str) -> PendingSteeringLip<String> {
    PendingSteeringLip::new(
        generation,
        generation.saturating_mul(10),
        submission.to_owned(),
    )
}

fn generations<Submission>(state: &SteeringPendingLipState<Submission>) -> Vec<u64> {
    state
        .pending()
        .iter()
        .map(PendingSteeringLip::generation)
        .collect()
}

fn submissions(state: &SteeringPendingLipState<String>) -> Vec<&str> {
    state
        .pending()
        .iter()
        .map(|pending| pending.submission().as_str())
        .collect()
}

fn three_lips() -> SteeringPendingLipState<String> {
    SteeringPendingLipState::from_parts(3, vec![lip(3, "third"), lip(2, "second"), lip(1, "first")])
}

fn begin<Submission: Clone>(
    state: &SteeringPendingLipState<Submission>,
    submission: Submission,
    started_at: u64,
) -> BeginPendingSteeringLip<Submission> {
    begin_pending_steering_lip(state, submission, started_at).expect("generation available")
}

#[test]
fn empty_state_starts_at_zero_and_first_begin_owns_generation_one() {
    let state = SteeringPendingLipState::<String>::new();
    assert_eq!(state.next_generation(), 0);
    assert!(state.is_empty());
    assert_eq!(state.len(), 0);

    let begun = begin(&state, "first".to_owned(), 123);
    assert_eq!(begun.begun.generation(), 1);
    assert_eq!(begun.begun.started_at(), 123);
    assert_eq!(begun.begun.submission(), "first");
    assert_eq!(begun.state.next_generation(), 1);
    assert_eq!(generations(&begun.state), vec![1]);
    assert_eq!(submissions(&begun.state), vec!["first"]);

    // The value passed by the caller is still the original empty snapshot.
    assert_eq!(state, SteeringPendingLipState::new());
}

#[test]
fn successive_begins_insert_newest_first_and_keep_each_old_snapshot() {
    let empty = SteeringPendingLipState::<String>::new();
    let first = begin(&empty, "first".to_owned(), 10);
    let second = begin(&first.state, "second".to_owned(), 20);
    let third = begin(&second.state, "third".to_owned(), 30);

    assert_eq!(generations(&third.state), vec![3, 2, 1]);
    assert_eq!(submissions(&third.state), vec!["third", "second", "first"]);
    assert_eq!(&third.begun, &third.state.pending()[0]);
    assert_eq!(generations(&first.state), vec![1]);
    assert_eq!(generations(&second.state), vec![2, 1]);
    assert_eq!(empty, SteeringPendingLipState::new());
}

#[test]
fn begin_clones_a_generic_non_copy_payload_into_begun_and_state() {
    #[derive(Clone, Debug, Eq, PartialEq)]
    struct Submission {
        text: String,
        attachment_bytes: Vec<u8>,
    }

    let submission = Submission {
        text: "steer".to_owned(),
        attachment_bytes: vec![0, 1, 255],
    };
    let state = SteeringPendingLipState::new();
    let begun = begin(&state, submission.clone(), 77);

    assert_eq!(begun.begun.submission(), &submission);
    assert_eq!(begun.state.pending()[0].submission(), &submission);
    assert_eq!(
        begun.state.pending()[0].submission(),
        begun.begun.submission()
    );
    assert_eq!(state, SteeringPendingLipState::new());
}

#[test]
fn release_missing_generation_is_false_and_preserves_the_value() {
    let state = three_lips();
    let before = state.clone();

    let result = release_pending_steering_lip(&state, 99);

    assert!(!result.released);
    assert_eq!(result.state, before);
    assert_eq!(state, before);
}

#[test]
fn release_first_middle_and_last_removes_only_the_matching_entry() {
    let state = three_lips();

    let first = release_pending_steering_lip(&state, 3);
    assert!(first.released);
    assert_eq!(generations(&first.state), vec![2, 1]);
    assert_eq!(submissions(&first.state), vec!["second", "first"]);

    let middle = release_pending_steering_lip(&state, 2);
    assert!(middle.released);
    assert_eq!(generations(&middle.state), vec![3, 1]);
    assert_eq!(submissions(&middle.state), vec!["third", "first"]);

    let last = release_pending_steering_lip(&state, 1);
    assert!(last.released);
    assert_eq!(generations(&last.state), vec![3, 2]);
    assert_eq!(submissions(&last.state), vec!["third", "second"]);
    assert_eq!(state, three_lips());
}

#[test]
fn old_settlement_cannot_remove_a_newer_pending_lip() {
    let empty = SteeringPendingLipState::<String>::new();
    let first = begin(&empty, "old".to_owned(), 1);
    let second = begin(&first.state, "new".to_owned(), 2);

    let settled_old = release_pending_steering_lip(&second.state, first.begun.generation());
    assert!(settled_old.released);
    assert_eq!(
        generations(&settled_old.state),
        vec![second.begun.generation()]
    );
    assert_eq!(submissions(&settled_old.state), vec!["new"]);

    // A replayed old receipt is now missing and still cannot touch the new
    // entry.
    let replay = release_pending_steering_lip(&settled_old.state, first.begun.generation());
    assert!(!replay.released);
    assert_eq!(generations(&replay.state), vec![second.begun.generation()]);
}

#[test]
fn duplicate_generation_entries_are_all_released_without_touching_others() {
    // Duplicates cannot be produced by begin from a valid state, but a
    // restored/impossible value remains a valid input value for release.
    let state = SteeringPendingLipState::from_parts(
        8,
        vec![
            lip(8, "new-a"),
            lip(4, "old"),
            lip(8, "new-b"),
            lip(2, "older"),
        ],
    );

    let result = release_pending_steering_lip(&state, 8);

    assert!(result.released);
    assert_eq!(generations(&result.state), vec![4, 2]);
    assert_eq!(submissions(&result.state), vec!["old", "older"]);
    assert_eq!(
        state,
        SteeringPendingLipState::from_parts(
            8,
            vec![
                lip(8, "new-a"),
                lip(4, "old"),
                lip(8, "new-b"),
                lip(2, "older"),
            ],
        )
    );
}

#[test]
fn impossible_counter_and_stack_order_are_preserved_by_release() {
    // The constructor does not silently repair a state whose counter and
    // entries disagree; release owns only the exact generation it receives.
    let state = SteeringPendingLipState::from_parts(1, vec![lip(7, "seven"), lip(9, "nine")]);

    let result = release_pending_steering_lip(&state, 7);

    assert!(result.released);
    assert_eq!(result.state.next_generation(), 1);
    assert_eq!(generations(&result.state), vec![9]);
    assert_eq!(submissions(&result.state), vec!["nine"]);
}

#[test]
fn generation_space_does_not_wrap_and_failed_begin_keeps_state_unchanged() {
    let state = SteeringPendingLipState::from_parts(u64::MAX, vec![lip(u64::MAX, "last")]);
    let before = state.clone();

    let error = begin_pending_steering_lip(&state, "discarded".to_owned(), 999).unwrap_err();

    assert_eq!(error, SteeringPendingLipError::GenerationExhausted);
    assert_eq!(
        error.to_string(),
        "steering pending lip generation space is exhausted"
    );
    assert_eq!(state, before);
}

#[test]
fn maximum_generation_is_issued_once_then_space_is_exhausted() {
    let state = SteeringPendingLipState::from_parts(u64::MAX - 1, Vec::new());

    let begun = begin(&state, "final".to_owned(), 456);

    assert_eq!(begun.begun.generation(), u64::MAX);
    assert_eq!(begun.state.next_generation(), u64::MAX);
    assert_eq!(generations(&begun.state), vec![u64::MAX]);
    assert_eq!(
        begin_pending_steering_lip(&begun.state, "never".to_owned(), 789),
        Err(SteeringPendingLipError::GenerationExhausted)
    );
}
