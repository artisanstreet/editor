//! Direct parity tests for the dependency-free context gauge policy.

#![allow(dead_code)]

#[path = "../../modules/frontend/src/context_usage_gauge_policy.rs"]
mod context_usage_gauge_policy;

use std::time::Duration;

use context_usage_gauge_policy::{
    CONTEXT_USAGE_CLOSE_DELAY, CONTEXT_USAGE_DESCRIPTION_ID, CONTEXT_USAGE_MAX_WIDTH_CLASS,
    CONTEXT_USAGE_OPEN_DELAY, CONTEXT_USAGE_SIDE_OFFSET_PX, CONTEXT_USAGE_WIDTH_CLASS,
    ContextUsageDetailsInput, ContextUsageGaugeInput, ContextUsageRingInput, GaugeControlOwnership,
    HoverCardAlign, HoverCardMaterial, HoverCardPresentation, HoverCardSide,
    project_context_usage_gauge,
};

fn input<'a>(
    description: &'a str,
    percent: f64,
    compaction_percent: f64,
    model_name: Option<&'a str>,
    window_tokens: f64,
) -> ContextUsageGaugeInput<'a> {
    ContextUsageGaugeInput::new(
        description,
        percent,
        compaction_percent,
        model_name,
        window_tokens,
    )
}

#[test]
fn ring_and_details_inputs_forward_each_value_without_recomputation() {
    let description = "already computed context description";
    let projection = project_context_usage_gauge(input(
        description,
        73.25,
        91.5,
        Some("GPT-5.6 Luna"),
        128_000.5,
    ));

    assert_eq!(
        projection.ring,
        ContextUsageRingInput {
            compaction_percent: 91.5,
            percent: 73.25,
        }
    );
    assert_eq!(
        projection.details,
        ContextUsageDetailsInput {
            model_name: Some("GPT-5.6 Luna"),
            percent: 73.25,
            window_tokens: 128_000.5,
        }
    );
    assert_eq!(projection.description.text, description);
    assert_eq!(projection.description.text.as_ptr(), description.as_ptr());
}

#[test]
fn accessible_label_rounds_raw_percent_like_javascript_math_round() {
    let cases = [
        (49.4, 49.0, "Context window 49% full"),
        (49.5, 50.0, "Context window 50% full"),
        (74.5, 75.0, "Context window 75% full"),
        (-10.5, -10.0, "Context window -10% full"),
        (100.5, 101.0, "Context window 101% full"),
    ];

    for (percent, expected_rounded, expected_label) in cases {
        let projection =
            project_context_usage_gauge(input("description", percent, 90.0, None, 1.0));
        assert_eq!(projection.trigger.label_percent, expected_rounded);
        assert_eq!(projection.trigger.aria_label, expected_label);
    }
}

#[test]
fn description_is_persistent_and_described_by_remains_present_when_card_is_closed() {
    let projection = project_context_usage_gauge(input(
        "Context window contains 3,000 of 10,000 tokens.",
        30.0,
        90.0,
        None,
        10_000.0,
    ));

    assert_eq!(
        projection.trigger.aria_described_by,
        CONTEXT_USAGE_DESCRIPTION_ID
    );
    assert_eq!(projection.description.id, CONTEXT_USAGE_DESCRIPTION_ID);
    assert!(projection.description.always_present);
    assert!(projection.description.visually_hidden);
    assert_eq!(
        projection.description.text,
        "Context window contains 3,000 of 10,000 tokens."
    );
}

#[test]
fn timing_placement_width_and_glass_material_are_typed_constants() {
    let presentation = HoverCardPresentation::CONTEXT_USAGE;

    assert_eq!(CONTEXT_USAGE_OPEN_DELAY, Duration::ZERO);
    assert_eq!(CONTEXT_USAGE_CLOSE_DELAY, Duration::from_millis(120));
    assert_eq!(presentation.open_delay, Duration::ZERO);
    assert_eq!(presentation.close_delay, Duration::from_millis(120));
    assert_eq!(presentation.placement.side, HoverCardSide::Top);
    assert_eq!(presentation.placement.align, HoverCardAlign::Start);
    assert_eq!(
        presentation.placement.side_offset_px,
        CONTEXT_USAGE_SIDE_OFFSET_PX
    );
    assert_eq!(presentation.width.preferred_rem, 18);
    assert_eq!(
        presentation.width.utility_class(),
        CONTEXT_USAGE_WIDTH_CLASS
    );
    assert_eq!(presentation.max_width.maximum_rem, 20);
    assert_eq!(presentation.max_width.viewport_inset_rem, 2);
    assert_eq!(
        presentation.max_width.utility_class(),
        CONTEXT_USAGE_MAX_WIDTH_CLASS
    );
    assert_eq!(presentation.material, HoverCardMaterial::ShaderGlassSurface);
    assert!(presentation.material.is_glass_surface());
}

#[test]
fn gauge_is_an_independent_sibling_control_without_nested_button_modeling() {
    let projection = project_context_usage_gauge(input("description", 50.0, 90.0, None, 1.0));

    assert_eq!(
        projection.trigger.ownership,
        GaugeControlOwnership::IndependentSibling
    );
    assert!(projection.trigger.ownership.is_independent_sibling());
}

#[test]
fn optional_reporting_model_name_is_kept_in_details_custody() {
    let none = project_context_usage_gauge(input("description", 50.0, 90.0, None, 1.0));
    assert_eq!(none.details.model_name, None);

    let empty = project_context_usage_gauge(input("description", 50.0, 90.0, Some(""), 1.0));
    assert_eq!(empty.details.model_name, Some(""));

    let name = String::from("Reporting model");
    let named =
        project_context_usage_gauge(input("description", 50.0, 90.0, Some(name.as_str()), 1.0));
    assert_eq!(named.details.model_name, Some(name.as_str()));
    assert_eq!(
        named.details.model_name.expect("name is present").as_ptr(),
        name.as_ptr()
    );
}

#[test]
fn non_finite_label_inputs_are_deterministic_without_affecting_forwarded_values() {
    let nan = project_context_usage_gauge(input("description", f64::NAN, f64::INFINITY, None, 1.0));
    assert!(nan.trigger.label_percent.is_nan());
    assert_eq!(nan.trigger.aria_label, "Context window NaN% full");
    assert!(nan.ring.percent.is_nan());
    assert_eq!(nan.ring.compaction_percent, f64::INFINITY);

    let infinity = project_context_usage_gauge(input(
        "description",
        f64::INFINITY,
        f64::NEG_INFINITY,
        None,
        f64::NAN,
    ));
    assert_eq!(infinity.trigger.aria_label, "Context window Infinity% full");
    assert_eq!(infinity.ring.compaction_percent, f64::NEG_INFINITY);
    assert!(infinity.details.window_tokens.is_nan());
}
