//! Dependency-free exhaustive coverage for the document attention-title policy.

#[path = "../../modules/frontend/src/attention_title_policy.rs"]
mod attention_title_policy;

use attention_title_policy::{
    DEV_TITLE_MARKER, FORGE_REPAIR_TITLE_MARKER, attention_count_from_title,
    attention_marked_title, attention_title_marker_for, title_requests_forge_repair,
    title_signals_awaiting_answer,
};

const JOINER: &str = "\u{2060}";
const REPAIR: &str = "\u{2060}\u{2060}";

#[test]
fn public_markers_match_the_protocol_helpers() {
    assert_eq!(DEV_TITLE_MARKER, "[Dev]");
    assert_eq!(FORGE_REPAIR_TITLE_MARKER, REPAIR);

    let cases = [
        (0.0, false, "(0)\u{2060}"),
        (0.0, true, "(?)\u{2060}"),
        (3.0, false, "(3)\u{2060}"),
        (3.0, true, "(3?)\u{2060}"),
        (1234.0, true, "(1234?)\u{2060}"),
    ];
    for (count, awaiting_answer, expected) in cases {
        assert_eq!(
            attention_title_marker_for(count, awaiting_answer),
            expected,
            "count={count}, awaiting_answer={awaiting_answer}"
        );
    }
}

#[test]
fn marker_counts_are_truncated_and_clamped_without_losing_large_values() {
    let cases = [
        (-12.75, false, "(0)\u{2060}"),
        (-0.25, true, "(?)\u{2060}"),
        (0.99, false, "(0)\u{2060}"),
        (3.99, false, "(3)\u{2060}"),
        (9999.99, true, "(9999?)\u{2060}"),
        (10_000.99, false, "(10000)\u{2060}"),
        (1e21, false, "(1e+21)\u{2060}"),
        (f64::MAX, false, "(1.7976931348623157e+308)\u{2060}"),
        (f64::NEG_INFINITY, true, "(?)\u{2060}"),
        (f64::INFINITY, false, "(Infinity)\u{2060}"),
        (f64::NAN, false, "(NaN)\u{2060}"),
    ];

    for (count, awaiting_answer, expected) in cases {
        assert_eq!(
            attention_title_marker_for(count, awaiting_answer),
            expected,
            "count={count}, awaiting_answer={awaiting_answer}"
        );
    }
}

#[test]
fn plain_and_development_titles_keep_the_development_prefix_first() {
    let cases = [
        (
            "Workspace",
            Some(2.0),
            false,
            false,
            "(2)\u{2060} Workspace",
        ),
        (
            "[Dev] Workspace",
            Some(2.0),
            false,
            false,
            "[Dev] (2)\u{2060} Workspace",
        ),
        (
            "[Dev] Workspace",
            Some(2.0),
            false,
            true,
            "[Dev] (2?)\u{2060} Workspace",
        ),
        (
            "[Dev] (3?)\u{2060} Workspace",
            None,
            false,
            false,
            "[Dev] Workspace",
        ),
        (
            "[Dev] (3)\u{2060} Workspace",
            Some(4.0),
            false,
            false,
            "[Dev] (4)\u{2060} Workspace",
        ),
        ("[Dev]", Some(2.0), false, false, "(2)\u{2060} [Dev]"),
        ("[Dev]Ops", Some(2.0), false, false, "(2)\u{2060} [Dev]Ops"),
    ];

    for (title, count, repair, awaiting_answer, expected) in cases {
        assert_eq!(
            attention_marked_title(title, count, repair, awaiting_answer),
            expected,
            "title={title:?}, count={count:?}, repair={repair}, awaiting_answer={awaiting_answer}"
        );
    }
}

#[test]
fn ordinary_parentheses_and_nonleading_markers_remain_title_content() {
    let cases = [
        ("(3) fix the build", "(3) fix the build"),
        ("(12345) release notes", "(12345) release notes"),
        ("route (3) fix the build", "route (3) fix the build"),
        ("before (3)\u{2060} title", "before (3)\u{2060} title"),
        ("(3)\u{2060}title", "(3)\u{2060}title"),
        ("(3)\u{2060}\ttitle", "(3)\u{2060}\ttitle"),
        ("(3)\u{2060}\u{00a0}title", "(3)\u{2060}\u{00a0}title"),
        ("(-3)\u{2060} title", "(-3)\u{2060} title"),
        ("(x)\u{2060} title", "(x)\u{2060} title"),
    ];

    for (title, expected) in cases {
        assert_eq!(
            attention_marked_title(title, None, false, false),
            expected,
            "ordinary or invalid title={title:?}"
        );
    }
}

#[test]
fn only_leading_supported_composition_prefixes_are_removed() {
    let supported = [
        ("(0)\u{2060} title", "title"),
        ("(1234)\u{2060} title", "title"),
        ("(3?)\u{2060} title", "title"),
        ("(?)\u{2060} title", "title"),
        ("(3)\u{2060}  title", " title"),
    ];
    for (title, expected) in supported {
        assert_eq!(
            attention_marked_title(title, None, false, false),
            expected,
            "owned prefix should be removed from {title:?}"
        );
    }

    let unsupported = [
        "(12345)\u{2060} title",
        "(3??)\u{2060} title",
        "()\u{2060} title",
        "(-3)\u{2060} title",
        "(x)\u{2060} title",
        "(3) title",
        "(3)\u{2060}\ttitle",
        "(3)\u{2060}title",
    ];
    for title in unsupported {
        assert_eq!(
            attention_marked_title(title, None, false, false),
            title,
            "unowned prefix should remain in {title:?}"
        );
    }
}

#[test]
fn protocol_marker_readers_are_position_independent_but_width_strict() {
    let valid = [
        ("(0)\u{2060} title", Some(0), false),
        ("prefix (1)\u{2060} title", Some(1), false),
        ("(321)\u{2060} title", Some(321), false),
        ("(1234?)\u{2060} title", Some(1234), true),
        ("prefix (?)\u{2060} suffix", Some(0), true),
        ("🙂 (42)\u{2060} 日本語", Some(42), false),
    ];
    for (title, expected_count, expected_awaiting) in valid {
        assert_eq!(
            attention_count_from_title(title),
            expected_count,
            "count for {title:?}"
        );
        assert_eq!(
            title_signals_awaiting_answer(title),
            expected_awaiting,
            "awaiting state for {title:?}"
        );
    }

    let invalid = [
        "(12345)\u{2060} title",
        "(3??)\u{2060} title",
        "(3) title",
        "(3)\\u{2060} title",
        "(-3)\u{2060} title",
        "(x)\u{2060} title",
        "()\u{2060} title",
    ];
    for title in invalid {
        assert_eq!(
            attention_count_from_title(title),
            None,
            "count for {title:?}"
        );
        assert!(
            !title_signals_awaiting_answer(title),
            "awaiting for {title:?}"
        );
    }
}

#[test]
fn repair_markers_are_detected_and_canonicalized_everywhere() {
    let detection_cases = [
        ("plain", false),
        ("one\u{2060}joiner", false),
        ("two\u{2060}\u{2060}joiners", true),
        ("three\u{2060}\u{2060}\u{2060}joiners", true),
        ("🙂\u{2060}\u{2060}日本語", true),
    ];
    for (title, expected) in detection_cases {
        assert_eq!(
            title_requests_forge_repair(title),
            expected,
            "title={title:?}"
        );
    }

    let cases = [
        ("Workspace", None, true, "Workspace\u{2060}\u{2060}"),
        (
            "before\u{2060}\u{2060}\u{2060}\u{2060}middle\u{2060}\u{2060}after",
            None,
            false,
            "beforemiddleafter",
        ),
        (
            "[Dev] (2)\u{2060} before\u{2060}\u{2060}\u{2060}\u{2060}after",
            Some(7.0),
            true,
            "[Dev] (7)\u{2060} beforeafter\u{2060}\u{2060}",
        ),
        ("one\u{2060}two", None, false, "one\u{2060}two"),
    ];
    for (title, count, repair, expected) in cases {
        assert_eq!(
            attention_marked_title(title, count, repair, false),
            expected,
            "title={title:?}, count={count:?}, repair={repair}"
        );
    }

    assert_eq!(JOINER, "\u{2060}");
}

#[test]
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn count_and_question_states_are_composed_as_one_marker() {
    let cases = [
        (Some(0.0), false, "(0)\u{2060} title"),
        (Some(0.0), true, "(?)\u{2060} title"),
        (Some(3.0), false, "(3)\u{2060} title"),
        (Some(3.0), true, "(3?)\u{2060} title"),
        (Some(1234.0), true, "(1234?)\u{2060} title"),
        (None, true, "title"),
    ];

    for (count, awaiting_answer, expected) in cases {
        let actual = attention_marked_title("title", count, false, awaiting_answer);
        assert_eq!(
            actual, expected,
            "count={count:?}, awaiting_answer={awaiting_answer}"
        );
        assert_eq!(
            attention_count_from_title(&actual),
            count.map(|value| value as u32),
            "parsed count for {actual:?}"
        );
        assert_eq!(
            title_signals_awaiting_answer(&actual),
            count.is_some() && awaiting_answer,
            "parsed question state for {actual:?}"
        );
    }
}

#[test]
fn repeated_rewrites_are_idempotent_for_supported_marker_states() {
    let states = [
        (Some(0.0), false, false),
        (Some(4.0), true, false),
        (Some(9999.0), false, true),
        (None, false, false),
        (None, true, true),
    ];

    for (count, awaiting_answer, repair) in states {
        let first = attention_marked_title(
            "[Dev] (999?)\u{2060} Workspace\u{2060}\u{2060}",
            count,
            repair,
            awaiting_answer,
        );
        let second = attention_marked_title(&first, count, repair, awaiting_answer);
        let third = attention_marked_title(&second, count, repair, awaiting_answer);
        assert_eq!(
            first, second,
            "first rewrite state={count:?}/{awaiting_answer}/{repair}"
        );
        assert_eq!(
            second, third,
            "second rewrite state={count:?}/{awaiting_answer}/{repair}"
        );
    }

    let old = attention_marked_title("Workspace", Some(3.0), true, true);
    assert_eq!(
        attention_marked_title(&old, Some(8.0), false, false),
        "(8)\u{2060} Workspace"
    );
}
