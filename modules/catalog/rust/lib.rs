//! Shared model catalog schema and runtime admission policy.
//! Models are supplied by host discovery. Only harness permission descriptors
//! are shipped locally; disconnected clients have no fallback model list.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

pub mod wire;

mod manifest;
mod policy;
mod validation;

pub use manifest::*;
pub use policy::*;

/// The shipped harness descriptor file every catalog is built on.
pub const NATIVE_MODEL_CATALOG_SOURCE: &str = "modules/catalog/rust/native_harnesses.json";
/// The revision of the shipped harness descriptors. Snapshots published under
/// any other revision, including those that still carried bundled models,
/// fail wire validation.
pub const NATIVE_MODEL_CATALOG_REVISION: &str = "2026-09-25.1";
/// Harness permissions and provider descriptors; contains no model catalog.
pub const NATIVE_HARNESS_MANIFEST_JSON: &str = include_str!("native_harnesses.json");

#[cfg(test)]
mod tests {
    use super::*;

    fn offline() -> NativeModelCatalog {
        NativeModelCatalog::from_manifest_json(include_str!(
            "../../../tests/fixtures/model_catalog.json"
        ))
        .expect("the discovery fixture must decode")
    }

    #[test]
    fn shipped_catalog_has_no_models() {
        let catalog = NativeModelCatalog::harnesses_only().unwrap();
        assert!(catalog.manifest.models.is_empty());
        assert!(catalog.default_model_id.is_none());
        assert!(catalog.runnable_harness_ids.is_empty());
        wire::encode_catalog(&catalog).unwrap();
    }

    #[test]
    fn shipped_harness_manifest_rejects_models() {
        let json = NATIVE_HARNESS_MANIFEST_JSON.replace(
            "\"models\": []",
            &format!(
                "\"models\": [{}]",
                serde_json::to_string(
                    &serde_json::from_str::<serde_json::Value>(include_str!(
                        "../../../tests/fixtures/model_catalog.json"
                    ))
                    .unwrap()["models"][0]
                )
                .unwrap()
            ),
        );
        assert!(NativeModelCatalog::from_harness_manifest_json(&json).is_err());
    }

    #[test]
    fn context_refresh_preserves_extended_intent_and_updates_native_limit() {
        let mut catalog = offline();
        let old = catalog.selection_policy_for_model("codex-astra").unwrap();
        catalog.catalog_revision = "host-refresh".to_owned();
        let model = catalog
            .manifest
            .models
            .iter_mut()
            .find(|model| model.id == "codex-astra")
            .unwrap();
        let option = model
            .capabilities
            .context_window
            .as_mut()
            .unwrap()
            .options
            .iter_mut()
            .find(|option| option.id == "extended")
            .unwrap();
        option.tokens = 872_000;
        option.label = "872K".to_owned();
        option.native_config.as_mut().unwrap().model_context_window = 872_000;
        let rebased = catalog.rebase_policy(&old).unwrap();
        assert_eq!(rebased.catalog_revision, "host-refresh");
        assert_eq!(
            rebased
                .context_window
                .unwrap()
                .native_config
                .unwrap()
                .model_context_window,
            872_000
        );
    }

    #[test]
    fn fixture_snapshot_preserves_manifest_provenance_and_shape() {
        let catalog = offline();
        assert_eq!(catalog.provenance.source, NATIVE_MODEL_CATALOG_SOURCE);
        assert_eq!(catalog.provenance.revision, NATIVE_MODEL_CATALOG_REVISION);
        assert_eq!(catalog.manifest.revision, NATIVE_MODEL_CATALOG_REVISION);
        assert_eq!(catalog.manifest.providers.len(), 16);
        assert_eq!(catalog.manifest.harnesses.len(), 5);
        assert_eq!(catalog.manifest.models.len(), 38);
        assert!(catalog.runnable_harness_ids.is_empty());
        assert!(catalog.routes.is_empty());
    }

    #[test]
    fn astra_defaults_to_extended_context_while_56_keeps_standard() {
        let catalog = offline();
        let astra = catalog.selection_policy_for_model("codex-astra").unwrap();
        let context = astra.context_window.unwrap();
        assert_eq!(context.id, "extended");
        assert_eq!(
            context.native_config.unwrap().model_context_window,
            1_050_000
        );
        for model in catalog.manifest.models.iter().filter(|model| {
            model.harness == "codex" && model.native_model_id.starts_with("gpt-5.6")
        }) {
            let policy = catalog.selection_policy_for_model(&model.id).unwrap();
            assert_eq!(policy.context_window.unwrap().id, "standard");
        }
    }

    #[test]
    fn offline_models_are_readable_but_not_runnable() {
        let catalog = offline();
        let row = catalog
            .models_for_engine("codex", "sol", None)
            .into_iter()
            .find(|row| row.id == "codex-sol")
            .expect("the real codex-sol row is present");
        assert!(!row.available);
        assert_eq!(
            row.unavailable_reason.as_deref(),
            Some("codex is not configured in this runtime.")
        );
        assert!(catalog.policy_for_model("codex-sol").is_err());
    }

    #[test]
    fn offline_model_choice_is_valid_but_execution_still_requires_runtime() {
        let catalog = offline();
        let mut policy = catalog.selection_policy_for_model("codex-sol").unwrap();
        assert!(catalog.validate_selection_policy(&policy).is_ok());
        assert!(catalog.admit_policy(&policy).is_err());
        policy.native_model_id = "wrong-model".to_owned();
        assert!(catalog.validate_selection_policy(&policy).is_err());
    }

    #[test]
    fn invalid_selected_option_is_rejected_without_normalization() {
        let catalog = offline();
        let error = catalog
            .policy_for_model("codex-sol")
            .expect_err("offline route must still block policy construction");
        assert!(matches!(
            error,
            NativePolicyValidationError::UnavailableModel { .. }
        ));

        let model = catalog.manifest.model("codex-sol").expect("fixture model");
        let policy = NativeModelPolicy {
            catalog_revision: catalog.catalog_revision.clone(),
            profile_id: None,
            engine_id: model.harness.clone(),
            model_id: model.id.clone(),
            native_model_id: model.native_model_id.clone(),
            native_selection: model.native_selection.clone(),
            reasoning_effort: Some(NativeOptionValue {
                id: "invented".to_owned(),
                native_value: "invented".to_owned(),
            }),
            speed: None,
            context_window: None,
            permission: None,
        };
        let result = catalog.validate_policy(&policy);
        assert!(matches!(
            result,
            Err(NativePolicyValidationError::InvalidThinkingOption { .. })
        ));
    }

    #[test]
    fn routed_models_keep_distinct_native_identities() {
        let mut catalog = offline();
        let first = catalog.manifest.models[0].clone();
        let mut second = first.clone();
        second.id = "codex-sol-route-b".to_owned();
        second.native_selection = Some(NativeModelSelection {
            model_id: "gpt-5.6-sol".to_owned(),
            provider_route_id: "route-b".to_owned(),
            variant_id: Some("high".to_owned()),
        });
        catalog.manifest.models.push(second);
        let first_identity = catalog.model_identity(&first.id).expect("first identity");
        let second_identity = catalog
            .model_identity("codex-sol-route-b")
            .expect("second identity");
        assert_ne!(first_identity, second_identity);
        assert_eq!(
            second_identity
                .native_selection
                .as_ref()
                .expect("routed identity")
                .provider_route_id,
            "route-b"
        );
    }

    #[test]
    fn favorite_and_query_projection_preserves_favorite_first_filtering() {
        let mut catalog = offline();
        catalog.favorite_ids = vec!["codex-terra".to_owned()];
        let rows = catalog.models_for_engine("codex", "", None);
        assert_eq!(rows.first().map(|row| row.id.as_str()), Some("codex-terra"));
        let filtered = catalog.models_for_engine("codex", "LUNA", None);
        assert_eq!(
            filtered
                .iter()
                .map(|row| row.id.as_str())
                .collect::<Vec<_>>(),
            vec!["codex-luna"]
        );
    }

    #[test]
    fn unavailable_route_is_visible_with_route_reason() {
        let mut catalog = offline();
        catalog.runnable_harness_ids.push("codex".to_owned());
        let mut model = catalog.manifest.models[0].clone();
        model.id = "codex-routed".to_owned();
        model.routing = NativeModelRouting::ProviderRoute {
            provider_route_id: "route-unavailable".to_owned(),
        };
        model.native_selection = Some(NativeModelSelection {
            model_id: model.native_model_id.clone(),
            provider_route_id: "route-unavailable".to_owned(),
            variant_id: None,
        });
        catalog.manifest.models.push(model);
        catalog.routes.push(NativeModelRoute {
            engine_id: "codex".to_owned(),
            group: NativeModelRouteGroup {
                id: "managed".to_owned(),
                label: "Managed".to_owned(),
                order: 0,
                show_route_labels: true,
            },
            id: "route-unavailable".to_owned(),
            label: "Managed route".to_owned(),
            status: NativeModelRouteStatus::Unavailable,
            unavailable_reason: Some("Provider credentials are missing.".to_owned()),
        });
        assert_eq!(
            catalog.selectability("codex-routed").unavailable_reason(),
            Some("Provider credentials are missing.")
        );
    }

    #[test]
    fn rebase_preserves_explicit_native_profile_with_scope_fallback() {
        let catalog = offline();
        let mut policy = catalog.selection_policy_for_model("codex-sol").unwrap();
        policy.profile_id = Some("default".to_owned());
        let rebased = catalog.rebase_policy(&policy).expect("rebased policy");
        assert_eq!(rebased.model_id, "codex-sol");
        assert_eq!(rebased.profile_id.as_deref(), Some("default"));

        let mut scoped = policy.clone();
        scoped.profile_id = None;
        let rebased = catalog.rebase_policy(&scoped).expect("rebased policy");
        assert_eq!(rebased.profile_id, None);
    }
}
