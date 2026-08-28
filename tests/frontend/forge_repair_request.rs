//! Focused, dependency-free coverage for Forge-repair request state and title intent.
//!
//! The production module is path-linked so this packet does not require
//! shared frontend registration, a browser host, or any runtime dependency.

#[path = "../../modules/frontend/src/forge_repair_request.rs"]
mod forge_repair_request;

use forge_repair_request::{
    DEV_TITLE_MARKER, FORGE_REPAIR_TITLE_MARKER, ForgeRepairRequestState, attention_marked_title,
};

const ATTENTION_JOINER: &str = "\u{2060}";
const REPAIR_MARKER: &str = "\u{2060}\u{2060}";

#[derive(Clone, Copy, Debug)]
struct TitleCase {
    label: &'static str,
    title: &'static str,
    attention_count: Option<usize>,
    awaiting_answer: bool,
    expected: &'static str,
}

#[test]
fn request_state_starts_clear_and_becomes_set_idempotently() {
    let mut state = ForgeRepairRequestState::new();

    assert!(!state.requested());
    assert!(!state.is_requested());

    state.request();
    assert!(state.requested());
    assert!(state.is_requested());

    state.request();
    assert!(state.requested());
}

#[test]
fn request_intent_preserves_attention_count_and_question_state() {
    let mut state = ForgeRepairRequestState::new();
    state.request();

    let intent = state.title_rewrite_intent(Some(3), true);

    assert_eq!(intent.attention_count(), Some(3));
    assert!(intent.awaiting_answer());
    assert!(intent.requests_forge_repair());
    assert_eq!(
        intent.apply_to("Workspace"),
        format!("(3?){ATTENTION_JOINER} Workspace{REPAIR_MARKER}")
    );
}

#[test]
fn requested_title_rewrites_match_the_shared_marker_table() {
    let mut state = ForgeRepairRequestState::new();
    state.request();

    let cases = [
        TitleCase {
            label: "ordinary",
            title: "Workspace",
            attention_count: None,
            awaiting_answer: false,
            expected: "Workspace\u{2060}\u{2060}",
        },
        TitleCase {
            label: "development-marked",
            title: "[Dev] Workspace",
            attention_count: None,
            awaiting_answer: false,
            expected: "[Dev] Workspace\u{2060}\u{2060}",
        },
        TitleCase {
            label: "attention-marked",
            title: "(2)\u{2060} Workspace",
            attention_count: Some(4),
            awaiting_answer: false,
            expected: "(4)\u{2060} Workspace\u{2060}\u{2060}",
        },
        TitleCase {
            label: "already-repair-marked",
            title: "Workspace\u{2060}\u{2060}\u{2060}\u{2060}",
            attention_count: None,
            awaiting_answer: false,
            expected: "Workspace\u{2060}\u{2060}",
        },
    ];

    for case in cases {
        let actual = state
            .title_rewrite_intent(case.attention_count, case.awaiting_answer)
            .apply_to(case.title);
        assert_eq!(actual, case.expected, "unexpected {0} title", case.label);
        assert_eq!(actual.matches(REPAIR_MARKER).count(), 1);
    }
}

#[test]
fn repeated_requests_converge_to_one_repair_suffix() {
    let mut state = ForgeRepairRequestState::new();
    state.request();
    state.request();

    let intent = state.title_rewrite_intent(Some(8), true);
    let first = intent.apply_to("[Dev] (999?)\u{2060} Workspace");
    let second = intent.apply_to(&first);

    assert_eq!(first, second);
    assert_eq!(
        first,
        format!("[Dev] (8?){ATTENTION_JOINER} Workspace{REPAIR_MARKER}")
    );
    assert_eq!(second.matches(REPAIR_MARKER).count(), 1);
}

#[test]
fn marker_ownership_and_ordering_match_the_attention_writer() {
    assert_eq!(DEV_TITLE_MARKER, "[Dev]");
    assert_eq!(FORGE_REPAIR_TITLE_MARKER, REPAIR_MARKER);

    let mut state = ForgeRepairRequestState::new();
    state.request();

    let supported_attention_titles = [
        "(0)\u{2060} Workspace",
        "(1234)\u{2060} Workspace",
        "(3?)\u{2060} Workspace",
        "(?)\u{2060} Workspace",
    ];
    for title in supported_attention_titles {
        let rewritten = state.title_rewrite_intent(None, false).apply_to(title);
        assert_eq!(rewritten, format!("Workspace{REPAIR_MARKER}"));
    }

    let ordinary_parenthesized = "(3) fix the build";
    assert_eq!(
        state
            .title_rewrite_intent(None, false)
            .apply_to(ordinary_parenthesized),
        format!("{ordinary_parenthesized}{REPAIR_MARKER}")
    );

    assert_eq!(
        attention_marked_title("one\u{2060}two", None, true, false),
        format!("one\u{2060}two{REPAIR_MARKER}")
    );
}

#[test]
fn attention_marker_formats_zero_question_and_regular_counts_exactly() {
    let mut state = ForgeRepairRequestState::new();
    state.request();

    let cases = [
        (Some(0), false, "(0)\u{2060} Workspace\u{2060}\u{2060}"),
        (Some(0), true, "(?)\u{2060} Workspace\u{2060}\u{2060}"),
        (
            Some(1234),
            true,
            "(1234?)\u{2060} Workspace\u{2060}\u{2060}",
        ),
    ];

    for (attention_count, awaiting_answer, expected) in cases {
        assert_eq!(
            state
                .title_rewrite_intent(attention_count, awaiting_answer)
                .apply_to("Workspace"),
            expected
        );
    }
}
