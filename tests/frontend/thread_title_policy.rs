//! Dependency-free parity tests for the native thread-title policy.
//!
//! The matrix mirrors `modules/frontend/src/lib/threads/title.ts` and keeps
//! the source module local because its shared library registration is owned by
//! the VP. Titles and selected results are checked as borrowed strings; raw
//! modes are checked in both borrowed and owned construction forms.

#[path = "../../modules/frontend/src/thread_title_policy.rs"]
mod thread_title_policy;

use thread_title_policy::{ThreadTitleInput, ThreadTitleMode, thread_display_title};

#[test]
fn recognized_modes_are_exact_and_summary_is_the_default() {
    assert_eq!(ThreadTitleMode::default(), ThreadTitleMode::Summary);
    assert_eq!(
        ThreadTitleMode::ALL,
        [ThreadTitleMode::Summary, ThreadTitleMode::LatestMessage]
    );

    assert_eq!(ThreadTitleMode::Summary.as_raw(), "summary");
    assert_eq!(ThreadTitleMode::LatestMessage.as_raw(), "latest_message");
    assert_eq!(ThreadTitleMode::Summary.as_str(), "summary");
    assert_eq!(ThreadTitleMode::LatestMessage.as_str(), "latest_message");
    assert!(ThreadTitleMode::Summary.is_summary());
    assert!(!ThreadTitleMode::LatestMessage.is_summary());
}

#[test]
fn every_mode_lock_and_summary_presence_combination_follows_the_policy() {
    let cases = [
        ("present", Some("Generated session title")),
        ("absent", None),
        ("empty", Some("")),
    ];
    let modes = [
        ThreadTitleMode::Summary,
        ThreadTitleMode::LatestMessage,
        ThreadTitleMode::Unknown(String::from("future_mode")),
    ];

    for mode in &modes {
        for (presence, summary_title) in cases {
            let unlocked = ThreadTitleInput::new(summary_title, "Latest user message", false);
            let expected = if mode.is_summary() {
                summary_title.unwrap_or("Latest user message")
            } else {
                "Latest user message"
            };
            assert_eq!(
                thread_display_title(unlocked, mode),
                expected,
                "mode={mode:?}, summary={presence}"
            );

            let locked = ThreadTitleInput::new(summary_title, "My renamed thread", true);
            assert_eq!(
                thread_display_title(locked, mode),
                "My renamed thread",
                "mode={mode:?}, summary={presence}, locked"
            );
        }
    }
}

#[test]
fn explicitly_empty_summary_is_present_and_beats_the_stored_title() {
    let input = ThreadTitleInput::new(Some(""), "Latest user message", false);

    assert_eq!(thread_display_title(input, ThreadTitleMode::Summary), "");
    assert_eq!(
        thread_display_title(input, ThreadTitleMode::LatestMessage),
        "Latest user message"
    );
}

#[test]
fn absent_summary_falls_back_without_treating_other_modes_as_summary() {
    let input = ThreadTitleInput::new(None, "Latest user message", false);

    for mode in [
        ThreadTitleMode::Summary,
        ThreadTitleMode::LatestMessage,
        ThreadTitleMode::Unknown(String::from("summary_v2")),
    ] {
        assert_eq!(
            thread_display_title(input, mode),
            "Latest user message",
            "mode without a summary must use the stored title"
        );
    }
}

#[test]
fn raw_mode_parsing_is_exact_and_preserves_unknown_values() {
    let known = [
        ("summary", ThreadTitleMode::Summary),
        ("latest_message", ThreadTitleMode::LatestMessage),
    ];
    for (raw, expected) in known {
        let mode = ThreadTitleMode::from_raw(raw);
        assert_eq!(mode, expected);
        assert_eq!(mode.as_raw(), raw);
    }

    for raw in [
        "Summary",
        " latest_message",
        "latest_message ",
        "",
        "future",
    ] {
        let mode = ThreadTitleMode::from_raw(raw);
        assert_eq!(mode, ThreadTitleMode::Unknown(raw.to_owned()));
        assert_eq!(mode.as_raw(), raw);
        assert!(!mode.is_summary());
    }
}

#[test]
fn borrowed_and_owned_raw_mode_inputs_use_the_same_exact_parser() {
    assert_eq!(ThreadTitleMode::from("summary"), ThreadTitleMode::Summary);
    assert_eq!(
        ThreadTitleMode::from(String::from("latest_message")),
        ThreadTitleMode::LatestMessage
    );

    let raw = String::from(" future mode 🚀 ");
    let raw_pointer = raw.as_ptr();
    let owned = ThreadTitleMode::from(raw);
    assert_eq!(
        owned,
        ThreadTitleMode::Unknown(String::from(" future mode 🚀 "))
    );
    assert_eq!(owned.as_raw(), " future mode 🚀 ");
    assert_eq!(owned.as_raw().as_ptr(), raw_pointer);
    assert_eq!(owned.clone().into_raw(), " future mode 🚀 ");
}

#[test]
fn titles_are_returned_verbatim_and_the_selector_borrows_selected_text() {
    let summary = String::from("  概要 🚀 — café\n");
    let stored = String::from("最新のメッセージ 🧵  ");
    let input = ThreadTitleInput::new(Some(summary.as_str()), stored.as_str(), false);

    let selected_summary = thread_display_title(input, &ThreadTitleMode::Summary);
    assert_eq!(selected_summary, summary.as_str());
    assert_eq!(selected_summary.as_ptr(), summary.as_ptr());

    let selected_stored = thread_display_title(input, ThreadTitleMode::LatestMessage);
    assert_eq!(selected_stored, stored.as_str());
    assert_eq!(selected_stored.as_ptr(), stored.as_ptr());

    let locked = ThreadTitleInput::new(Some(summary.as_str()), stored.as_str(), true);
    let selected_locked = thread_display_title(locked, &ThreadTitleMode::Summary);
    assert_eq!(selected_locked, stored.as_str());
    assert_eq!(selected_locked.as_ptr(), stored.as_ptr());
}
