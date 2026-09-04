//! Focused parity tests for the pure composer send-readiness policy.

#[path = "../../modules/frontend/src/composer_send_readiness.rs"]
mod composer_send_readiness;

use composer_send_readiness::{
    ContextWindowCapability, ContextWindowOption, HarnessDefinition, ModelCapabilities,
    ModelDefinition, NativeSelection, RuntimeCatalog, SurfaceUsageAggregate, ThreadSessionPolicy,
    UsageOrigin, composer_context_usage_is_current, composer_context_window_tokens,
    composer_send_blocked_reason,
};

fn catalog<'a>(
    harnesses: &'a [HarnessDefinition<'a>],
    models: &'a [ModelDefinition<'a>],
    runnable_harness_ids: &'a [&'a str],
) -> RuntimeCatalog<'a> {
    RuntimeCatalog::new(harnesses, models, runnable_harness_ids)
}

fn policy(engine_id: &'static str, model: Option<&'static str>) -> ThreadSessionPolicy<'static> {
    ThreadSessionPolicy::new(engine_id, model)
}

fn usage(
    engine_id: Option<&'static str>,
    model_id: Option<&'static str>,
    context_window_tokens: Option<u64>,
) -> SurfaceUsageAggregate<'static> {
    SurfaceUsageAggregate::new(
        Some(UsageOrigin::new(engine_id, model_id)),
        context_window_tokens,
    )
}

#[test]
fn offline_forge_wins_with_the_exact_message() {
    let harnesses = [HarnessDefinition::new("codex", "Codex")];
    let models = [];
    let runnable = ["codex"];
    let catalog = catalog(&harnesses, &models, &runnable);
    let selected = policy("unknown", Some("model"));

    assert_eq!(
        composer_send_blocked_reason(false, &catalog, Some(&selected)),
        Some("Forge is offline — reconnect to send".to_owned())
    );
}

#[test]
fn absent_policy_engine_allows_send_when_forge_is_available() {
    let harnesses = [];
    let models = [];
    let runnable = [];
    let catalog = catalog(&harnesses, &models, &runnable);

    assert_eq!(composer_send_blocked_reason(true, &catalog, None), None);
}

#[test]
fn runnable_harness_allows_send_even_when_catalog_label_is_present() {
    let harnesses = [HarnessDefinition::new("codex", "Codex")];
    let models = [];
    let runnable = ["codex"];
    let catalog = catalog(&harnesses, &models, &runnable);
    let selected = policy("codex", Some("gpt-5"));

    assert_eq!(
        composer_send_blocked_reason(true, &catalog, Some(&selected)),
        None
    );
}

#[test]
fn non_runnable_known_harness_uses_its_catalog_label() {
    let harnesses = [HarnessDefinition::new("claude", "Claude Code")];
    let models = [];
    let runnable = [];
    let catalog = catalog(&harnesses, &models, &runnable);
    let selected = policy("claude", Some("claude-fable-5"));

    assert_eq!(
        composer_send_blocked_reason(true, &catalog, Some(&selected)),
        Some(
            "Claude Code models are preview-only — this engine cannot run in Artisan yet"
                .to_owned()
        )
    );
}

#[test]
fn non_runnable_unknown_harness_falls_back_to_its_id() {
    let harnesses = [HarnessDefinition::new("codex", "Codex")];
    let models = [];
    let runnable = [];
    let catalog = catalog(&harnesses, &models, &runnable);
    let selected = policy("hermes", Some("hermes-model"));

    assert_eq!(
        composer_send_blocked_reason(true, &catalog, Some(&selected)),
        Some("hermes models are preview-only — this engine cannot run in Artisan yet".to_owned())
    );
}

#[test]
fn usage_matching_requires_an_origin_and_matching_engine() {
    let selected = policy("claude", Some("claude-fable-5"));

    assert!(!composer_context_usage_is_current(None, None));
    assert!(!composer_context_usage_is_current(Some(&selected), None));
    assert!(!composer_context_usage_is_current(
        Some(&selected),
        Some(&SurfaceUsageAggregate::new(None, Some(1)))
    ));
    assert!(!composer_context_usage_is_current(
        Some(&selected),
        Some(&usage(Some("codex"), Some("claude-fable-5"), Some(1)))
    ));
    assert!(!composer_context_usage_is_current(
        Some(&selected),
        Some(&usage(Some("claude"), Some("claude-sonnet-5"), Some(1)))
    ));
    assert!(!composer_context_usage_is_current(
        Some(&selected),
        Some(&usage(None, Some("claude-fable-5"), Some(1)))
    ));
}

#[test]
fn matching_engine_with_missing_origin_model_is_current() {
    let selected = policy("claude", Some("claude-fable-5"));

    assert!(composer_context_usage_is_current(
        Some(&selected),
        Some(&usage(Some("claude"), None, Some(1)))
    ));
}

#[test]
fn matching_engine_and_model_origin_is_current_but_stale_model_is_not() {
    let selected = policy("claude", Some("claude-fable-5"));

    assert!(composer_context_usage_is_current(
        Some(&selected),
        Some(&usage(Some("claude"), Some("claude-fable-5"), Some(1)))
    ));
    assert!(!composer_context_usage_is_current(
        Some(&selected),
        Some(&usage(Some("claude"), Some("claude-opus-5"), Some(1)))
    ));
}

#[test]
fn current_wire_window_takes_precedence_over_catalog_options() {
    let harnesses = [HarnessDefinition::new("claude", "Claude")];
    let options = [
        ContextWindowOption::new("standard", "", 200_000),
        ContextWindowOption::new("extended", "[1m]", 1_000_000),
    ];
    let capability = ContextWindowCapability::new("extended", &options);
    let models = [ModelDefinition::new(
        "claude",
        "claude-fable-5",
        None,
        ModelCapabilities::new(None, Some(capability)),
    )];
    let runnable = [];
    let catalog = catalog(&harnesses, &models, &runnable);
    let selected = policy("claude", Some("claude-fable-5"));

    assert_eq!(
        composer_context_window_tokens(
            &catalog,
            Some(&selected),
            Some(&usage(
                Some("claude"),
                Some("claude-fable-5"),
                Some(314_159)
            ))
        ),
        Some(314_159)
    );
}

#[test]
fn stale_wire_window_falls_back_to_the_exact_catalog_model() {
    let harnesses = [HarnessDefinition::new("claude", "Claude")];
    let options = [
        ContextWindowOption::new("standard", "", 200_000),
        ContextWindowOption::new("extended", "[1m]", 1_000_000),
    ];
    let capability = ContextWindowCapability::new("extended", &options);
    let models = [ModelDefinition::new(
        "claude",
        "claude-fable-5",
        None,
        ModelCapabilities::new(None, Some(capability)),
    )];
    let runnable = [];
    let catalog = catalog(&harnesses, &models, &runnable);
    let selected = policy("claude", Some("claude-fable-5"));

    assert_eq!(
        composer_context_window_tokens(
            &catalog,
            Some(&selected),
            Some(&usage(Some("codex"), Some("gpt-5"), Some(314_159)))
        ),
        Some(1_000_000)
    );
}

#[test]
fn absent_wire_window_uses_the_declared_default_not_the_first_option() {
    let harnesses = [HarnessDefinition::new("claude", "Claude")];
    let options = [
        ContextWindowOption::new("standard", "", 200_000),
        ContextWindowOption::new("extended", "[1m]", 1_000_000),
    ];
    let capability = ContextWindowCapability::new("extended", &options);
    let models = [ModelDefinition::new(
        "claude",
        "claude-fable-5",
        None,
        ModelCapabilities::new(None, Some(capability)),
    )];
    let runnable = [];
    let catalog = catalog(&harnesses, &models, &runnable);
    let selected = policy("claude", Some("claude-fable-5"));

    assert_eq!(
        composer_context_window_tokens(&catalog, Some(&selected), None),
        Some(1_000_000)
    );
}

#[test]
fn requested_suffix_selects_its_option_and_unknown_suffix_uses_default() {
    let harnesses = [HarnessDefinition::new("claude", "Claude")];
    let options = [
        ContextWindowOption::new("standard", "", 200_000),
        ContextWindowOption::new("extended", "[1m]", 1_000_000),
    ];
    let capability = ContextWindowCapability::new("extended", &options);
    let models = [ModelDefinition::new(
        "claude",
        "claude-fable-5",
        None,
        ModelCapabilities::new(None, Some(capability)),
    )];
    let runnable = [];
    let catalog = catalog(&harnesses, &models, &runnable);

    let standard = policy("claude", Some("claude-fable-5")).with_context_window(Some(""));
    assert_eq!(
        composer_context_window_tokens(&catalog, Some(&standard), None),
        Some(200_000)
    );

    let unknown = policy("claude", Some("claude-fable-5")).with_context_window(Some("[unknown]"));
    assert_eq!(
        composer_context_window_tokens(&catalog, Some(&unknown), None),
        Some(1_000_000)
    );
}

#[test]
fn legacy_scalar_is_used_when_exact_model_has_no_configurable_capability() {
    let harnesses = [HarnessDefinition::new("codex", "Codex")];
    let models = [ModelDefinition::new(
        "codex",
        "gpt-5",
        None,
        ModelCapabilities::new(Some(272_000), None),
    )];
    let runnable = ["codex"];
    let catalog = catalog(&harnesses, &models, &runnable);
    let selected = policy("codex", Some("gpt-5"));

    assert_eq!(
        composer_context_window_tokens(&catalog, Some(&selected), None),
        Some(272_000)
    );
}

#[test]
fn missing_capability_or_policy_produces_no_window() {
    let harnesses = [HarnessDefinition::new("claude", "Claude")];
    let no_capability = [ModelDefinition::new(
        "claude",
        "claude-haiku-5",
        None,
        ModelCapabilities::new(None, None),
    )];
    let runnable = [];
    let catalog = catalog(&harnesses, &no_capability, &runnable);

    assert_eq!(
        composer_context_window_tokens(
            &catalog,
            Some(&policy("claude", Some("claude-haiku-5"))),
            None
        ),
        None
    );
    assert_eq!(composer_context_window_tokens(&catalog, None, None), None);
    assert_eq!(
        composer_context_window_tokens(
            &catalog,
            Some(&policy("claude", Some("missing-model"))),
            None
        ),
        None
    );
}

#[test]
fn missing_declared_default_does_not_silently_choose_first_option() {
    let harnesses = [HarnessDefinition::new("claude", "Claude")];
    let options = [ContextWindowOption::new("standard", "", 200_000)];
    let capability = ContextWindowCapability::new("missing", &options);
    let models = [ModelDefinition::new(
        "claude",
        "claude-fable-5",
        None,
        ModelCapabilities::new(Some(99), Some(capability)),
    )];
    let runnable = [];
    let catalog = catalog(&harnesses, &models, &runnable);
    let selected = policy("claude", Some("claude-fable-5"));

    assert_eq!(
        composer_context_window_tokens(&catalog, Some(&selected), None),
        None
    );
}

#[test]
fn route_and_variant_selection_disambiguates_catalog_models() {
    let harnesses = [HarnessDefinition::new("cursor", "Cursor")];
    let generic_options = [ContextWindowOption::new("generic", "", 111)];
    let route_options = [ContextWindowOption::new("route", "", 222)];
    let variant_options = [ContextWindowOption::new("variant", "", 333)];
    let models = [
        // This generic model must not match a route-aware policy.
        ModelDefinition::new(
            "cursor",
            "claude-sonnet-5",
            None,
            ModelCapabilities::new(
                None,
                Some(ContextWindowCapability::new("generic", &generic_options)),
            ),
        ),
        ModelDefinition::new(
            "cursor",
            "claude-sonnet-5",
            Some(NativeSelection::new("gateway-a", "sonnet", None)),
            ModelCapabilities::new(
                None,
                Some(ContextWindowCapability::new("route", &route_options)),
            ),
        ),
        ModelDefinition::new(
            "cursor",
            "claude-sonnet-5",
            Some(NativeSelection::new("gateway-a", "sonnet", Some("fast"))),
            ModelCapabilities::new(
                None,
                Some(ContextWindowCapability::new("variant", &variant_options)),
            ),
        ),
    ];
    let runnable = [];
    let catalog = catalog(&harnesses, &models, &runnable);

    let route_policy = policy("cursor", Some("claude-sonnet-5")).with_native_selection(
        Some("gateway-a"),
        Some("sonnet"),
        None,
    );
    assert_eq!(
        composer_context_window_tokens(&catalog, Some(&route_policy), None),
        Some(222)
    );

    let variant_policy =
        route_policy.with_native_selection(Some("gateway-a"), Some("sonnet"), Some("fast"));
    assert_eq!(
        composer_context_window_tokens(&catalog, Some(&variant_policy), None),
        Some(333)
    );
}

#[test]
fn native_selection_uses_policy_model_id_before_catalog_model_name() {
    let harnesses = [HarnessDefinition::new("cursor", "Cursor")];
    let options = [ContextWindowOption::new("route", "", 444)];
    let models = [ModelDefinition::new(
        "cursor",
        "display-model",
        Some(NativeSelection::new("provider-route", "native-model", None)),
        ModelCapabilities::new(None, Some(ContextWindowCapability::new("route", &options))),
    )];
    let runnable = [];
    let catalog = catalog(&harnesses, &models, &runnable);
    let selected = policy("cursor", Some("display-model")).with_native_selection(
        Some("provider-route"),
        Some("native-model"),
        None,
    );

    assert_eq!(
        composer_context_window_tokens(&catalog, Some(&selected), None),
        Some(444)
    );

    let wrong_model =
        selected.with_native_selection(Some("provider-route"), Some("display-model"), None);
    assert_eq!(
        composer_context_window_tokens(&catalog, Some(&wrong_model), None),
        None
    );
}

#[test]
fn unqualified_model_requires_unqualified_policy_selection() {
    let harnesses = [HarnessDefinition::new("cursor", "Cursor")];
    let options = [ContextWindowOption::new("generic", "", 111)];
    let models = [ModelDefinition::new(
        "cursor",
        "model",
        None,
        ModelCapabilities::new(
            None,
            Some(ContextWindowCapability::new("generic", &options)),
        ),
    )];
    let runnable = [];
    let catalog = catalog(&harnesses, &models, &runnable);

    let unqualified = policy("cursor", Some("model"));
    assert_eq!(
        composer_context_window_tokens(&catalog, Some(&unqualified), None),
        Some(111)
    );

    let qualified = unqualified.with_native_selection(Some("route"), Some("model"), None);
    assert_eq!(
        composer_context_window_tokens(&catalog, Some(&qualified), None),
        None
    );
}

#[test]
fn first_exact_duplicate_model_wins_in_manifest_order() {
    let harnesses = [HarnessDefinition::new("claude", "Claude")];
    let first_options = [ContextWindowOption::new("first", "", 1)];
    let second_options = [ContextWindowOption::new("second", "", 2)];
    let models = [
        ModelDefinition::new(
            "claude",
            "model",
            None,
            ModelCapabilities::new(
                None,
                Some(ContextWindowCapability::new("first", &first_options)),
            ),
        ),
        ModelDefinition::new(
            "claude",
            "model",
            None,
            ModelCapabilities::new(
                None,
                Some(ContextWindowCapability::new("second", &second_options)),
            ),
        ),
    ];
    let runnable = [];
    let catalog = catalog(&harnesses, &models, &runnable);
    let selected = policy("claude", Some("model"));

    assert_eq!(
        composer_context_window_tokens(&catalog, Some(&selected), None),
        Some(1)
    );
}
