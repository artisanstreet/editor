//! Dependency-free truth-table coverage for the pure reader-attention policy.
//!
//! The implementation is loaded directly so this focused harness does not
//! require shared module registration or any host/runtime integration.

#[path = "../../modules/frontend/src/reader_attention.rs"]
mod reader_attention;

use reader_attention::{
    VisibilityState, reader_can_acknowledge_root_conversation, reader_is_watching,
    reader_is_watching_raw,
};

#[test]
fn every_focus_and_visibility_combination_matches_the_truth_table() {
    let cases = [
        (false, "visible", false),
        (true, "visible", true),
        (false, "hidden", false),
        (true, "hidden", false),
        (false, "prerender", false),
        (true, "prerender", false),
    ];

    for (has_focus, raw_visibility_state, expected) in cases {
        assert_eq!(
            reader_is_watching_raw(has_focus, raw_visibility_state),
            expected,
            "focus={has_focus}, visibility={raw_visibility_state:?}"
        );

        let typed_visibility_state = VisibilityState::from_raw(raw_visibility_state);
        assert_eq!(
            reader_is_watching(has_focus, &typed_visibility_state),
            expected,
            "typed focus={has_focus}, visibility={raw_visibility_state:?}"
        );
    }
}

#[test]
fn visibility_parsing_is_exact_and_preserves_unknown_host_values() {
    assert_eq!(
        VisibilityState::from_raw("visible"),
        VisibilityState::Visible
    );
    assert_eq!(VisibilityState::from_raw("hidden"), VisibilityState::Hidden);

    for raw in ["VISIBLE", "Visible", "hidden ", " prerender", ""] {
        let state = VisibilityState::from_raw(raw);
        assert_eq!(state, VisibilityState::Unknown(raw.to_owned()));
        assert_eq!(state.as_raw(), raw);
        assert!(!state.is_visible());
        assert!(!reader_is_watching_raw(true, raw));
    }

    assert_eq!(VisibilityState::Visible.as_raw(), "visible");
    assert_eq!(VisibilityState::Hidden.as_raw(), "hidden");
}

#[test]
fn owned_and_borrowed_raw_values_use_the_same_exact_parser() {
    assert_eq!(VisibilityState::from("visible"), VisibilityState::Visible);
    assert_eq!(
        VisibilityState::from(String::from("hidden")),
        VisibilityState::Hidden
    );
    assert_eq!(
        VisibilityState::from(String::from("Visible")),
        VisibilityState::Unknown(String::from("Visible"))
    );
}

#[test]
fn every_watching_and_inspection_combination_matches_the_truth_table() {
    let cases = [
        (false, false, false),
        (false, true, false),
        (true, false, true),
        (true, true, false),
    ];

    for (watching, inspecting_agent, expected) in cases {
        assert_eq!(
            reader_can_acknowledge_root_conversation(watching, inspecting_agent),
            expected,
            "watching={watching}, inspecting_agent={inspecting_agent}"
        );
    }
}

#[test]
fn root_acknowledgement_composes_watching_and_agent_inspection() {
    let document_cases = [
        (false, "visible", false),
        (true, "visible", true),
        (true, "hidden", false),
        (true, "PRERENDER", false),
    ];

    for (has_focus, raw_visibility_state, watching) in document_cases {
        for inspecting_agent in [false, true] {
            let expected = watching && !inspecting_agent;
            let actual = reader_can_acknowledge_root_conversation(
                reader_is_watching_raw(has_focus, raw_visibility_state),
                inspecting_agent,
            );
            assert_eq!(
                actual, expected,
                "focus={has_focus}, visibility={raw_visibility_state:?}, inspecting_agent={inspecting_agent}"
            );
        }
    }
}
