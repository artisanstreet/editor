//! Focused external coverage for the native composer draft/submission state
//! leaf (`artisan_frontend::composer`).
//!
//! These tests pin the audited behavior carried over from the legacy
//! thread composer: derived send readiness, typed refusals for blank,
//! over-ceiling, disabled, and already-in-flight attempts, draft preservation
//! across refused and accepted begins, and single-flight exclusion with a
//! successful retry after finish. They also pin flight-identity fencing:
//! every begun flight carries an opaque token required for completion, so a
//! late, duplicated, or foreign completion cannot consume a newer flight,
//! disturb its draft, or rearm submission. Finally they pin authored-change
//! tracking per flight: any actual post-begin draft change — even one later
//! reverted — blocks an accepted clear, while a same-value rewrite does not.

use artisan_domain::bounds::MESSAGE_BODY_MAX_BYTES;
use artisan_domain::text::MessageBodyError;
use artisan_frontend::composer::{ComposerState, DraftDisposition, SubmissionBlocked};

/// A valid draft with deliberate surrounding whitespace: the state model must
/// keep it verbatim while treating it as ready.
const PADDED_DRAFT: &str = "  Fix the flaky login test.  ";

#[test]
fn a_fresh_composer_is_quiet_and_not_ready() {
    let mut composer = ComposerState::new();

    assert_eq!(composer.draft(), "");
    assert!(!composer.is_disabled());
    assert!(!composer.is_submitting());
    assert!(!composer.send_ready());

    let refused = composer
        .begin_submission()
        .expect_err("an empty draft cannot begin a submission");
    assert_eq!(
        refused,
        SubmissionBlocked::InvalidBody(MessageBodyError::Blank)
    );
}

#[test]
fn readiness_follows_draft_visibility_disabled_and_flight_state() {
    let mut composer = ComposerState::new();

    composer.set_draft("   \t ");
    assert!(
        !composer.send_ready(),
        "whitespace-only drafts are not send-ready"
    );

    composer.set_draft("Ready message");
    assert!(composer.send_ready());

    composer.set_disabled(true);
    assert!(
        !composer.send_ready(),
        "a disabled surface disarms an otherwise ready draft"
    );

    composer.set_disabled(false);
    let (_body, _token) = composer
        .begin_submission()
        .expect("the fixture begins a flight");
    assert!(
        !composer.send_ready(),
        "an active submission disarms the control until it finishes"
    );
}

#[test]
fn blank_drafts_are_refused_without_a_flight_or_lost_text() {
    for blank in ["", " ", "\t\n  "] {
        let mut composer = ComposerState::new();
        composer.set_draft(blank);

        let refusal = composer.begin_submission().expect_err("blank is refused");
        assert_eq!(
            refusal,
            SubmissionBlocked::InvalidBody(MessageBodyError::Blank)
        );
        assert!(
            !composer.is_submitting(),
            "a refused attempt never starts a flight"
        );
        assert_eq!(composer.draft(), blank, "the untouched draft is preserved");
        assert!(!composer.send_ready());
    }
}

#[test]
fn oversized_bodies_are_refused_through_the_native_domain_bound() {
    let mut composer = ComposerState::new();

    // Two-byte characters double the pressure: exactly MESSAGE_BODY_MAX_BYTES
    // UTF-8 bytes must stay acceptable at the readiness layer...
    composer.set_draft("é".repeat(MESSAGE_BODY_MAX_BYTES / 2));
    assert!(composer.send_ready(), "readiness tracks visible characters");

    // ...while one extra character crosses the byte ceiling only when a
    // submission actually begins.
    composer.set_draft("é".repeat((MESSAGE_BODY_MAX_BYTES / 2) + 1));

    let refusal = composer.begin_submission().expect_err("the bound holds");
    assert_eq!(
        refusal,
        SubmissionBlocked::InvalidBody(MessageBodyError::TooLong {
            length: MESSAGE_BODY_MAX_BYTES + 2,
            maximum: MESSAGE_BODY_MAX_BYTES,
        })
    );
    assert!(
        !composer.is_submitting(),
        "an invalid body must not start a flight"
    );
    assert_eq!(
        composer.draft().chars().count(),
        (MESSAGE_BODY_MAX_BYTES / 2) + 1,
        "the oversized draft is kept for trimming or retry"
    );
}

#[test]
fn disabled_surfaces_refuse_submission_but_keep_the_draft() {
    let mut composer = ComposerState::new();
    composer.set_draft(PADDED_DRAFT);
    composer.set_disabled(true);

    let refusal = composer.begin_submission().expect_err("disabled refuses");
    assert_eq!(refusal, SubmissionBlocked::Disabled);
    assert!(!composer.is_submitting());
    assert_eq!(composer.draft(), PADDED_DRAFT);

    composer.set_disabled(false);
    let (body, _token) = composer
        .begin_submission()
        .expect("re-enabling restores sending");
    assert_eq!(body.as_str(), PADDED_DRAFT);
}

#[test]
fn an_active_flight_excludes_further_submissions_and_mutates_nothing() {
    let mut composer = ComposerState::new();
    composer.set_draft(PADDED_DRAFT);

    let (body, _token) = composer
        .begin_submission()
        .expect("the first begin acquires the flight");
    assert_eq!(body.as_str(), PADDED_DRAFT);
    assert!(composer.is_submitting());

    let refusal = composer
        .begin_submission()
        .expect_err("single-flight excludes concurrent submissions");
    assert_eq!(refusal, SubmissionBlocked::InFlight);
    assert!(composer.is_submitting(), "the original flight continues");
    assert_eq!(
        composer.draft(),
        PADDED_DRAFT,
        "an excluded begin leaves the draft alone"
    );
}

#[test]
fn an_accepted_finish_resets_the_draft_and_allows_the_next_send() {
    let mut composer = ComposerState::new();
    composer.set_draft(PADDED_DRAFT);
    let (_body, token) = composer
        .begin_submission()
        .expect("the fixture begins a flight");

    composer.finish_submission(token, DraftDisposition::Accepted);
    assert!(!composer.is_submitting());
    assert_eq!(
        composer.draft(),
        "",
        "only an accepted send resets the composer"
    );

    composer.set_draft("Next message");
    let (next, _token) = composer
        .begin_submission()
        .expect("a finished flight frees the gate");
    assert_eq!(next.as_str(), "Next message");
}

#[test]
fn a_retained_finish_keeps_every_character_for_retry() {
    let mut composer = ComposerState::new();
    composer.set_draft(PADDED_DRAFT);
    let (_body, token) = composer
        .begin_submission()
        .expect("the fixture begins a flight");

    composer.finish_submission(token, DraftDisposition::Retained);
    assert!(!composer.is_submitting());
    assert_eq!(
        composer.draft(),
        PADDED_DRAFT,
        "a failed attempt keeps the exact untrimmed draft"
    );
    assert!(composer.send_ready(), "the retained draft rearms the send");

    let (retried_body, retried_token) = composer
        .begin_submission()
        .expect("retry after finish succeeds");
    assert_eq!(
        retried_body.as_str(),
        PADDED_DRAFT,
        "the retried body is the same untrimmed content"
    );
    assert!(composer.is_submitting());

    composer.finish_submission(retried_token, DraftDisposition::Retained);
    assert!(!composer.is_submitting());
}

#[test]
fn a_duplicate_completion_after_the_flight_ended_cannot_clobber_a_fresh_draft() {
    let mut composer = ComposerState::new();
    composer.set_draft("First message");
    let (_body, token) = composer
        .begin_submission()
        .expect("the fixture begins a flight");
    composer.finish_submission(token, DraftDisposition::Retained);
    assert!(!composer.is_submitting(), "the fixture ends idle");

    // The retired token replays twice while nothing is in flight; neither
    // disposition may discard text typed after its flight finished.
    composer.set_draft("Typed after the race");
    composer.finish_submission(token, DraftDisposition::Accepted);
    composer.finish_submission(token, DraftDisposition::Retained);

    assert_eq!(
        composer.draft(),
        "Typed after the race",
        "a duplicate completion must not discard newly typed text"
    );
    assert!(!composer.is_submitting());
}

#[test]
fn an_accepted_finish_cannot_erase_text_entered_after_its_flight_began() {
    let mut composer = ComposerState::new();
    composer.set_draft("First message");
    let (begun, token) = composer
        .begin_submission()
        .expect("the fixture begins a flight");
    assert_eq!(begun.as_str(), "First message");

    // The model permits programmatic edits while a flight runs even though
    // the legacy editor disabled them; the older accepted completion must
    // clear only the body it actually sent.
    composer.set_draft("Second message drafted mid-flight");
    composer.finish_submission(token, DraftDisposition::Accepted);

    assert!(!composer.is_submitting(), "the flight itself still ends");
    assert_eq!(
        composer.draft(),
        "Second message drafted mid-flight",
        "a newer draft survives an older accepted completion"
    );

    composer
        .begin_submission()
        .expect("the freed gate sends the surviving draft");
}

#[test]
fn a_stale_accepted_completion_from_an_older_flight_cannot_consume_a_newer_one() {
    let mut composer = ComposerState::new();
    composer.set_draft("First message");
    let (_first_body, first_token) = composer.begin_submission().expect("flight A begins");
    composer.finish_submission(first_token, DraftDisposition::Accepted);
    assert!(!composer.is_submitting(), "flight A completed cleanly");

    composer.set_draft("Second message");
    let (_second_body, second_token) = composer
        .begin_submission()
        .expect("flight B begins while A is already settled");

    // The late or duplicated completion of A replays while B owns the gate.
    composer.finish_submission(first_token, DraftDisposition::Accepted);

    assert!(
        composer.is_submitting(),
        "A's stale token must not consume B's flight"
    );
    assert_eq!(
        composer.draft(),
        "Second message",
        "A's stale accepted replay must not clear B's draft"
    );

    let refusal = composer
        .begin_submission()
        .expect_err("A's stale replay must not rearm the gate either");
    assert_eq!(refusal, SubmissionBlocked::InFlight);

    composer.finish_submission(second_token, DraftDisposition::Accepted);
    assert!(
        !composer.is_submitting(),
        "B still completes under its own token"
    );
    assert_eq!(
        composer.draft(),
        "",
        "B's own accepted send clears the body B actually sent"
    );
}

#[test]
fn a_stale_retained_completion_from_an_older_flight_cannot_release_a_newer_gate() {
    let mut composer = ComposerState::new();
    composer.set_draft("First message");
    let (_first_body, first_token) = composer.begin_submission().expect("flight A begins");
    composer.finish_submission(first_token, DraftDisposition::Retained);

    composer.set_draft("Second message");
    let (_second_body, second_token) = composer.begin_submission().expect("flight B begins");

    // Replaying A's retained outcome while B is live must neither end B nor
    // free the single-flight gate early.
    composer.finish_submission(first_token, DraftDisposition::Retained);

    assert!(
        composer.is_submitting(),
        "A's stale token must not release B's flight"
    );
    assert_eq!(
        composer.draft(),
        "Second message",
        "B's draft survives A's stale retained replay verbatim"
    );

    let refusal = composer
        .begin_submission()
        .expect_err("a third begin stays excluded: the gate was never released by A's replay");
    assert_eq!(refusal, SubmissionBlocked::InFlight);

    composer.finish_submission(second_token, DraftDisposition::Retained);
    assert!(!composer.is_submitting(), "B completes under its own token");
    assert_eq!(
        composer.draft(),
        "Second message",
        "B's own retained finish keeps every character for retry"
    );
}

#[test]
fn an_identical_body_retry_mints_distinct_identity_the_old_token_cannot_complete() {
    let mut composer = ComposerState::new();
    composer.set_draft(PADDED_DRAFT);
    let (_first_body, first_token) = composer
        .begin_submission()
        .expect("the first attempt begins");
    composer.finish_submission(first_token, DraftDisposition::Retained);

    // The identical authored text is retried: byte-equal body, brand-new
    // flight identity.
    let (retry_body, retry_token) = composer
        .begin_submission()
        .expect("the identical-body retry begins");
    assert_eq!(
        retry_body.as_str(),
        PADDED_DRAFT,
        "the retry sends the same untrimmed content"
    );
    assert_ne!(
        retry_token, first_token,
        "identity is per attempt, not per body; retired identity is never reused"
    );

    // Replaying the first attempt's token against the equal-bodied retry —
    // even as an accepted send of exactly that text — must be inert.
    composer.finish_submission(first_token, DraftDisposition::Accepted);
    assert!(
        composer.is_submitting(),
        "the older flight's token cannot complete the identical-body retry"
    );
    assert_eq!(
        composer.draft(),
        PADDED_DRAFT,
        "...and cannot clear the retry's draft either"
    );

    composer.finish_submission(retry_token, DraftDisposition::Accepted);
    assert!(
        !composer.is_submitting(),
        "the retry's own token completes it"
    );
    assert_eq!(
        composer.draft(),
        "",
        "its accepted send clears the text it actually sent"
    );
}

#[test]
fn a_foreign_composers_token_completes_nothing_here() {
    let mut mine = ComposerState::new();
    let mut other = ComposerState::new();

    other.set_draft("Another surface's message");
    let (_other_body, other_token) = other
        .begin_submission()
        .expect("the foreign surface begins its flight");

    mine.set_draft(PADDED_DRAFT);
    let (_mine_body, mine_token) = mine
        .begin_submission()
        .expect("my surface begins its own flight");

    mine.finish_submission(other_token, DraftDisposition::Accepted);

    assert!(
        mine.is_submitting(),
        "another composer's token cannot end my flight"
    );
    assert_eq!(mine.draft(), PADDED_DRAFT, "my draft is untouched");

    let refusal = mine
        .begin_submission()
        .expect_err("the gate stays held by my real flight");
    assert_eq!(refusal, SubmissionBlocked::InFlight);

    mine.finish_submission(mine_token, DraftDisposition::Accepted);
    assert!(
        !mine.is_submitting(),
        "my own matching completion still works"
    );
}

#[test]
fn an_edit_reverted_to_the_submitted_text_still_blocks_the_accepted_clear() {
    let mut composer = ComposerState::new();
    composer.set_draft("Original message");
    let (_body, token) = composer
        .begin_submission()
        .expect("the fixture begins a flight");

    // Edit away and back: the draft ends byte-identical to the submitted
    // body, but an actual post-begin change happened during this flight.
    composer.set_draft("Mid-flight replacement");
    composer.set_draft("Original message");
    assert_eq!(composer.draft(), "Original message", "fixture is reverted");

    composer.finish_submission(token, DraftDisposition::Accepted);

    assert!(!composer.is_submitting(), "the matched flight still ends");
    assert_eq!(
        composer.draft(),
        "Original message",
        "a reverted edit is still an edit; the authored draft survives"
    );

    // The preserved draft can be submitted again immediately.
    let (resent_body, resent_token) = composer
        .begin_submission()
        .expect("the preserved draft rearms sending");
    assert_eq!(resent_body.as_str(), "Original message");

    // This new flight saw no edits of its own, so its accepted send clears.
    composer.finish_submission(resent_token, DraftDisposition::Accepted);
    assert_eq!(
        composer.draft(),
        "",
        "a fresh unchanged flight clears normally"
    );
}

#[test]
fn rewriting_the_same_text_mid_flight_is_not_an_edit() {
    let mut composer = ComposerState::new();
    composer.set_draft(PADDED_DRAFT);
    let (_body, token) = composer
        .begin_submission()
        .expect("the fixture begins a flight");

    // Same-value writes leave the current flight unmarked, unlike any
    // differing write; reverting semantics are covered by the revert test.
    composer.set_draft(PADDED_DRAFT);
    composer.set_draft(PADDED_DRAFT);

    composer.finish_submission(token, DraftDisposition::Accepted);
    assert!(!composer.is_submitting());
    assert_eq!(
        composer.draft(),
        "",
        "an identical no-op rewrite must not block the normal accepted clear"
    );
}

#[test]
fn a_new_flight_does_not_inherit_the_previous_flight_s_changed_marker() {
    let mut composer = ComposerState::new();
    composer.set_draft("First text");
    let (_first_body, first_token) = composer
        .begin_submission()
        .expect("the first flight begins");
    composer.set_draft("Edited mid-flight");
    composer.finish_submission(first_token, DraftDisposition::Retained);
    assert_eq!(
        composer.draft(),
        "Edited mid-flight",
        "retention keeps every character including the edit"
    );

    // The second flight starts from the retained text and starts unmarked,
    // forgetting the first flight's edits entirely.
    let (_second_body, second_token) = composer
        .begin_submission()
        .expect("the second flight begins");
    composer.finish_submission(second_token, DraftDisposition::Accepted);

    assert_eq!(
        composer.draft(),
        "",
        "the new flight's accepted send clears: old flights' markers do not carry over"
    );
}
