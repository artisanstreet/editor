//! Runtime catalog, policy admission, and model-row projection.

use thiserror::Error;

use crate::manifest::{
    NativeContextConfig, NativeContextWindowOption, NativeHarness, NativeModelCapabilities,
    NativeModelCatalogError, NativeModelDefinition, NativeModelManifest, NativeModelRouting,
    NativeModelSelection, NativePermissionOption, NativeSpeedOption, NativeThinkingCapability,
    NativeThinkingOption,
};
use crate::{NATIVE_HARNESS_MANIFEST_JSON, NATIVE_MODEL_CATALOG_SOURCE};
/// Runtime route grouping metadata supplied by Forge.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeModelRouteGroup {
    /// Stable group identifier.
    pub id: String,
    /// Exact group label.
    pub label: String,
    /// Display order.
    pub order: u32,
    /// Whether rows should show route labels.
    pub show_route_labels: bool,
}

/// Runtime route status.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeModelRouteStatus {
    /// The route can be used by a newly selected policy.
    Available,
    /// The route is visible but cannot be selected.
    Unavailable,
}

/// A route observed in a live runtime snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeModelRoute {
    /// Engine/harness owning the route.
    pub engine_id: String,
    /// Display grouping metadata.
    pub group: NativeModelRouteGroup,
    /// Stable route identifier.
    pub id: String,
    /// Exact route label.
    pub label: String,
    /// Runtime availability.
    pub status: NativeModelRouteStatus,
    /// Exact reason when unavailable.
    pub unavailable_reason: Option<String>,
}

/// Scope from which a runtime catalog was discovered.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeCatalogScope {
    /// Profile identifier.
    pub profile_id: String,
    /// Working directory used for discovery.
    pub working_directory: String,
    /// Workspace trust mode.
    pub workspace_trust: String,
}

/// One exact native option value persisted by the owner.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeOptionValue {
    /// Stable catalog option identifier.
    pub id: String,
    /// Exact native value.
    pub native_value: String,
}

/// One exact context-window value persisted by the owner.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeContextSelection {
    /// Stable catalog option identifier.
    pub id: String,
    /// Exact native suffix.
    pub native_suffix: String,
    /// Optional exact native configuration.
    pub native_config: Option<NativeContextConfig>,
}

/// Per-model persisted defaults received from the owner.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeModelDefaults {
    /// Exact catalog model identifier.
    pub model_id: String,
    /// Persisted reasoning selection.
    pub reasoning_effort: Option<NativeOptionValue>,
    /// Persisted speed selection.
    pub speed: Option<NativeOptionValue>,
    /// Persisted context selection.
    pub context_window: Option<NativeContextSelection>,
    /// Persisted permission selection.
    pub permission: Option<NativeOptionValue>,
}

/// Runtime state layered over the bundled static manifest.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NativeCatalogRuntime {
    /// Runtime revision, if Forge supplied one.
    pub catalog_revision: Option<String>,
    /// Harnesses actually registered in this process.
    pub runnable_harness_ids: Vec<String>,
    /// Scoped runtime routes.
    pub routes: Vec<NativeModelRoute>,
    /// Owner-selected default model.
    pub default_model_id: Option<String>,
    /// Owner-authoritative favorite IDs.
    pub favorite_ids: Vec<String>,
    /// Owner-authoritative per-model defaults.
    pub model_defaults: Vec<NativeModelDefaults>,
    /// Optional discovery scope.
    pub scope: Option<NativeCatalogScope>,
}

/// Provenance retained alongside the static manifest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeCatalogProvenance {
    /// Source file that produced the snapshot.
    pub source: String,
    /// Static manifest revision.
    pub revision: String,
}

/// A complete static-plus-runtime catalog snapshot.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeModelCatalog {
    /// Complete static manifest, including unavailable models.
    pub manifest: NativeModelManifest,
    /// Current runtime snapshot revision.
    pub catalog_revision: String,
    /// Harness IDs that can actually run on this machine.
    pub runnable_harness_ids: Vec<String>,
    /// Runtime route observations.
    pub routes: Vec<NativeModelRoute>,
    /// Owner-authoritative default model ID.
    pub default_model_id: Option<String>,
    /// Owner-authoritative favorite IDs.
    pub favorite_ids: Vec<String>,
    /// Owner-authoritative per-model defaults.
    pub model_defaults: Vec<NativeModelDefaults>,
    /// Optional runtime discovery scope.
    pub scope: Option<NativeCatalogScope>,
    /// Static snapshot provenance.
    pub provenance: NativeCatalogProvenance,
}

impl NativeModelCatalog {
    /// Builds an empty, non-runnable catalog with the supported harness descriptors.
    ///
    /// # Errors
    ///
    /// Returns [`NativeModelCatalogError`] if the checked-in snapshot fails
    /// decoding or validation, which would be a build-time invariant
    /// violation.
    pub fn offline() -> Result<Self, NativeModelCatalogError> {
        let manifest = NativeModelManifest::from_json_str(NATIVE_HARNESS_MANIFEST_JSON)?;
        let revision = manifest.revision.clone();
        Ok(Self {
            provenance: NativeCatalogProvenance {
                source: NATIVE_MODEL_CATALOG_SOURCE.to_owned(),
                revision: revision.clone(),
            },
            manifest,
            catalog_revision: revision,
            runnable_harness_ids: Vec::new(),
            routes: Vec::new(),
            default_model_id: None,
            favorite_ids: Vec::new(),
            model_defaults: Vec::new(),
            scope: None,
        })
    }

    /// Decodes an explicit manifest supplied by the caller (for imported snapshots and fixtures).
    ///
    /// # Errors
    /// Returns an error when the manifest is malformed.
    pub fn from_manifest_json(json: &str) -> Result<Self, NativeModelCatalogError> {
        Ok(Self::from_manifest(
            NativeModelManifest::from_json_str(json)?,
            NativeCatalogRuntime::default(),
        ))
    }

    /// Builds a catalog from a manifest and an owner-supplied runtime layer.
    #[must_use]
    pub fn from_manifest(manifest: NativeModelManifest, runtime: NativeCatalogRuntime) -> Self {
        let revision = manifest.revision.clone();
        let catalog_revision = runtime.catalog_revision.unwrap_or_else(|| revision.clone());
        Self {
            provenance: NativeCatalogProvenance {
                source: NATIVE_MODEL_CATALOG_SOURCE.to_owned(),
                revision,
            },
            manifest,
            catalog_revision,
            runnable_harness_ids: runtime.runnable_harness_ids,
            routes: runtime.routes,
            default_model_id: runtime.default_model_id,
            favorite_ids: runtime.favorite_ids,
            model_defaults: runtime.model_defaults,
            scope: runtime.scope,
        }
    }

    /// Replaces only the owner/runtime layer while retaining the static
    /// manifest and its provenance.
    pub fn replace_runtime(&mut self, runtime: NativeCatalogRuntime) {
        self.catalog_revision = runtime
            .catalog_revision
            .unwrap_or_else(|| self.manifest.revision.clone());
        self.runnable_harness_ids = runtime.runnable_harness_ids;
        self.routes = runtime.routes;
        self.default_model_id = runtime.default_model_id;
        self.favorite_ids = runtime.favorite_ids;
        self.model_defaults = runtime.model_defaults;
        self.scope = runtime.scope;
    }

    /// Returns the runtime layer in the protocol-facing shape.
    #[must_use]
    pub fn runtime(&self) -> NativeCatalogRuntime {
        NativeCatalogRuntime {
            catalog_revision: Some(self.catalog_revision.clone()),
            runnable_harness_ids: self.runnable_harness_ids.clone(),
            routes: self.routes.clone(),
            default_model_id: self.default_model_id.clone(),
            favorite_ids: self.favorite_ids.clone(),
            model_defaults: self.model_defaults.clone(),
            scope: self.scope.clone(),
        }
    }

    /// Returns a model's exact static identity, including route/variant IDs.
    ///
    /// # Errors
    ///
    /// Returns [`NativePolicyValidationError::UnknownModel`] when `model_id`
    /// is not present in the static manifest.
    pub fn model_identity(
        &self,
        model_id: &str,
    ) -> Result<NativeModelIdentity, NativePolicyValidationError> {
        let model = self
            .manifest
            .model(model_id)
            .ok_or_else(|| NativePolicyValidationError::UnknownModel(model_id.to_owned()))?;
        Ok(NativeModelIdentity {
            engine_id: model.harness.clone(),
            catalog_model_id: model.id.clone(),
            native_model_id: model.native_model_id.clone(),
            native_selection: model.native_selection.clone(),
        })
    }

    /// Returns whether the model can be admitted as a newly selected policy.
    #[must_use]
    pub fn selectability(&self, model_id: &str) -> NativeModelSelectability {
        let Some(model) = self.manifest.model(model_id) else {
            return NativeModelSelectability::Unavailable {
                reason: format!("Model '{model_id}' is not in this catalog."),
            };
        };

        if let Some(disabled) = &model.disabled {
            return NativeModelSelectability::Unavailable {
                reason: disabled.reason.clone(),
            };
        }
        if !self
            .runnable_harness_ids
            .iter()
            .any(|harness_id| harness_id == &model.harness)
        {
            return NativeModelSelectability::Unavailable {
                reason: format!("{} is not configured in this runtime.", model.harness),
            };
        }

        let required_route = model_route_id(model);
        if let Some(route_id) = required_route {
            let Some(route) = self.routes.iter().find(|route| route.id == route_id) else {
                return NativeModelSelectability::Unavailable {
                    reason: format!("Route '{route_id}' is not available in this runtime."),
                };
            };
            if route.status != NativeModelRouteStatus::Available {
                return NativeModelSelectability::Unavailable {
                    reason: route
                        .unavailable_reason
                        .clone()
                        .unwrap_or_else(|| format!("Route '{route_id}' is unavailable.")),
                };
            }
        }

        NativeModelSelectability::Available
    }

    /// Returns whether a model is a favorite in the owner-authoritative list.
    #[must_use]
    pub fn is_favorite(&self, model_id: &str) -> bool {
        self.favorite_ids
            .iter()
            .any(|favorite| favorite == model_id)
    }

    /// Builds a runnable policy using persisted values when they still match
    /// the current model capabilities, otherwise capability defaults.
    ///
    /// # Errors
    ///
    /// Returns [`NativePolicyValidationError::UnavailableModel`] when the
    /// model is disabled or not runnable in this runtime, or any error from
    /// [`Self::preview_policy_for_model`].
    pub fn policy_for_model(
        &self,
        model_id: &str,
    ) -> Result<NativeModelPolicy, NativePolicyValidationError> {
        let availability = self.selectability(model_id);
        if !availability.is_available() {
            return Err(NativePolicyValidationError::UnavailableModel {
                model_id: model_id.to_owned(),
                reason: availability
                    .unavailable_reason()
                    .unwrap_or("Model is unavailable.")
                    .to_owned(),
            });
        }
        self.preview_policy_for_model(model_id)
    }

    /// Builds a user choice without requiring an installed or connected engine.
    /// Execution must still pass [`Self::admit_policy`].
    ///
    /// # Errors
    ///
    /// Returns any error from [`Self::preview_policy_for_model`] and
    /// [`Self::validate_selection_policy`].
    pub fn selection_policy_for_model(
        &self,
        model_id: &str,
    ) -> Result<NativeModelPolicy, NativePolicyValidationError> {
        let policy = self.preview_policy_for_model(model_id)?;
        self.validate_selection_policy(&policy)?;
        Ok(policy)
    }

    /// Checks catalog identity, options, and explicit model retirement only.
    ///
    /// # Errors
    ///
    /// Returns any error from [`Self::validate_policy`], or
    /// [`NativePolicyValidationError::UnavailableModel`] when the catalog
    /// explicitly retires the model.
    pub fn validate_selection_policy(
        &self,
        policy: &NativeModelPolicy,
    ) -> Result<(), NativePolicyValidationError> {
        self.validate_policy(policy)?;
        if let Some(disabled) = self
            .manifest
            .model(&policy.model_id)
            .and_then(|model| model.disabled.as_ref())
        {
            return Err(NativePolicyValidationError::UnavailableModel {
                model_id: policy.model_id.clone(),
                reason: disabled.reason.clone(),
            });
        }
        Ok(())
    }

    /// Builds a policy-shaped preview from capability and persisted defaults
    /// without requiring a runtime harness. This powers disconnected preview
    /// controls; callers must use [`Self::policy_for_model`] or
    /// [`Self::admit_policy`] before treating the result as runnable.
    ///
    /// # Errors
    ///
    /// Returns [`NativePolicyValidationError`] when the model or its harness
    /// is unknown or a persisted option no longer matches the catalog.
    pub fn preview_policy_for_model(
        &self,
        model_id: &str,
    ) -> Result<NativeModelPolicy, NativePolicyValidationError> {
        let model = self
            .manifest
            .model(model_id)
            .ok_or_else(|| NativePolicyValidationError::UnknownModel(model_id.to_owned()))?;
        let harness = self
            .manifest
            .harness(&model.harness)
            .ok_or_else(|| NativePolicyValidationError::UnknownHarness(model.harness.clone()))?;
        let persisted = self
            .model_defaults
            .iter()
            .find(|defaults| defaults.model_id == model.id);

        let reasoning_effort = persisted
            .and_then(|defaults| defaults.reasoning_effort.clone())
            .filter(|value| thinking_value(model, value).is_some())
            .or_else(|| default_thinking(model));
        let speed = persisted
            .and_then(|defaults| defaults.speed.clone())
            .filter(|value| speed_value(model, value).is_some())
            .or_else(|| default_speed(model));
        let context_window = persisted
            .and_then(|defaults| defaults.context_window.clone())
            .filter(|value| context_value(model, value).is_some())
            .or_else(|| default_context(model));
        let permission = persisted
            .and_then(|defaults| defaults.permission.clone())
            .filter(|value| permission_value(harness, value).is_some())
            .or_else(|| default_permission(harness));

        let policy = NativeModelPolicy {
            catalog_revision: self.catalog_revision.clone(),
            profile_id: self.scope.as_ref().map(|scope| scope.profile_id.clone()),
            engine_id: model.harness.clone(),
            model_id: model.id.clone(),
            native_model_id: model.native_model_id.clone(),
            native_selection: model.native_selection.clone(),
            reasoning_effort,
            speed,
            context_window,
            permission,
        };
        self.validate_policy(&policy)?;
        Ok(policy)
    }

    /// Reconciles an owner-authoritative policy onto this snapshot while
    /// retaining only option values that still exist in the new capabilities.
    /// A disconnected or unavailable model may still be displayed; the caller
    /// decides whether to admit it with [`Self::admit_policy`].
    #[must_use]
    pub fn rebase_policy(&self, policy: &NativeModelPolicy) -> Option<NativeModelPolicy> {
        let model = self.manifest.model(&policy.model_id)?;
        if policy.engine_id != model.harness
            || policy.native_model_id != model.native_model_id
            || policy.native_selection != model.native_selection
        {
            return None;
        }
        let harness = self.manifest.harness(&model.harness)?;
        let rebased = NativeModelPolicy {
            catalog_revision: self.catalog_revision.clone(),
            // An explicit native profile is owner-authoritative durability
            // (the saved thread configuration), not scope state: keep it so
            // a reloaded thread displays its saved profile. Managed
            // `OpenCode` policies stay scope-fenced and fall back to the
            // discovery scope, as does any policy without an explicit one.
            profile_id: if policy.engine_id == "opencode2" {
                self.scope.as_ref().map(|scope| scope.profile_id.clone())
            } else {
                policy
                    .profile_id
                    .clone()
                    .or_else(|| self.scope.as_ref().map(|scope| scope.profile_id.clone()))
            },
            engine_id: model.harness.clone(),
            model_id: model.id.clone(),
            native_model_id: model.native_model_id.clone(),
            native_selection: model.native_selection.clone(),
            reasoning_effort: policy
                .reasoning_effort
                .clone()
                .filter(|value| thinking_value(model, value).is_some()),
            speed: policy
                .speed
                .clone()
                .filter(|value| speed_value(model, value).is_some()),
            context_window: policy.context_window.as_ref().and_then(|value| {
                model
                    .capabilities
                    .context_window
                    .as_ref()?
                    .options
                    .iter()
                    .find(|option| option.id == value.id)
                    .map(|option| NativeContextSelection {
                        id: option.id.clone(),
                        native_suffix: option.native_suffix.clone(),
                        native_config: option.native_config.clone(),
                    })
            }),
            permission: policy
                .permission
                .clone()
                .filter(|value| permission_value(harness, value).is_some()),
        };
        self.validate_policy(&rebased).ok().map(|()| rebased)
    }

    /// Validates exact model, route, and option membership without requiring
    /// the model to be currently runnable. This is used for authoritative
    /// policies that may be displayed while a runtime is disconnected.
    ///
    /// # Errors
    ///
    /// Returns [`NativePolicyValidationError`] for a stale revision, unknown
    /// model or harness, engine or native-identity mismatch, or an option
    /// value outside the model's admitted choices.
    pub fn validate_policy(
        &self,
        policy: &NativeModelPolicy,
    ) -> Result<(), NativePolicyValidationError> {
        if policy.catalog_revision != self.catalog_revision {
            return Err(NativePolicyValidationError::StaleCatalog {
                expected: self.catalog_revision.clone(),
                received: policy.catalog_revision.clone(),
            });
        }
        let model = self
            .manifest
            .model(&policy.model_id)
            .ok_or_else(|| NativePolicyValidationError::UnknownModel(policy.model_id.clone()))?;
        if policy.engine_id != model.harness {
            return Err(NativePolicyValidationError::EngineMismatch {
                expected: model.harness.clone(),
                received: policy.engine_id.clone(),
            });
        }
        if policy.native_model_id != model.native_model_id {
            return Err(NativePolicyValidationError::NativeModelMismatch {
                expected: model.native_model_id.clone(),
                received: policy.native_model_id.clone(),
            });
        }
        if policy.native_selection != model.native_selection {
            return Err(NativePolicyValidationError::NativeSelectionMismatch);
        }

        if let Some(value) = &policy.reasoning_effort
            && thinking_value(model, value).is_none()
        {
            return Err(NativePolicyValidationError::InvalidThinkingOption {
                model_id: model.id.clone(),
                option_id: value.id.clone(),
            });
        }
        if let Some(value) = &policy.speed
            && speed_value(model, value).is_none()
        {
            return Err(NativePolicyValidationError::InvalidSpeedOption {
                model_id: model.id.clone(),
                option_id: value.id.clone(),
            });
        }
        if let Some(value) = &policy.context_window
            && context_value(model, value).is_none()
        {
            return Err(NativePolicyValidationError::InvalidContextOption {
                model_id: model.id.clone(),
                option_id: value.id.clone(),
            });
        }
        if let Some(value) = &policy.permission {
            let harness = self.manifest.harness(&model.harness).ok_or_else(|| {
                NativePolicyValidationError::UnknownHarness(model.harness.clone())
            })?;
            if permission_value(harness, value).is_none() {
                return Err(NativePolicyValidationError::InvalidPermissionOption {
                    engine_id: model.harness.clone(),
                    option_id: value.id.clone(),
                });
            }
        }
        Ok(())
    }

    /// Validates a policy and additionally requires current runtime
    /// selectability.
    ///
    /// # Errors
    ///
    /// Returns any error from [`Self::validate_policy`], or
    /// [`NativePolicyValidationError::UnavailableModel`] when the model is not
    /// currently selectable in this runtime.
    pub fn admit_policy(
        &self,
        policy: &NativeModelPolicy,
    ) -> Result<(), NativePolicyValidationError> {
        self.validate_policy(policy)?;
        match self.selectability(&policy.model_id) {
            NativeModelSelectability::Available => Ok(()),
            NativeModelSelectability::Unavailable { reason } => {
                Err(NativePolicyValidationError::UnavailableModel {
                    model_id: policy.model_id.clone(),
                    reason,
                })
            }
        }
    }

    /// Returns filtered model rows for an engine, retaining unavailable rows
    /// for truthful preview while sorting favorites to the top.
    #[must_use]
    pub fn models_for_engine(
        &self,
        engine_id: &str,
        query: &str,
        selected_model_id: Option<&str>,
    ) -> Vec<NativeModelView> {
        let query = query.trim().to_ascii_lowercase();
        let mut rows = self
            .manifest
            .models
            .iter()
            .enumerate()
            .filter(|(_, model)| model.harness == engine_id)
            .filter(|(_, model)| query.is_empty() || self.model_matches_query(model, &query))
            .map(|(source_index, model)| self.model_view(model, source_index, selected_model_id))
            .collect::<Vec<_>>();
        rows.sort_by_key(|row| {
            (
                !row.favorite,
                !(row.selected || same_variant_family(self, &row.id, selected_model_id)),
                row.source_index,
            )
        });
        rows
    }

    /// Projects only variants of one routed model, avoiding full-catalog clones during preview redraws.
    #[must_use]
    pub fn variants_for_model(&self, model: &NativeModelView) -> Vec<NativeModelView> {
        let route = model
            .native_selection
            .as_ref()
            .map(|selection| selection.provider_route_id.as_str());
        self.manifest
            .models
            .iter()
            .enumerate()
            .filter(|(_, candidate)| {
                candidate.harness == model.engine_id
                    && candidate.native_model_id == model.native_model_id
                    && candidate
                        .native_selection
                        .as_ref()
                        .map(|selection| selection.provider_route_id.as_str())
                        == route
            })
            .map(|(index, candidate)| self.model_view(candidate, index, Some(&model.id)))
            .collect()
    }

    /// Returns route-aware grouped rows for an engine.
    #[must_use]
    pub fn route_groups_for_engine(
        &self,
        engine_id: &str,
        query: &str,
        selected_model_id: Option<&str>,
    ) -> Vec<NativeModelGroupView> {
        let mut groups: Vec<NativeModelGroupView> = Vec::new();
        for row in self.models_for_engine(engine_id, query, selected_model_id) {
            let (group_id, group_label, group_order, show_route_labels) =
                self.group_for_model(&row);
            if let Some(group) = groups.iter_mut().find(|group| group.id == group_id) {
                group.available |= row.available;
                if group.unavailable_reason.is_none() && !row.available {
                    group.unavailable_reason.clone_from(&row.unavailable_reason);
                }
                group.models.push(row);
            } else {
                groups.push(NativeModelGroupView {
                    id: group_id,
                    label: group_label,
                    order: group_order,
                    show_route_labels,
                    available: row.available,
                    unavailable_reason: row.unavailable_reason.clone(),
                    models: vec![row],
                });
            }
        }
        groups.sort_by_key(|group| group.order);
        groups
    }

    fn model_matches_query(&self, model: &NativeModelDefinition, query: &str) -> bool {
        let provider_label = self
            .manifest
            .provider(&model.provider)
            .map_or("", |provider| provider.label.as_str());
        [
            model.id.as_str(),
            model.name.as_str(),
            model.native_model_id.as_str(),
            model.description.as_deref().unwrap_or(""),
            provider_label,
        ]
        .iter()
        .any(|value| value.to_ascii_lowercase().contains(query))
    }

    fn model_view(
        &self,
        model: &NativeModelDefinition,
        source_index: usize,
        selected_model_id: Option<&str>,
    ) -> NativeModelView {
        let provider_label = self
            .manifest
            .provider(&model.provider)
            .map_or_else(|| model.provider.clone(), |provider| provider.label.clone());
        let route = model_route_id(model).and_then(|route_id| {
            self.routes
                .iter()
                .find(|route| route.id == route_id && route.engine_id == model.harness)
        });
        let availability = self.selectability(&model.id);
        NativeModelView {
            id: model.id.clone(),
            name: model.name.clone(),
            lab: provider_label,
            provider_id: model.provider.clone(),
            engine_id: model.harness.clone(),
            description: model.description.clone(),
            native_model_id: model.native_model_id.clone(),
            native_selection: model.native_selection.clone(),
            route_label: route.map(|route| route.label.clone()),
            variant_label: model
                .native_selection
                .as_ref()
                .and_then(|selection| selection.variant_id.clone()),
            capabilities: model.capabilities.clone(),
            selected: selected_model_id == Some(model.id.as_str()),
            favorite: self.is_favorite(&model.id),
            available: availability.is_available(),
            unavailable_reason: availability.unavailable_reason().map(str::to_owned),
            source_index,
        }
    }

    fn group_for_model(&self, row: &NativeModelView) -> (String, String, u32, bool) {
        let model_routing = self.manifest.model(&row.id).map(|model| &model.routing);
        let Some(route_id) = row
            .native_selection
            .as_ref()
            .map(|selection| selection.provider_route_id.as_str())
            .or(match model_routing {
                Some(NativeModelRouting::ProviderRoute { provider_route_id }) => {
                    Some(provider_route_id.as_str())
                }
                _ => None,
            })
        else {
            return ("default".to_owned(), "Models".to_owned(), 0, false);
        };
        if let Some(route) = self
            .routes
            .iter()
            .find(|route| route.id == route_id && route.engine_id == row.engine_id)
        {
            return (
                route.group.id.clone(),
                route.group.label.clone(),
                route.group.order,
                route.group.show_route_labels,
            );
        }
        (route_id.to_owned(), route_id.to_owned(), u32::MAX, true)
    }
}

/// Exact identity needed to distinguish routed models that share a native
/// provider model name.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct NativeModelIdentity {
    /// Harness/engine identifier.
    pub engine_id: String,
    /// Stable catalog model ID.
    pub catalog_model_id: String,
    /// Definition-native model ID.
    pub native_model_id: String,
    /// Optional exact routed model/route/variant selection.
    pub native_selection: Option<NativeModelSelection>,
}

/// A model row rendered by the selector.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeModelView {
    /// Stable catalog ID.
    pub id: String,
    /// Exact display name.
    pub name: String,
    /// Provider label shown under the name.
    pub lab: String,
    /// Provider identifier.
    pub provider_id: String,
    /// Harness/engine identifier.
    pub engine_id: String,
    /// Optional description shown in preview.
    pub description: Option<String>,
    /// Native model identifier.
    pub native_model_id: String,
    /// Optional route identity.
    pub native_selection: Option<NativeModelSelection>,
    /// Runtime route label, if observed.
    pub route_label: Option<String>,
    /// Optional variant label.
    pub variant_label: Option<String>,
    /// Full capability set for preview/control rendering.
    pub capabilities: NativeModelCapabilities,
    /// Whether this is the current selected model.
    pub selected: bool,
    /// Whether the owner marked this model favorite.
    pub favorite: bool,
    /// Whether a new policy may select it now.
    pub available: bool,
    /// Exact truthful unavailable reason.
    pub unavailable_reason: Option<String>,
    /// Source manifest order, retained for stable sorting.
    pub source_index: usize,
}

/// A route/group section in the model list.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeModelGroupView {
    /// Stable group ID.
    pub id: String,
    /// Display group label.
    pub label: String,
    /// Runtime display order.
    pub order: u32,
    /// Whether route labels should be visible in child rows.
    pub show_route_labels: bool,
    /// Whether at least one child model is selectable.
    pub available: bool,
    /// First truthful unavailable reason, when no child is available.
    pub unavailable_reason: Option<String>,
    /// Child model rows.
    pub models: Vec<NativeModelView>,
}

/// Admission status for one model.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeModelSelectability {
    /// The model may be selected by a new policy.
    Available,
    /// The model remains visible but is not selectable.
    Unavailable {
        /// Exact reason shown by the UI.
        reason: String,
    },
}

impl NativeModelSelectability {
    /// Returns whether this status admits a new selection.
    #[must_use]
    pub const fn is_available(&self) -> bool {
        matches!(self, Self::Available)
    }

    /// Returns the truthful reason, if unavailable.
    #[must_use]
    pub fn unavailable_reason(&self) -> Option<&str> {
        match self {
            Self::Available => None,
            Self::Unavailable { reason } => Some(reason),
        }
    }
}

/// A selected model policy emitted to the application owner.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeModelPolicy {
    /// Runtime catalog revision used to derive this policy.
    pub catalog_revision: String,
    /// Optional profile scope.
    pub profile_id: Option<String>,
    /// Harness/engine identifier.
    pub engine_id: String,
    /// Exact catalog model ID.
    pub model_id: String,
    /// Exact definition-native model ID.
    pub native_model_id: String,
    /// Exact routed model/route/variant identity.
    pub native_selection: Option<NativeModelSelection>,
    /// Selected reasoning native value.
    pub reasoning_effort: Option<NativeOptionValue>,
    /// Selected speed native value.
    pub speed: Option<NativeOptionValue>,
    /// Selected context native values.
    pub context_window: Option<NativeContextSelection>,
    /// Selected permission native value.
    pub permission: Option<NativeOptionValue>,
}

/// Validation failures for owner-supplied or user-selected policy values.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum NativePolicyValidationError {
    /// Model ID is not present in the static manifest.
    #[error("unknown model '{0}'")]
    UnknownModel(String),
    /// Harness ID is not present in the static manifest.
    #[error("unknown harness '{0}'")]
    UnknownHarness(String),
    /// The policy engine does not match the model's harness.
    #[error("policy engine '{received}' does not match model engine '{expected}'")]
    EngineMismatch {
        /// Model's exact harness ID.
        expected: String,
        /// Received policy engine ID.
        received: String,
    },
    /// Native model IDs differ from the catalog.
    #[error("native model id mismatch: expected '{expected}', received '{received}'")]
    NativeModelMismatch {
        /// Expected native ID.
        expected: String,
        /// Received native ID.
        received: String,
    },
    /// Routed native identity differs from the catalog.
    #[error("native route/variant selection does not match the catalog")]
    NativeSelectionMismatch,
    /// Policy revision is stale.
    #[error("stale catalog revision: expected '{expected}', received '{received}'")]
    StaleCatalog {
        /// Current catalog revision.
        expected: String,
        /// Policy revision.
        received: String,
    },
    /// A model is truthful but unavailable for new selection.
    #[error("model '{model_id}' is unavailable: {reason}")]
    UnavailableModel {
        /// Exact catalog model ID.
        model_id: String,
        /// Exact availability reason.
        reason: String,
    },
    /// Reasoning option is not supported by this model.
    #[error("invalid reasoning option '{option_id}' for model '{model_id}'")]
    InvalidThinkingOption {
        /// Exact model ID.
        model_id: String,
        /// Received option ID.
        option_id: String,
    },
    /// Speed option is not supported or is disabled.
    #[error("invalid speed option '{option_id}' for model '{model_id}'")]
    InvalidSpeedOption {
        /// Exact model ID.
        model_id: String,
        /// Received option ID.
        option_id: String,
    },
    /// Context option is not supported by this model.
    #[error("invalid context option '{option_id}' for model '{model_id}'")]
    InvalidContextOption {
        /// Exact model ID.
        model_id: String,
        /// Received option ID.
        option_id: String,
    },
    /// Permission option is not supported by this harness.
    #[error("invalid permission option '{option_id}' for engine '{engine_id}'")]
    InvalidPermissionOption {
        /// Exact engine ID.
        engine_id: String,
        /// Received option ID.
        option_id: String,
    },
}

fn model_route_id(model: &NativeModelDefinition) -> Option<&str> {
    model
        .native_selection
        .as_ref()
        .map(|selection| selection.provider_route_id.as_str())
        .or(match &model.routing {
            NativeModelRouting::ProviderRoute { provider_route_id } => {
                Some(provider_route_id.as_str())
            }
            _ => None,
        })
}

fn same_variant_family(
    catalog: &NativeModelCatalog,
    model_id: &str,
    selected_model_id: Option<&str>,
) -> bool {
    let Some(selected_model_id) = selected_model_id else {
        return false;
    };
    let Ok(model) = catalog.model_identity(model_id) else {
        return false;
    };
    let Ok(selected) = catalog.model_identity(selected_model_id) else {
        return false;
    };
    model.engine_id == selected.engine_id
        && model
            .native_selection
            .as_ref()
            .map(|selection| selection.provider_route_id.as_str())
            == selected
                .native_selection
                .as_ref()
                .map(|selection| selection.provider_route_id.as_str())
        && model.native_model_id == selected.native_model_id
}

fn thinking_value<'a>(
    model: &'a NativeModelDefinition,
    value: &NativeOptionValue,
) -> Option<&'a NativeThinkingOption> {
    match &model.capabilities.thinking {
        NativeThinkingCapability::Supported { options, .. } => options
            .iter()
            .find(|option| option.id == value.id && option.native_value == value.native_value),
        NativeThinkingCapability::Unavailable | NativeThinkingCapability::Native { .. } => None,
    }
}

fn default_thinking(model: &NativeModelDefinition) -> Option<NativeOptionValue> {
    match &model.capabilities.thinking {
        NativeThinkingCapability::Supported { default, options } => options
            .iter()
            .find(|option| &option.id == default)
            .map(|option| NativeOptionValue {
                id: option.id.clone(),
                native_value: option.native_value.clone(),
            }),
        NativeThinkingCapability::Unavailable | NativeThinkingCapability::Native { .. } => None,
    }
}

fn speed_value<'a>(
    model: &'a NativeModelDefinition,
    value: &NativeOptionValue,
) -> Option<&'a NativeSpeedOption> {
    model.capabilities.speed_options.iter().find(|option| {
        option.id == value.id
            && option.native_value == value.native_value
            && option.disabled.is_none()
    })
}

fn default_speed(model: &NativeModelDefinition) -> Option<NativeOptionValue> {
    model
        .capabilities
        .speed_options
        .iter()
        .find(|option| option.default && option.disabled.is_none())
        .map(|option| NativeOptionValue {
            id: option.id.clone(),
            native_value: option.native_value.clone(),
        })
}

fn context_value<'a>(
    model: &'a NativeModelDefinition,
    value: &NativeContextSelection,
) -> Option<&'a NativeContextWindowOption> {
    model
        .capabilities
        .context_window
        .as_ref()
        .and_then(|capability| {
            capability.options.iter().find(|option| {
                option.id == value.id
                    && option.native_suffix == value.native_suffix
                    && option.native_config == value.native_config
            })
        })
}

fn default_context(model: &NativeModelDefinition) -> Option<NativeContextSelection> {
    model
        .capabilities
        .context_window
        .as_ref()
        .and_then(|capability| {
            capability
                .options
                .iter()
                .find(|option| option.id == capability.default)
        })
        .map(|option| NativeContextSelection {
            id: option.id.clone(),
            native_suffix: option.native_suffix.clone(),
            native_config: option.native_config.clone(),
        })
}

fn permission_value<'a>(
    harness: &'a NativeHarness,
    value: &NativeOptionValue,
) -> Option<&'a NativePermissionOption> {
    harness
        .permissions
        .options
        .iter()
        .find(|option| option.id == value.id && option.native_value == value.native_value)
}

fn default_permission(harness: &NativeHarness) -> Option<NativeOptionValue> {
    harness
        .permissions
        .options
        .iter()
        .find(|option| option.id == harness.permissions.default)
        .map(|option| NativeOptionValue {
            id: option.id.clone(),
            native_value: option.native_value.clone(),
        })
}
