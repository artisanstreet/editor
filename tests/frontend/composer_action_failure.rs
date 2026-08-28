//! Focused, dependency-free coverage for composer action failures.
//!
//! The production module is path-linked deliberately: this packet does not
//! edit shared frontend registration, so it can be checked with plain `rustc`
//! and no Cargo, Bazel, notification, transport, or UI dependencies.

#[path = "../../modules/frontend/src/composer_action_failure.rs"]
mod composer_action_failure;

use composer_action_failure::ComposerActionFailure;

#[test]
fn constructor_preserves_title_and_description_roles_byte_for_byte() {
    let title = "Could not send message";
    let description = "  exact cause\nwith tabs\tand spacing  ";
    let failure = ComposerActionFailure::new(title, description);

    assert_eq!(failure.title.as_bytes(), title.as_bytes());
    assert_eq!(failure.description.as_bytes(), description.as_bytes());
    assert_ne!(failure.title, failure.description);
}

#[test]
fn empty_and_unicode_values_are_preserved() {
    let cases = [("", ""), ("标题 — 🚀", "原因\n空")];

    for (title, description) in cases {
        let failure = ComposerActionFailure::new(title, description);

        assert_eq!(failure.title.as_bytes(), title.as_bytes());
        assert_eq!(failure.description.as_bytes(), description.as_bytes());
    }
}

#[test]
fn reached_producers_keep_their_evidenced_titles_and_caller_descriptions() {
    let cases = [
        (
            "attachment intake",
            "Could not attach image",
            "image is too large — keep this cause",
        ),
        (
            "queued-message discard",
            "Could not discard the queued message",
            "the queued message was already settled",
        ),
        (
            "existing-thread send",
            "Could not send message",
            "the receiver refused the submission",
        ),
        (
            "new-thread send",
            "Could not start a new thread",
            "navigation failed after draft creation",
        ),
        (
            "steer settlement",
            "Could not confirm the steer",
            "acknowledgement returned an exact error",
        ),
    ];

    for (producer, title, description) in cases {
        let failure = ComposerActionFailure::new(title, description);

        assert_eq!(failure.title, title, "{producer} title mapping");
        assert_eq!(
            failure.description, description,
            "{producer} description mapping"
        );
    }
}

#[test]
fn derived_clone_debug_and_equality_behave_as_for_an_owned_value() {
    let failure = ComposerActionFailure::new("title", "description");
    let clone = failure.clone();

    assert_eq!(clone, failure);
    assert_eq!(format!("{clone:?}"), format!("{failure:?}"));
    assert!(format!("{failure:?}").contains("title"));
    assert!(format!("{failure:?}").contains("description"));
    assert_ne!(
        failure,
        ComposerActionFailure::new("different title", "description")
    );
}
