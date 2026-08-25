//! Focused external coverage for the native composer draft/submission state
//! leaf (`artisan_frontend::composer`).
//!
//! These tests pin the audited behavior carried over from the legacy
//! thread composer: derived send readiness, typed refusals for blank,
//! over-ceiling, disabled, and already-in-flight attempts, draft preservation
//! across refused and accepted begins, and single-flight exclusion with a
//! successful retry after finish.

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
    composer
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
    let body = composer
        .begin_submission()
        .expect("re-enabling restores sending");
    assert_eq!(body.as_str(), PADDED_DRAFT);
}

#[test]
fn an_active_flight_excludes_further_submissions_and_mutates_nothing() {
    let mut composer = ComposerState::new();
    composer.set_draft(PADDED_DRAFT);

    let body = composer
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
    composer
        .begin_submission()
        .expect("the fixture begins a flight");

    composer.finish_submission(DraftDisposition::Accepted);
    assert!(!composer.is_submitting());
    assert_eq!(
        composer.draft(),
        "",
        "only an accepted send resets the composer"
    );

    composer.set_draft("Next message");
    let next = composer
        .begin_submission()
        .expect("a finished flight frees the gate");
    assert_eq!(next.as_str(), "Next message");
}

#[test]
fn a_retained_finish_keeps_every_character_for_retry() {
    let mut composer = ComposerState::new();
    composer.set_draft(PADDED_DRAFT);
    composer
        .begin_submission()
        .expect("the fixture begins a flight");

    composer.finish_submission(DraftDisposition::Retained);
    assert!(!composer.is_submitting());
    assert_eq!(
        composer.draft(),
        PADDED_DRAFT,
        "a failed attempt keeps the exact untrimmed draft"
    );
    assert!(composer.send_ready(), "the retained draft rearms the send");

    let retried = composer
        .begin_submission()
        .expect("retry after finish succeeds");
    assert_eq!(
        retried.as_str(),
        PADDED_DRAFT,
        "the retried body is the same untrimmed content"
    );
    assert!(composer.is_submitting());

    composer.finish_submission(DraftDisposition::Retained);
    assert!(!composer.is_submitting());
}

#[test]
fn finishing_without_an_active_flight_cannot_clobber_a_fresh_draft() {
    let mut composer = ComposerState::new();
    composer.set_draft("Typed after the race");
    composer.finish_submission(DraftDisposition::Accepted);
    composer.finish_submission(DraftDisposition::Retained);

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
    let begun = composer
        .begin_submission()
        .expect("the fixture begins a flight");
    assert_eq!(begun.as_str(), "First message");

    // The model permits programmatic edits while a flight runs even though
    // the legacy editor disabled them; the older accepted completion must
    // clear only the body it actually sent.
    composer.set_draft("Second message drafted mid-flight");
    composer.finish_submission(DraftDisposition::Accepted);

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
