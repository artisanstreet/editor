//! Dependency-free parity tests for the development-instance policy.

#[path = "../../modules/frontend/src/dev_instance_policy.rs"]
mod dev_instance_policy;

use dev_instance_policy::{
    DEV_TITLE_MARKER, DecodedHealth, DecodedHealthValue, dev_marked_title, is_development_instance,
};

#[test]
fn development_detection_requires_a_non_null_object_and_exact_true() {
    let cases = [
        ("absent health", None, false),
        ("null", Some(DecodedHealth::Null), false),
        (
            "top-level false",
            Some(DecodedHealth::Boolean(false)),
            false,
        ),
        ("top-level true", Some(DecodedHealth::Boolean(true)), false),
        ("number", Some(DecodedHealth::Number), false),
        ("string", Some(DecodedHealth::String), false),
        (
            "array with missing development",
            Some(DecodedHealth::array(None)),
            false,
        ),
        (
            "object with missing development",
            Some(DecodedHealth::object(None)),
            false,
        ),
        (
            "object with null development",
            Some(DecodedHealth::object(Some(DecodedHealthValue::Null))),
            false,
        ),
        (
            "object with number development",
            Some(DecodedHealth::object(Some(DecodedHealthValue::Number))),
            false,
        ),
        (
            "object with string development",
            Some(DecodedHealth::object(Some(DecodedHealthValue::String))),
            false,
        ),
        (
            "object with array development",
            Some(DecodedHealth::object(Some(DecodedHealthValue::Array))),
            false,
        ),
        (
            "object with object development",
            Some(DecodedHealth::object(Some(DecodedHealthValue::Object))),
            false,
        ),
        (
            "object with false development",
            Some(DecodedHealth::object(Some(DecodedHealthValue::Boolean(
                false,
            )))),
            false,
        ),
        (
            "object with true development",
            Some(DecodedHealth::object(Some(DecodedHealthValue::Boolean(
                true,
            )))),
            true,
        ),
    ];

    for (case, health, expected) in cases {
        assert_eq!(is_development_instance(health), expected, "case={case}");
    }
}

#[test]
fn arrays_follow_the_same_reachable_development_rule_as_objects() {
    let cases = [
        ("missing", DecodedHealth::array(None), false),
        (
            "null",
            DecodedHealth::array(Some(DecodedHealthValue::Null)),
            false,
        ),
        (
            "number",
            DecodedHealth::array(Some(DecodedHealthValue::Number)),
            false,
        ),
        (
            "string",
            DecodedHealth::array(Some(DecodedHealthValue::String)),
            false,
        ),
        (
            "nested array",
            DecodedHealth::array(Some(DecodedHealthValue::Array)),
            false,
        ),
        (
            "nested object",
            DecodedHealth::array(Some(DecodedHealthValue::Object)),
            false,
        ),
        (
            "false",
            DecodedHealth::array(Some(DecodedHealthValue::Boolean(false))),
            false,
        ),
        (
            "true",
            DecodedHealth::array(Some(DecodedHealthValue::Boolean(true))),
            true,
        ),
    ];

    for (case, health, expected) in cases {
        assert_eq!(
            is_development_instance(Some(health)),
            expected,
            "array case={case}"
        );
    }

    // The decoded projection intentionally does not discard a property based
    // on whether JavaScript reached it as an own or prototype property.
    let own_property = DecodedHealth::array(Some(DecodedHealthValue::Boolean(true)));
    let prototype_reachable_property =
        DecodedHealth::array(Some(DecodedHealthValue::Boolean(true)));
    assert!(is_development_instance(Some(own_property)));
    assert!(is_development_instance(Some(prototype_reachable_property)));
}

#[test]
fn marker_and_basic_titles_are_exact() {
    assert_eq!(DEV_TITLE_MARKER, "[Dev]");

    let cases = [
        ("", "[Dev]"),
        (" ", "[Dev]"),
        ("\t\n\r\u{000B}\u{000C}", "[Dev]"),
        ("Workspace", "[Dev] Workspace"),
        ("日本語の題名 🚀", "[Dev] 日本語の題名 🚀"),
    ];

    for (title, expected) in cases {
        assert_eq!(dev_marked_title(title), expected, "title={title:?}");
    }
}

#[test]
fn every_ecmascript_trailing_whitespace_character_is_trimmed() {
    let whitespace = [
        '\u{0009}', '\u{000A}', '\u{000B}', '\u{000C}', '\u{000D}', '\u{0020}', '\u{00A0}',
        '\u{1680}', '\u{2000}', '\u{2001}', '\u{2002}', '\u{2003}', '\u{2004}', '\u{2005}',
        '\u{2006}', '\u{2007}', '\u{2008}', '\u{2009}', '\u{200A}', '\u{2028}', '\u{2029}',
        '\u{202F}', '\u{205F}', '\u{3000}', '\u{FEFF}',
    ];

    for character in whitespace {
        let title = format!("Title{character}");
        assert_eq!(
            dev_marked_title(&title),
            "[Dev] Title",
            "character={character:?}"
        );
    }
}

#[test]
fn non_ecmascript_whitespace_is_preserved() {
    for title in ["Title\u{0085}", "Title\u{180E}"] {
        assert_eq!(dev_marked_title(title), format!("[Dev] {title}"));
    }

    assert_eq!(
        dev_marked_title("Résumé — разработка — 開発\u{3000}"),
        "[Dev] Résumé — разработка — 開発"
    );
}

#[test]
fn exact_marker_prefix_is_case_sensitive_and_accepts_no_space_continuations() {
    let cases = [
        ("[Dev]", "[Dev]"),
        ("[Dev] ", "[Dev] "),
        ("[Dev]\tTitle", "[Dev]\tTitle"),
        ("[Dev]Ops", "[Dev]Ops"),
        ("[DEV] Workspace", "[Dev] [DEV] Workspace"),
        ("[dev] Workspace", "[Dev] [dev] Workspace"),
        ("[Dev ] Workspace", "[Dev] [Dev ] Workspace"),
        (" [Dev] Workspace", "[Dev]  [Dev] Workspace"),
    ];

    for (title, expected) in cases {
        assert_eq!(dev_marked_title(title), expected, "title={title:?}");
    }
}

#[test]
fn title_marking_is_idempotent_for_a_table_of_inputs() {
    let titles = [
        "",
        "Workspace",
        "  Workspace\u{00A0}",
        "日本語の題名 🚀",
        "[Dev]Ops",
        "[DEV] Workspace",
        "Title\u{0085}",
    ];

    for title in titles {
        let marked = dev_marked_title(title);
        assert_eq!(dev_marked_title(&marked), marked, "title={title:?}");
    }
}
