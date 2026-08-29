//! Dependency-free exhaustive tests for the frontend speed presentation leaf.
//!
//! The shared frontend registration is VP-owned, so this focused harness
//! includes the production file directly. Its tables mirror the catalog
//! fields and the exact selector/dispatch call-site rules.

#![forbid(unsafe_code)]

#[path = "../../modules/frontend/src/speed_presentation.rs"]
mod speed_presentation;

use speed_presentation::{
    DEFAULT_NATIVE_VALUE, DEFAULT_SPEED_ID, FAST_CLASS, FAST_LABEL, SUPERFAST_CLASS,
    SUPERFAST_LABEL, SpeedOption, SpeedOptionPresentation, authoritative_speed_id,
    available_speed_options, default_speed, dispatch_speed_presentation, is_available,
    selected_speed_by_id, selected_speed_by_native_value, speed_by_id, speed_by_native_value,
    speed_option_presentation, trigger_speed_presentation,
};

fn option(
    id: &str,
    label: &str,
    native_value: &str,
    description: &str,
    default: bool,
    disabled: Option<&str>,
) -> SpeedOption {
    SpeedOption::new(
        id,
        label,
        native_value,
        description,
        default,
        disabled.map(str::to_owned),
    )
}

fn standard_speeds() -> Vec<SpeedOption> {
    vec![
        option(
            "standard",
            "Standard",
            "standard",
            "Standard description",
            true,
            None,
        ),
        option("fast", "Fast", "fast", "Fast description", false, None),
    ]
}

fn all_branded_speeds() -> Vec<SpeedOption> {
    vec![
        option(
            "standard",
            "Standard",
            "standard",
            "Standard description",
            true,
            None,
        ),
        option("fast", "Fast", "fast", "Fast description", false, None),
        option(
            "superfast",
            "Superfast",
            "superfast",
            "Superfast description",
            false,
            None,
        ),
    ]
}

fn disabled_fast_speeds() -> Vec<SpeedOption> {
    vec![
        option(
            "standard",
            "Standard",
            "standard",
            "Standard description",
            true,
            None,
        ),
        option(
            "fast",
            "Fast",
            "fast",
            "Fast unavailable",
            false,
            Some("Fast mode is unavailable"),
        ),
    ]
}

#[test]
fn every_recognized_presentation_value_has_exact_strings() {
    let cases = [
        (
            option("standard", "Standard", "standard", "ordinary", true, None),
            "",
            "Standard",
        ),
        (
            option(
                "fast",
                "Provider fast",
                "fast",
                "fast description",
                false,
                None,
            ),
            FAST_CLASS,
            FAST_LABEL,
        ),
        (
            option(
                "superfast",
                "Provider accelerated",
                "superfast",
                "superfast description",
                false,
                None,
            ),
            SUPERFAST_CLASS,
            SUPERFAST_LABEL,
        ),
    ];

    for (speed, expected_class, expected_label) in cases {
        assert_eq!(
            speed_option_presentation(&speed),
            SpeedOptionPresentation {
                class_name: expected_class,
                label: expected_label.to_owned(),
            },
            "id={}",
            speed.id
        );
    }

    assert_eq!(FAST_CLASS, "text-amber-600 dark:text-amber-400");
    assert_eq!(FAST_LABEL, "Fast");
    assert_eq!(
        SUPERFAST_CLASS,
        "bg-linear-to-r from-purple-500 to-green-500 bg-clip-text text-transparent"
    );
    assert_eq!(SUPERFAST_LABEL, "Superfast");
}

#[test]
fn ordinary_and_unknown_ids_keep_labels_and_do_not_normalize() {
    let cases = [
        ("standard", "Native"),
        ("standard-plus", "Standard Plus"),
        ("unknown", "Whatever"),
        ("", ""),
        ("FAST", "FAST"),
        ("Fast", "Fast"),
        ("fast ", "fast "),
        (" superfast", " superfast"),
    ];

    for (id, label) in cases {
        let speed = option(id, label, "wire", "exact description", false, None);
        let presentation = speed_option_presentation(&speed);
        assert_eq!(presentation.class_name, "", "id={id:?}");
        assert_eq!(presentation.label, label, "id={id:?}");
    }
}

#[test]
fn presentation_uses_only_id_and_label_and_preserves_description_elsewhere() {
    let speed = option(
        "fast",
        "Provider label",
        "provider-fast",
        "Provider description with exact punctuation.",
        true,
        Some("temporarily unavailable"),
    );

    assert_eq!(
        speed.description,
        "Provider description with exact punctuation."
    );
    assert_eq!(speed.native_value, "provider-fast");
    assert!(speed.default);
    assert_eq!(speed.disabled.as_deref(), Some("temporarily unavailable"));
    assert_eq!(speed_option_presentation(&speed).class_name, FAST_CLASS);
    assert_eq!(speed_option_presentation(&speed).label, FAST_LABEL);
}

#[test]
fn availability_is_only_the_presence_of_a_disabled_reason() {
    let available = option("fast", "Fast", "fast", "available", false, None);
    let empty_reason = option("fast", "Fast", "fast", "disabled", false, Some(""));
    let unavailable = option(
        "fast",
        "Fast",
        "fast",
        "disabled",
        false,
        Some("account does not support Fast"),
    );

    assert!(is_available(&available));
    assert!(!is_available(&empty_reason));
    assert!(!is_available(&unavailable));
}

#[test]
fn available_options_filter_disabled_values_and_preserve_catalog_order() {
    let options = vec![
        option("first", "First", "first", "first", false, None),
        option("hidden", "Hidden", "hidden", "hidden", false, Some("no")),
        option("fast", "Fast", "fast", "fast", false, None),
        option("last", "Last", "last", "last", false, None),
    ];
    let available = available_speed_options(&options);
    let ids: Vec<&str> = available.iter().map(|speed| speed.id.as_str()).collect();

    assert_eq!(ids, ["first", "fast", "last"]);
    assert_eq!(available[0].description, "first");
    assert_eq!(available[1].description, "fast");
    assert_eq!(available[2].description, "last");
}

#[test]
fn display_order_is_input_order_even_when_it_is_not_rank_order() {
    let options = vec![
        option("superfast", "Superfast", "superfast", "super", false, None),
        option("standard", "Standard", "standard", "standard", true, None),
        option("fast", "Fast", "fast", "fast", false, None),
    ];
    let ids: Vec<&str> = available_speed_options(&options)
        .iter()
        .map(|speed| speed.id.as_str())
        .collect();

    assert_eq!(ids, ["superfast", "standard", "fast"]);
}

#[test]
fn default_selection_prefers_available_default_then_first_available() {
    let standard = standard_speeds();
    assert_eq!(
        default_speed(&standard).map(|speed| speed.id.as_str()),
        Some("standard")
    );

    let disabled_default = vec![
        option(
            "standard",
            "Standard",
            "standard",
            "standard",
            true,
            Some("retired"),
        ),
        option("fast", "Fast", "fast", "fast", false, None),
    ];
    assert_eq!(
        default_speed(&disabled_default).map(|speed| speed.id.as_str()),
        Some("fast")
    );

    let no_marked_default = vec![
        option("first", "First", "first", "first", false, None),
        option("second", "Second", "second", "second", false, None),
    ];
    assert_eq!(
        default_speed(&no_marked_default).map(|speed| speed.id.as_str()),
        Some("first")
    );
}

#[test]
fn default_selection_is_none_for_empty_or_entirely_unavailable_catalogs() {
    let empty: Vec<SpeedOption> = Vec::new();
    assert_eq!(default_speed(&empty), None);

    let disabled = vec![
        option(
            "standard",
            "Standard",
            "standard",
            "standard",
            true,
            Some("no"),
        ),
        option("fast", "Fast", "fast", "fast", false, Some("no")),
    ];
    assert_eq!(default_speed(&disabled), None);
}

#[test]
fn exact_id_and_native_value_lookups_are_case_sensitive_and_first_match_wins() {
    let options = vec![
        option(
            "fast",
            "First",
            "native-fast",
            "first",
            false,
            Some("disabled"),
        ),
        option("fast", "Second", "native-fast", "second", false, None),
    ];

    assert_eq!(
        speed_by_id(&options, "fast").map(|speed| speed.label.as_str()),
        Some("First")
    );
    assert_eq!(
        speed_by_native_value(&options, "native-fast").map(|speed| speed.label.as_str()),
        Some("First")
    );
    assert_eq!(speed_by_id(&options, "FAST"), None);
    assert_eq!(speed_by_native_value(&options, "NATIVE-FAST"), None);
    assert_eq!(speed_by_id(&options, "missing"), None);
    assert_eq!(speed_by_native_value(&options, "missing"), None);
}

#[test]
fn selected_id_uses_exact_available_value_then_default_fallback() {
    let options = standard_speeds();
    assert_eq!(
        selected_speed_by_id(&options, "fast").map(|speed| speed.id.as_str()),
        Some("fast")
    );
    assert_eq!(
        selected_speed_by_id(&options, "unknown").map(|speed| speed.id.as_str()),
        Some("standard")
    );
    assert_eq!(
        selected_speed_by_id(&options, "FAST").map(|speed| speed.id.as_str()),
        Some("standard")
    );

    let disabled = disabled_fast_speeds();
    assert_eq!(
        selected_speed_by_id(&disabled, "fast").map(|speed| speed.id.as_str()),
        Some("standard")
    );
}

#[test]
fn selected_native_value_uses_exact_available_value_then_default_fallback() {
    let options = standard_speeds();
    assert_eq!(
        selected_speed_by_native_value(&options, "fast").map(|speed| speed.id.as_str()),
        Some("fast")
    );
    assert_eq!(
        selected_speed_by_native_value(&options, "unknown").map(|speed| speed.id.as_str()),
        Some("standard")
    );
    assert_eq!(
        selected_speed_by_native_value(&options, "FAST").map(|speed| speed.id.as_str()),
        Some("standard")
    );

    let disabled = disabled_fast_speeds();
    assert_eq!(
        selected_speed_by_native_value(&disabled, "fast").map(|speed| speed.id.as_str()),
        Some("standard")
    );
    let all_disabled = vec![option(
        "standard",
        "Standard",
        "standard",
        "standard",
        true,
        Some("no"),
    )];
    assert_eq!(
        selected_speed_by_native_value(&all_disabled, "standard"),
        None
    );
}

#[test]
fn authoritative_sync_uses_raw_native_then_raw_default_then_standard() {
    let options = vec![
        option(
            "standard",
            "Standard",
            "standard",
            "standard",
            true,
            Some("currently unavailable"),
        ),
        option(
            "fast",
            "Fast",
            "fast",
            "fast",
            false,
            Some("currently unavailable"),
        ),
    ];

    assert_eq!(selected_speed_by_native_value(&options, "fast"), None);
    assert_eq!(authoritative_speed_id(&options, "fast"), "fast");
    assert_eq!(authoritative_speed_id(&options, "retired"), "standard");

    let multiple_defaults = vec![
        option(
            "first-default",
            "First default",
            "first-default",
            "first",
            true,
            Some("currently unavailable"),
        ),
        option(
            "second-default",
            "Second default",
            "second-default",
            "second",
            true,
            None,
        ),
    ];
    assert_eq!(
        authoritative_speed_id(&multiple_defaults, "retired"),
        "first-default"
    );

    let no_default = vec![option(
        "economy",
        "Economy",
        "economy",
        "economy",
        false,
        Some("currently unavailable"),
    )];
    assert_eq!(authoritative_speed_id(&no_default, "retired"), "standard");

    let no_standard_option: Vec<SpeedOption> = Vec::new();
    assert_eq!(
        authoritative_speed_id(&no_standard_option, "retired"),
        DEFAULT_SPEED_ID
    );
}

#[test]
fn trigger_hides_default_and_unknown_but_not_a_disabled_selected_value() {
    let options = standard_speeds();
    assert_eq!(trigger_speed_presentation(&options, "standard"), None);
    assert_eq!(trigger_speed_presentation(&options, "unknown"), None);
    assert_eq!(trigger_speed_presentation(&options, ""), None);

    let disabled = disabled_fast_speeds();
    assert_eq!(
        trigger_speed_presentation(&disabled, "fast"),
        Some(SpeedOptionPresentation {
            class_name: FAST_CLASS,
            label: FAST_LABEL.to_owned(),
        })
    );

    let empty: Vec<SpeedOption> = Vec::new();
    assert_eq!(trigger_speed_presentation(&empty, "fast"), None);
}

#[test]
fn trigger_uses_exact_branded_and_unknown_non_default_presentations() {
    let options = all_branded_speeds();
    assert_eq!(
        trigger_speed_presentation(&options, "fast"),
        Some(SpeedOptionPresentation {
            class_name: FAST_CLASS,
            label: FAST_LABEL.to_owned(),
        })
    );
    assert_eq!(
        trigger_speed_presentation(&options, "superfast"),
        Some(SpeedOptionPresentation {
            class_name: SUPERFAST_CLASS,
            label: SUPERFAST_LABEL.to_owned(),
        })
    );

    let custom = vec![
        option("standard", "Standard", "standard", "standard", true, None),
        option("economy", "Economy", "economy", "economy", false, None),
    ];
    assert_eq!(
        trigger_speed_presentation(&custom, "economy"),
        Some(SpeedOptionPresentation {
            class_name: "",
            label: "Economy".to_owned(),
        })
    );
}

#[test]
fn dispatch_hides_default_and_unknown_but_not_an_unavailable_value() {
    let options = standard_speeds();
    assert_eq!(dispatch_speed_presentation(&options, "standard"), None);
    assert_eq!(dispatch_speed_presentation(&options, "unknown"), None);
    assert_eq!(dispatch_speed_presentation(&options, ""), None);

    let disabled = disabled_fast_speeds();
    assert_eq!(
        dispatch_speed_presentation(&disabled, "fast"),
        Some(SpeedOptionPresentation {
            class_name: FAST_CLASS,
            label: FAST_LABEL.to_owned(),
        })
    );
}

#[test]
fn dispatch_uses_native_value_not_id_and_preserves_exact_custom_label() {
    let options = vec![
        option(
            "standard",
            "Standard",
            "native-default",
            "standard",
            true,
            None,
        ),
        option("fast", "Provider Fast", "native-fast", "fast", false, None),
        option(
            "custom",
            "Custom tier",
            "native-custom",
            "custom",
            false,
            None,
        ),
    ];

    assert_eq!(
        dispatch_speed_presentation(&options, "native-fast"),
        Some(SpeedOptionPresentation {
            class_name: "text-amber-600 dark:text-amber-400",
            label: "Fast".to_owned(),
        })
    );
    assert_eq!(
        dispatch_speed_presentation(&options, "native-custom"),
        Some(SpeedOptionPresentation {
            class_name: "",
            label: "Custom tier".to_owned(),
        })
    );
    assert_eq!(dispatch_speed_presentation(&options, "fast"), None);
}

#[test]
fn default_constants_preserve_the_standard_picker_and_wire_fallbacks() {
    assert_eq!(DEFAULT_SPEED_ID, "standard");
    assert_eq!(DEFAULT_NATIVE_VALUE, "standard");
}

#[test]
fn speed_policy_does_not_conflate_reasoning_effort_with_speed() {
    for effort in ["light", "medium", "high", "xhigh", "max", "ultra"] {
        let option = option(effort, effort, effort, "effort", false, None);
        assert_eq!(speed_option_presentation(&option).class_name, "");
        assert_eq!(speed_option_presentation(&option).label, effort);
    }
}

#[test]
fn exhaustive_selection_and_presentation_table_covers_all_states() {
    let options = all_branded_speeds();
    let cases = [
        ("standard", Some("standard"), None, None),
        ("fast", Some("fast"), Some(FAST_LABEL), Some(FAST_CLASS)),
        (
            "superfast",
            Some("superfast"),
            Some(SUPERFAST_LABEL),
            Some(SUPERFAST_CLASS),
        ),
        ("unknown", Some("standard"), None, None),
        ("", Some("standard"), None, None),
    ];

    for (input, expected_selection, expected_label, expected_class) in cases {
        let selected = selected_speed_by_id(&options, input).map(|speed| speed.id.as_str());
        assert_eq!(selected, expected_selection, "selection input={input:?}");

        let trigger = trigger_speed_presentation(&options, input);
        match (trigger, expected_label, expected_class) {
            (None, None, None) => {}
            (
                Some(SpeedOptionPresentation { class_name, label }),
                Some(expected_label),
                Some(expected_class),
            ) => {
                assert_eq!(class_name, expected_class, "input={input:?}");
                assert_eq!(label, expected_label, "input={input:?}");
            }
            (actual, label, class_name) => panic!(
                "unexpected trigger input={input:?}: actual={actual:?} expected={label:?}/{class_name:?}"
            ),
        }
    }
}
