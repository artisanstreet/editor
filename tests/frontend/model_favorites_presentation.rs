//! Dependency-free parity tests for model favorites presentation and recovery.
//!
//! The production module is included directly so this target needs no Cargo
//! graph, transport implementation, catalog decoder, or UI registration.

#[path = "../../modules/frontend/src/model_favorites_presentation.rs"]
mod model_favorites_presentation;

use model_favorites_presentation::{
    ModelChoiceView, UnstarAction, UnstarCompletion, present_model_favorites,
    project_model_favorites, resolve_unstar_completion, unstar_action, unstar_control_disabled,
    unstar_failed, unstar_succeeded,
};

fn model<'a>(
    id: &'a str,
    name: &'a str,
    lab: &'a str,
    provider: &'a str,
    variant_id: Option<&'a str>,
) -> ModelChoiceView<'a> {
    ModelChoiceView::new(id, name, lab, provider, variant_id)
}

#[test]
fn empty_projection_exposes_empty_state_and_unavailable_control() {
    let models = [model("only-model", "Only model", "Lab", "provider", None)];
    let favorite_ids: [&str; 0] = [];

    let presentation = present_model_favorites(&models, favorite_ids, false);

    assert!(presentation.favorites.is_empty());
    assert!(presentation.is_empty());
    assert!(!presentation.forge_available);
    assert!(presentation.unstar_disabled());
    assert!(unstar_control_disabled(false));
}

#[test]
fn unknown_ids_are_dropped_while_duplicates_and_favorite_order_survive() {
    let models = [
        model("catalog-first", "First", "Lab A", "provider-a", None),
        model("catalog-second", "Second", "Lab B", "provider-b", None),
        model("catalog-third", "Third", "Lab C", "provider-c", None),
    ];
    let favorite_ids = [
        "missing",
        "catalog-third",
        "catalog-third",
        "catalog-first",
        "also-missing",
        "catalog-second",
    ];

    let favorites = project_model_favorites(&favorite_ids, &models);

    assert_eq!(
        favorites,
        vec![models[2], models[2], models[0], models[1]],
        "the map/find/filter projection must not deduplicate or sort"
    );
}

#[test]
fn first_matching_catalog_entry_wins_without_reordering_the_favorite_ids() {
    let first = model("duplicate", "First catalog row", "First lab", "first", None);
    let second = model(
        "duplicate",
        "Second catalog row",
        "Second lab",
        "second",
        None,
    );
    let third = model("other", "Other", "Other lab", "other", None);
    let models = [first, second, third];
    let favorite_ids = ["other", "duplicate"];

    let favorites = project_model_favorites(&favorite_ids, &models);

    assert_eq!(favorites, vec![third, first]);
}

#[test]
fn exact_unicode_display_fields_and_optional_variants_are_borrowed_unchanged() {
    let id = "model/é 🦀";
    let name = "  GPT — 東京\n🪐  ";
    let lab = "Láb · 数据";
    let provider = "provider/ß";
    let variant_id = "  native_変_01  ";
    let variant = model(id, name, lab, provider, Some(variant_id));
    let base = model("base", "Base", "Base lab", "base-provider", None);
    let models = [base, variant];
    let favorite_ids = ["model/é 🦀", "base"];

    let favorites = project_model_favorites(&favorite_ids, &models);

    assert_eq!(favorites[0].id, id);
    assert_eq!(favorites[0].name, name);
    assert_eq!(favorites[0].lab, lab);
    assert_eq!(favorites[0].provider, provider);
    assert_eq!(favorites[0].variant_id, Some(variant_id));
    assert_eq!(favorites[0].native_variant_id(), Some(variant_id));
    assert_eq!(favorites[1].variant_id, None);
    assert_eq!(favorites[0].name.as_ptr(), name.as_ptr());
    assert_eq!(
        favorites[0].variant_id.map(str::as_ptr),
        Some(variant_id.as_ptr())
    );
}

#[test]
fn presentation_clones_and_compares_while_preserving_borrowed_models() {
    let choice = model("model", "Exact name", "Exact lab", "exact-provider", None);
    let models = [choice];
    let favorite_ids = [String::from("model")];
    let presentation = present_model_favorites(&models, &favorite_ids, true);
    let clone = presentation.clone();

    assert_eq!(presentation, clone);
    assert!(!presentation.is_empty());
    assert!(presentation.forge_available);
    assert!(!presentation.unstar_disabled());
    assert_eq!(
        presentation.favorites[0].name.as_ptr(),
        choice.name.as_ptr()
    );
}

#[test]
fn available_unstar_emits_exact_model_id_and_false_favorite_value() {
    let action = unstar_action(true, "model/with exact id");

    assert_eq!(
        action,
        UnstarAction::Request {
            model_id: "model/with exact id",
            favorite: false,
        }
    );
    assert_eq!(action.request(), Some(("model/with exact id", false)));
    assert!(!action.is_no_op());
}

#[test]
fn unavailable_unstar_is_a_no_op_and_never_constructs_a_request() {
    let action = unstar_action(false, "model-that-must-not-be-sent");

    assert_eq!(action, UnstarAction::NoOp);
    assert!(action.is_no_op());
    assert_eq!(action.request(), None);
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DefaultsState {
    favorite_ids: Vec<String>,
    revision: u32,
}

#[test]
fn successful_unstar_replaces_defaults_with_the_returned_state_exactly() {
    let returned = DefaultsState {
        favorite_ids: vec![String::from("remaining")],
        revision: 7,
    };
    let expected = returned.clone();

    let completion = resolve_unstar_completion::<_, ()>(Ok(returned));

    assert_eq!(completion, unstar_succeeded(expected.clone()));
    assert_eq!(completion.applied_state(), Some(&expected));
    assert!(!completion.requests_current_refresh());
    assert_eq!(unstar_succeeded(expected), completion);
}

#[test]
fn failed_unstar_requires_a_fresh_current_replacement() {
    let completion = resolve_unstar_completion::<DefaultsState, _>(Err("request failed"));

    assert_eq!(completion, unstar_failed());
    assert_eq!(completion, UnstarCompletion::RefreshCurrent);
    assert_eq!(completion.applied_state(), None);
    assert!(completion.requests_current_refresh());
}

#[test]
fn completion_values_are_cloneable_and_failure_does_not_retain_stale_state() {
    let success = unstar_succeeded(DefaultsState {
        favorite_ids: vec![String::from("old")],
        revision: 1,
    });
    let success_clone = success.clone();
    let refresh = unstar_failed::<DefaultsState>();

    assert_eq!(success, success_clone);
    assert_eq!(refresh, UnstarCompletion::RefreshCurrent);
    assert_ne!(success, refresh);
}
