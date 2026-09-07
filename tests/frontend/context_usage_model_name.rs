//! Focused parity tests for the context-usage model-name resolver.
//!
//! The cases mirror the reached TypeScript leaf and deliberately use borrowed
//! catalog names so the tests also pin the resolver's stable return lifetime.

#[path = "../../modules/frontend/src/context_usage_model_name.rs"]
mod context_usage_model_name;

use context_usage_model_name::{
    ContextOriginView, ModelDefinitionView, RuntimeCatalogView, SurfaceUsageAggregateView,
    context_usage_model_name,
};

fn usage<'a>(
    engine_id: Option<&'a str>,
    model_id: Option<&'a str>,
) -> SurfaceUsageAggregateView<'a> {
    SurfaceUsageAggregateView::new(Some(ContextOriginView::new(engine_id, model_id)))
}

fn catalog<'models, 'fields>(
    models: &'models [ModelDefinitionView<'fields>],
) -> RuntimeCatalogView<'models, 'fields> {
    RuntimeCatalogView::new(models)
}

#[test]
fn incomplete_origin_returns_no_name() {
    let models = [ModelDefinitionView::new("codex", "gpt-5.6-luna", "Luna")];
    let empty = SurfaceUsageAggregateView::new(None);

    assert_eq!(context_usage_model_name(empty, catalog(&models)), None);
    assert_eq!(
        context_usage_model_name(usage(None, Some("gpt-5.6-luna")), catalog(&models)),
        None
    );
    assert_eq!(
        context_usage_model_name(usage(Some("codex"), None), catalog(&models)),
        None
    );
    assert_eq!(
        context_usage_model_name(usage(None, None), catalog(&models)),
        None
    );
}

#[test]
fn engine_mismatch_does_not_match_native_model_id_alone() {
    let models = [ModelDefinitionView::new("claude", "gpt-5.6-luna", "Claude")];

    assert_eq!(
        context_usage_model_name(usage(Some("codex"), Some("gpt-5.6-luna")), catalog(&models)),
        None
    );
}

#[test]
fn model_mismatch_does_not_match_engine_alone() {
    let models = [ModelDefinitionView::new("codex", "gpt-5.6-sol", "Sol")];

    assert_eq!(
        context_usage_model_name(usage(Some("codex"), Some("gpt-5.6-luna")), catalog(&models)),
        None
    );
}

#[test]
fn exact_engine_and_native_model_match_returns_catalog_name() {
    let models = [ModelDefinitionView::new(
        "codex",
        "gpt-5.6-luna",
        "GPT-5.6 Luna",
    )];

    assert_eq!(
        context_usage_model_name(usage(Some("codex"), Some("gpt-5.6-luna")), catalog(&models)),
        Some("GPT-5.6 Luna")
    );
}

#[test]
fn matching_duplicates_keep_the_first_manifest_entry() {
    let models = [
        ModelDefinitionView::new("claude", "gpt-5.6-luna", "Other engine"),
        ModelDefinitionView::new("codex", "gpt-5.6-luna", "First catalog name"),
        ModelDefinitionView::new("codex", "gpt-5.6-luna", "Later duplicate name"),
    ];

    assert_eq!(
        context_usage_model_name(usage(Some("codex"), Some("gpt-5.6-luna")), catalog(&models)),
        Some("First catalog name")
    );
}

#[test]
fn manifest_order_is_preserved_when_matching_entries_are_reordered() {
    let first = ModelDefinitionView::new("codex", "gpt-5.6-luna", "First position");
    let second = ModelDefinitionView::new("codex", "gpt-5.6-luna", "Second position");
    let models = [second, first];

    assert_eq!(
        context_usage_model_name(usage(Some("codex"), Some("gpt-5.6-luna")), catalog(&models)),
        Some("Second position")
    );
}

#[test]
fn returned_name_is_the_stable_catalog_borrow() {
    let first_name = String::from("name with exact case and spacing");
    let second_name = String::from("must not be selected");
    let models = [
        ModelDefinitionView::new("codex", "gpt-5.6-luna", first_name.as_str()),
        ModelDefinitionView::new("codex", "gpt-5.6-luna", second_name.as_str()),
    ];
    let runtime_catalog = catalog(&models);
    let selected =
        context_usage_model_name(usage(Some("codex"), Some("gpt-5.6-luna")), runtime_catalog)
            .expect("exact origin should resolve");

    assert_eq!(selected, first_name.as_str());
    assert_eq!(selected.as_ptr(), first_name.as_ptr());
    assert_eq!(
        context_usage_model_name(usage(Some("codex"), Some("gpt-5.6-luna")), runtime_catalog),
        Some(first_name.as_str())
    );
}
