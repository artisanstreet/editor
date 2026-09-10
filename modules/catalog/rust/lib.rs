//! Typed, offline-readable model catalog and runtime admission policy.
//!
//! The bundled JSON is a checked-in snapshot of the catalog crate's real
//! ModelManifest. This module deliberately uses serde_json::Value with
//! explicit validation to preserve every capability field. Forge and the
//! native picker share these definitions and reject unsupported selections
//! through the same policy boundary.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use std::collections::HashSet;

use serde_json::{Map, Value};
use thiserror::Error;

pub mod wire;

/// The exact TypeScript source that produced the bundled manifest.
pub const NATIVE_MODEL_CATALOG_SOURCE: &str = "modules/catalog/src/model-manifest.ts";
/// The revision encoded by the bundled manifest snapshot.
pub const NATIVE_MODEL_CATALOG_REVISION: &str = "2026-08-21.2";
/// The complete static manifest bundled with the native selector.
pub const NATIVE_MODEL_CATALOG_JSON: &str = include_str!("native_model_catalog.json");

/// A failure while decoding or validating the checked-in catalog snapshot.
#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum NativeModelCatalogError {
    /// The JSON or one of its typed fields is malformed.
    #[error("invalid native model catalog at {path}: {message}")]
    Invalid {
        /// JSON path or logical location of the malformed value.
        path: String,
        /// Human-readable validation detail.
        message: String,
    },
}

impl NativeModelCatalogError {
    fn invalid(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Invalid {
            path: path.into(),
            message: message.into(),
        }
    }
}

/// The static provider descriptor used by model rows.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeModelProvider {
    /// Stable provider identifier.
    pub id: String,
    /// Exact provider label from the source manifest.
    pub label: String,
}

/// A gateway exposed by a harness.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeModelGateway {
    /// Stable gateway identifier.
    pub id: String,
    /// Exact gateway kind (managed or provider-direct in the snapshot).
    pub kind: String,
    /// Exact display label.
    pub label: String,
}

/// One harness permission choice from the catalog.
#[derive(Clone, Debug, PartialEq)]
pub struct NativePermissionOption {
    /// Approval behavior exposed by the harness.
    pub approval_behavior: String,
    /// Whether this option is always or dynamically available.
    pub availability: String,
    /// Exact explanatory copy.
    pub description: String,
    /// Scope of edits admitted by this option.
    pub edit_scope: String,
    /// Stable catalog option identifier.
    pub id: String,
    /// Exact display label.
    pub label: String,
    /// Native value sent to the harness.
    pub native_value: String,
    /// Safety boundary communicated by the harness.
    pub safety_boundary: String,
}

/// Harness-level permission capability.
#[derive(Clone, Debug, PartialEq)]
pub struct NativePermissionCapability {
    /// Default permission option identifier.
    pub default: String,
    /// Permission options in source order.
    pub options: Vec<NativePermissionOption>,
}

/// A static engine/harness descriptor.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeHarness {
    /// Optional compaction model selected by this harness.
    pub compaction_default_model_id: Option<String>,
    /// Stable harness identifier.
    pub id: String,
    /// Configured provider gateways.
    pub gateways: Vec<NativeModelGateway>,
    /// Exact display label.
    pub label: String,
    /// Permission options accepted by the harness.
    pub permissions: NativePermissionCapability,
}

/// An optional native context-window configuration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeContextConfig {
    /// Native context window value sent to a provider adapter.
    pub model_context_window: u64,
}

/// One context-window choice.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeContextWindowOption {
    /// Optional warning copy.
    pub advisory: Option<String>,
    /// Optional explanatory copy.
    pub description: Option<String>,
    /// Stable catalog option identifier.
    pub id: String,
    /// Exact display label.
    pub label: String,
    /// Optional native configuration object.
    pub native_config: Option<NativeContextConfig>,
    /// Exact native suffix used by adapters that encode the choice in a model
    /// name.
    pub native_suffix: String,
    /// Context capacity represented by this option.
    pub tokens: u64,
}

/// A model's context-window capability.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeContextWindowCapability {
    /// Capability availability (configurable in the current manifest).
    pub availability: String,
    /// Default context option identifier.
    pub default: String,
    /// Options in source order.
    pub options: Vec<NativeContextWindowOption>,
}

/// One reasoning/thinking choice.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeThinkingOption {
    /// Optional warning copy.
    pub advisory: Option<String>,
    /// Optional explanatory copy.
    pub description: Option<String>,
    /// Economics classification from the source manifest.
    pub economics: String,
    /// Stable catalog option identifier.
    pub id: String,
    /// Native value sent to the harness.
    pub native_value: String,
    /// Presentation grouping (base or special in the current manifest).
    pub presentation_group: String,
}

/// Reasoning capability for one model.
#[derive(Clone, Debug, PartialEq)]
pub enum NativeThinkingCapability {
    /// The model exposes no selectable reasoning option.
    Unavailable,
    /// The model delegates reasoning to a native control and has no
    /// selectable option at this layer.
    Native {
        /// Exact native-control description.
        description: String,
    },
    /// The model exposes the listed selectable options.
    Supported {
        /// Default option identifier.
        default: String,
        /// Options in source order.
        options: Vec<NativeThinkingOption>,
    },
}

/// One speed choice.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeSpeedOption {
    /// Availability classification from the source manifest.
    pub availability: String,
    /// Billing/consumption basis.
    pub consumption_basis: String,
    /// Optional total consumption multiplier.
    pub consumption_multiplier: Option<f64>,
    /// Optional input consumption multiplier.
    pub input_consumption_multiplier: Option<f64>,
    /// Optional output consumption multiplier.
    pub output_consumption_multiplier: Option<f64>,
    /// Whether this is the capability default.
    pub default: bool,
    /// Exact explanatory copy.
    pub description: String,
    /// The source optional disabled flag, preserved exactly.
    pub disabled: Option<bool>,
    /// Stable catalog option identifier.
    pub id: String,
    /// Exact display label.
    pub label: String,
    /// Native value sent to the harness.
    pub native_value: String,
    /// Optional provenance link.
    pub source_url: Option<String>,
    /// Optional speed multiplier.
    pub speed_multiplier: Option<f64>,
    /// Optional verification date.
    pub verified_at: Option<String>,
}

/// All capability axes presented by a model preview.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeModelCapabilities {
    /// Optional legacy context token count.
    pub context_window_tokens: Option<u64>,
    /// Optional configurable context-window capability.
    pub context_window: Option<NativeContextWindowCapability>,
    /// Whether image input is supported.
    pub image_input: bool,
    /// Whether local tools are supported.
    pub local_tools: bool,
    /// Whether MCP is supported.
    pub mcp: bool,
    /// Optional output-token limit.
    pub output_tokens: Option<u64>,
    /// Optional reasoning-display mode.
    pub reasoning_display: Option<String>,
    /// Speed choices in source order.
    pub speed_options: Vec<NativeSpeedOption>,
    /// Reasoning/thinking capability.
    pub thinking: NativeThinkingCapability,
    /// Whether web search is supported.
    pub web_search: bool,
}

/// Optional model pricing metadata, retained when supplied by the manifest.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeModelCost {
    /// Input USD per million tokens.
    pub input_usd_per_million: Option<f64>,
    /// Output USD per million tokens.
    pub output_usd_per_million: Option<f64>,
}

/// A disabled model definition and its truthful reason.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeDisabledModel {
    /// Exact reason supplied by the catalog.
    pub reason: String,
}

/// The routing declaration attached to a model definition.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum NativeModelRouting {
    /// Use the harness's default route.
    Default,
    /// Use a named gateway.
    Gateway {
        /// Gateway identifier.
        gateway_id: String,
    },
    /// Use a named provider route.
    ProviderRoute {
        /// Provider-route identifier.
        provider_route_id: String,
    },
}

/// Exact native routing identity for a catalog model.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct NativeModelSelection {
    /// Native model identifier selected by the route.
    pub model_id: String,
    /// Provider route identifier.
    pub provider_route_id: String,
    /// Optional native variant identifier.
    pub variant_id: Option<String>,
}

/// A complete static model definition.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeModelDefinition {
    /// Stable catalog identifier used by favorites and policy events.
    pub id: String,
    /// Exact model display name.
    pub name: String,
    /// Native model identifier.
    pub native_model_id: String,
    /// Optional exact model description.
    pub description: Option<String>,
    /// Harness/engine that runs this model.
    pub harness: String,
    /// Provider identifier.
    pub provider: String,
    /// Routing declaration.
    pub routing: NativeModelRouting,
    /// Optional native route/variant identity.
    pub native_selection: Option<NativeModelSelection>,
    /// Catalog status (curated, dynamic, or prototype).
    pub status: String,
    /// Optional upstream model identifier.
    pub upstream_model_id: Option<String>,
    /// Optional metadata confidence marker.
    pub metadata_confidence: Option<String>,
    /// Optional pricing metadata.
    pub cost: Option<NativeModelCost>,
    /// Optional disabled state.
    pub disabled: Option<NativeDisabledModel>,
    /// Complete model capability set.
    pub capabilities: NativeModelCapabilities,
}

/// The static manifest represented by the bundled JSON.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeModelManifest {
    /// Manifest revision.
    pub revision: String,
    /// Providers in source order.
    pub providers: Vec<NativeModelProvider>,
    /// Harnesses in source order.
    pub harnesses: Vec<NativeHarness>,
    /// Models in source order.
    pub models: Vec<NativeModelDefinition>,
}

impl NativeModelManifest {
    /// Decodes and validates a complete model-manifest JSON document.
    pub fn from_json_str(json: &str) -> Result<Self, NativeModelCatalogError> {
        let value: Value = serde_json::from_str(json)
            .map_err(|error| NativeModelCatalogError::invalid("$", error.to_string()))?;
        Self::from_value(&value)
    }

    /// Decodes a manifest from an already parsed JSON value.
    pub fn from_value(value: &Value) -> Result<Self, NativeModelCatalogError> {
        let object = value_object(value, "$")?;
        let revision = required_string(object, "revision", "$")?;

        let providers_value = required_value(object, "providers", "$")?;
        let providers_array = array(providers_value, "$.providers")?;
        let providers = providers_array
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let path = format!("$.providers[{index}]");
                let object = value_object(value, &path)?;
                Ok(NativeModelProvider {
                    id: required_string(object, "id", &path)?,
                    label: required_string(object, "label", &path)?,
                })
            })
            .collect::<Result<Vec<_>, NativeModelCatalogError>>()?;

        let harnesses_value = required_value(object, "harnesses", "$")?;
        let harnesses_array = array(harnesses_value, "$.harnesses")?;
        let harnesses = harnesses_array
            .iter()
            .enumerate()
            .map(|(index, value)| parse_harness(value, index))
            .collect::<Result<Vec<_>, NativeModelCatalogError>>()?;

        let models_value = required_value(object, "models", "$")?;
        let models_array = array(models_value, "$.models")?;
        let models = models_array
            .iter()
            .enumerate()
            .map(|(index, value)| parse_model(value, index))
            .collect::<Result<Vec<_>, NativeModelCatalogError>>()?;

        let manifest = Self {
            revision,
            providers,
            harnesses,
            models,
        };
        manifest.validate_references()?;
        Ok(manifest)
    }

    fn validate_references(&self) -> Result<(), NativeModelCatalogError> {
        let mut provider_ids = HashSet::new();
        for provider in &self.providers {
            if !provider_ids.insert(provider.id.as_str()) {
                return Err(NativeModelCatalogError::invalid(
                    "$.providers",
                    format!("duplicate provider id '{}'", provider.id),
                ));
            }
        }

        let mut harness_ids = HashSet::new();
        for harness in &self.harnesses {
            if !harness_ids.insert(harness.id.as_str()) {
                return Err(NativeModelCatalogError::invalid(
                    "$.harnesses",
                    format!("duplicate harness id '{}'", harness.id),
                ));
            }
        }

        let mut model_ids = HashSet::new();
        for model in &self.models {
            if !model_ids.insert(model.id.as_str()) {
                return Err(NativeModelCatalogError::invalid(
                    "$.models",
                    format!("duplicate model id '{}'", model.id),
                ));
            }
            if !provider_ids.contains(model.provider.as_str()) {
                return Err(NativeModelCatalogError::invalid(
                    format!("model {} provider", model.id),
                    format!("unknown provider '{}'", model.provider),
                ));
            }
            if !harness_ids.contains(model.harness.as_str()) {
                return Err(NativeModelCatalogError::invalid(
                    format!("model {} harness", model.id),
                    format!("unknown harness '{}'", model.harness),
                ));
            }
        }
        Ok(())
    }

    /// Finds a model by its exact catalog identifier.
    #[must_use]
    pub fn model(&self, model_id: &str) -> Option<&NativeModelDefinition> {
        self.models.iter().find(|model| model.id == model_id)
    }

    /// Finds a harness by its exact identifier.
    #[must_use]
    pub fn harness(&self, harness_id: &str) -> Option<&NativeHarness> {
        self.harnesses
            .iter()
            .find(|harness| harness.id == harness_id)
    }

    /// Finds a provider by its exact identifier.
    #[must_use]
    pub fn provider(&self, provider_id: &str) -> Option<&NativeModelProvider> {
        self.providers
            .iter()
            .find(|provider| provider.id == provider_id)
    }
}

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
    /// Decodes the bundled full manifest in an offline, non-runnable state.
    pub fn offline() -> Result<Self, NativeModelCatalogError> {
        let manifest = NativeModelManifest::from_json_str(NATIVE_MODEL_CATALOG_JSON)?;
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
                reason: format!("Model '{}' is not in this catalog.", model_id),
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
                    reason: format!("Route '{}' is not available in this runtime.", route_id),
                };
            };
            if route.status != NativeModelRouteStatus::Available {
                return NativeModelSelectability::Unavailable {
                    reason: route
                        .unavailable_reason
                        .clone()
                        .unwrap_or_else(|| format!("Route '{}' is unavailable.", route_id)),
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
    pub fn selection_policy_for_model(
        &self,
        model_id: &str,
    ) -> Result<NativeModelPolicy, NativePolicyValidationError> {
        let policy = self.preview_policy_for_model(model_id)?;
        self.validate_selection_policy(&policy)?;
        Ok(policy)
    }

    /// Checks catalog identity, options, and explicit model retirement only.
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
            context_window: policy
                .context_window
                .clone()
                .filter(|value| context_value(model, value).is_some()),
            permission: policy
                .permission
                .clone()
                .filter(|value| permission_value(harness, value).is_some()),
        };
        self.validate_policy(&rebased).ok().map(|_| rebased)
    }

    /// Validates exact model, route, and option membership without requiring
    /// the model to be currently runnable. This is used for authoritative
    /// policies that may be displayed while a runtime is disconnected.
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
                    group.unavailable_reason = row.unavailable_reason.clone();
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
            .or_else(|| match model_routing {
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
        .or_else(|| match &model.routing {
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

fn value_object<'a>(
    value: &'a Value,
    path: &str,
) -> Result<&'a Map<String, Value>, NativeModelCatalogError> {
    value
        .as_object()
        .ok_or_else(|| NativeModelCatalogError::invalid(path, "expected an object"))
}

fn array<'a>(value: &'a Value, path: &str) -> Result<&'a Vec<Value>, NativeModelCatalogError> {
    value
        .as_array()
        .ok_or_else(|| NativeModelCatalogError::invalid(path, "expected an array"))
}

fn required_value<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    path: &str,
) -> Result<&'a Value, NativeModelCatalogError> {
    object.get(key).ok_or_else(|| {
        NativeModelCatalogError::invalid(format!("{path}.{key}"), "missing required value")
    })
}

fn optional_value<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    object.get(key).filter(|value| !value.is_null())
}

fn required_string(
    object: &Map<String, Value>,
    key: &str,
    path: &str,
) -> Result<String, NativeModelCatalogError> {
    let value = required_value(object, key, path)?;
    let string = value.as_str().ok_or_else(|| {
        NativeModelCatalogError::invalid(format!("{path}.{key}"), "expected a string")
    })?;
    if string.is_empty() {
        return Err(NativeModelCatalogError::invalid(
            format!("{path}.{key}"),
            "expected a non-empty string",
        ));
    }
    Ok(string.to_owned())
}

/// Reads a required source-schema string whose empty value is meaningful.
///
/// `context_window.options[].native_suffix` uses `""` for the provider's
/// ordinary context size. Other identifiers continue through
/// [`required_string`] and remain non-empty.
fn required_string_allow_empty(
    object: &Map<String, Value>,
    key: &str,
    path: &str,
) -> Result<String, NativeModelCatalogError> {
    let value = required_value(object, key, path)?;
    value.as_str().map(str::to_owned).ok_or_else(|| {
        NativeModelCatalogError::invalid(format!("{path}.{key}"), "expected a string")
    })
}

fn optional_string(
    object: &Map<String, Value>,
    key: &str,
    path: &str,
) -> Result<Option<String>, NativeModelCatalogError> {
    let Some(value) = optional_value(object, key) else {
        return Ok(None);
    };
    let string = value.as_str().ok_or_else(|| {
        NativeModelCatalogError::invalid(format!("{path}.{key}"), "expected a string")
    })?;
    if string.is_empty() {
        return Err(NativeModelCatalogError::invalid(
            format!("{path}.{key}"),
            "expected a non-empty string when supplied",
        ));
    }
    Ok(Some(string.to_owned()))
}

fn required_bool(
    object: &Map<String, Value>,
    key: &str,
    path: &str,
) -> Result<bool, NativeModelCatalogError> {
    required_value(object, key, path)?
        .as_bool()
        .ok_or_else(|| NativeModelCatalogError::invalid(format!("{path}.{key}"), "expected a bool"))
}

fn required_u64(
    object: &Map<String, Value>,
    key: &str,
    path: &str,
) -> Result<u64, NativeModelCatalogError> {
    required_value(object, key, path)?.as_u64().ok_or_else(|| {
        NativeModelCatalogError::invalid(format!("{path}.{key}"), "expected an integer")
    })
}

fn optional_u64(
    object: &Map<String, Value>,
    key: &str,
    path: &str,
) -> Result<Option<u64>, NativeModelCatalogError> {
    let Some(value) = optional_value(object, key) else {
        return Ok(None);
    };
    value.as_u64().map(Some).ok_or_else(|| {
        NativeModelCatalogError::invalid(format!("{path}.{key}"), "expected an integer")
    })
}

fn optional_f64(
    object: &Map<String, Value>,
    key: &str,
    path: &str,
) -> Result<Option<f64>, NativeModelCatalogError> {
    let Some(value) = optional_value(object, key) else {
        return Ok(None);
    };
    value.as_f64().map(Some).ok_or_else(|| {
        NativeModelCatalogError::invalid(format!("{path}.{key}"), "expected a number")
    })
}

fn optional_bool(
    object: &Map<String, Value>,
    key: &str,
    path: &str,
) -> Result<Option<bool>, NativeModelCatalogError> {
    let Some(value) = optional_value(object, key) else {
        return Ok(None);
    };
    value
        .as_bool()
        .map(Some)
        .ok_or_else(|| NativeModelCatalogError::invalid(format!("{path}.{key}"), "expected a bool"))
}

fn parse_harness(value: &Value, index: usize) -> Result<NativeHarness, NativeModelCatalogError> {
    let path = format!("$.harnesses[{index}]");
    let object = value_object(value, &path)?;
    let gateways = array(
        required_value(object, "gateways", &path)?,
        &format!("{path}.gateways"),
    )?
    .iter()
    .enumerate()
    .map(|(index, value)| {
        let path = format!("{path}.gateways[{index}]");
        let object = value_object(value, &path)?;
        Ok(NativeModelGateway {
            id: required_string(object, "id", &path)?,
            kind: required_string(object, "kind", &path)?,
            label: required_string(object, "label", &path)?,
        })
    })
    .collect::<Result<Vec<_>, NativeModelCatalogError>>()?;
    let permissions_value = required_value(object, "permissions", &path)?;
    let permissions_path = format!("{path}.permissions");
    let permissions_object = value_object(permissions_value, &permissions_path)?;
    let permission_options = array(
        required_value(permissions_object, "options", &permissions_path)?,
        &format!("{permissions_path}.options"),
    )?
    .iter()
    .enumerate()
    .map(|(index, value)| {
        let path = format!("{permissions_path}.options[{index}]");
        let object = value_object(value, &path)?;
        Ok(NativePermissionOption {
            approval_behavior: required_string(object, "approval_behavior", &path)?,
            availability: required_string(object, "availability", &path)?,
            description: required_string(object, "description", &path)?,
            edit_scope: required_string(object, "edit_scope", &path)?,
            id: required_string(object, "id", &path)?,
            label: required_string(object, "label", &path)?,
            native_value: required_string(object, "native_value", &path)?,
            safety_boundary: required_string(object, "safety_boundary", &path)?,
        })
    })
    .collect::<Result<Vec<_>, NativeModelCatalogError>>()?;
    Ok(NativeHarness {
        compaction_default_model_id: optional_string(object, "compaction_default_model_id", &path)?,
        id: required_string(object, "id", &path)?,
        gateways,
        label: required_string(object, "label", &path)?,
        permissions: NativePermissionCapability {
            default: required_string(permissions_object, "default", &permissions_path)?,
            options: permission_options,
        },
    })
}

fn parse_model(
    value: &Value,
    index: usize,
) -> Result<NativeModelDefinition, NativeModelCatalogError> {
    let path = format!("$.models[{index}]");
    let object = value_object(value, &path)?;
    let routing_value = required_value(object, "routing", &path)?;
    let routing_path = format!("{path}.routing");
    let routing_object = value_object(routing_value, &routing_path)?;
    let routing_kind = required_string(routing_object, "kind", &routing_path)?;
    let routing = match routing_kind.as_str() {
        "default" => NativeModelRouting::Default,
        "gateway" => NativeModelRouting::Gateway {
            gateway_id: required_string(routing_object, "gateway_id", &routing_path)?,
        },
        "provider-route" => NativeModelRouting::ProviderRoute {
            provider_route_id: required_string(routing_object, "provider_route_id", &routing_path)?,
        },
        other => {
            return Err(NativeModelCatalogError::invalid(
                routing_path,
                format!("unknown routing kind '{other}'"),
            ));
        }
    };

    let native_selection = optional_value(object, "native_selection")
        .map(|value| {
            let path = format!("{path}.native_selection");
            let object = value_object(value, &path)?;
            Ok(NativeModelSelection {
                model_id: required_string(object, "model_id", &path)?,
                provider_route_id: required_string(object, "provider_route_id", &path)?,
                variant_id: optional_string(object, "variant_id", &path)?,
            })
        })
        .transpose()?;

    let capabilities_value = required_value(object, "capabilities", &path)?;
    let capabilities_path = format!("{path}.capabilities");
    let capabilities_object = value_object(capabilities_value, &capabilities_path)?;
    let speed_options = array(
        required_value(capabilities_object, "speed_options", &capabilities_path)?,
        &format!("{capabilities_path}.speed_options"),
    )?
    .iter()
    .enumerate()
    .map(|(index, value)| parse_speed_option(value, &capabilities_path, index))
    .collect::<Result<Vec<_>, NativeModelCatalogError>>()?;
    let thinking = parse_thinking(
        required_value(capabilities_object, "thinking", &capabilities_path)?,
        &capabilities_path,
    )?;
    let context_window = optional_value(capabilities_object, "context_window")
        .map(|value| parse_context_capability(value, &capabilities_path))
        .transpose()?;
    let cost = optional_value(object, "cost")
        .map(|value| {
            let path = format!("{path}.cost");
            let object = value_object(value, &path)?;
            Ok(NativeModelCost {
                input_usd_per_million: optional_f64(object, "input_usd_per_million", &path)?,
                output_usd_per_million: optional_f64(object, "output_usd_per_million", &path)?,
            })
        })
        .transpose()?;
    let disabled = optional_value(object, "disabled")
        .map(|value| {
            let path = format!("{path}.disabled");
            let object = value_object(value, &path)?;
            Ok(NativeDisabledModel {
                reason: required_string(object, "reason", &path)?,
            })
        })
        .transpose()?;

    Ok(NativeModelDefinition {
        id: required_string(object, "id", &path)?,
        name: required_string(object, "name", &path)?,
        native_model_id: required_string(object, "native_model_id", &path)?,
        description: optional_string(object, "description", &path)?,
        harness: required_string(object, "harness", &path)?,
        provider: required_string(object, "provider", &path)?,
        routing,
        native_selection,
        status: required_string(object, "status", &path)?,
        upstream_model_id: optional_string(object, "upstream_model_id", &path)?,
        metadata_confidence: optional_string(object, "metadata_confidence", &path)?,
        cost,
        disabled,
        capabilities: NativeModelCapabilities {
            context_window_tokens: optional_u64(
                capabilities_object,
                "context_window_tokens",
                &capabilities_path,
            )?,
            context_window,
            image_input: required_bool(capabilities_object, "image_input", &capabilities_path)?,
            local_tools: required_bool(capabilities_object, "local_tools", &capabilities_path)?,
            mcp: required_bool(capabilities_object, "mcp", &capabilities_path)?,
            output_tokens: optional_u64(capabilities_object, "output_tokens", &capabilities_path)?,
            reasoning_display: optional_string(
                capabilities_object,
                "reasoning_display",
                &capabilities_path,
            )?,
            speed_options,
            thinking,
            web_search: required_bool(capabilities_object, "web_search", &capabilities_path)?,
        },
    })
}

fn parse_thinking(
    value: &Value,
    path: &str,
) -> Result<NativeThinkingCapability, NativeModelCatalogError> {
    let thinking_path = format!("{path}.thinking");
    let object = value_object(value, &thinking_path)?;
    match required_string(object, "availability", &thinking_path)?.as_str() {
        "unavailable" => Ok(NativeThinkingCapability::Unavailable),
        "native" => Ok(NativeThinkingCapability::Native {
            description: required_string(object, "description", &thinking_path)?,
        }),
        "supported" => {
            let options = array(
                required_value(object, "options", &thinking_path)?,
                &format!("{thinking_path}.options"),
            )?
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let path = format!("{thinking_path}.options[{index}]");
                let object = value_object(value, &path)?;
                Ok(NativeThinkingOption {
                    advisory: optional_string(object, "advisory", &path)?,
                    description: optional_string(object, "description", &path)?,
                    economics: required_string(object, "economics", &path)?,
                    id: required_string(object, "id", &path)?,
                    native_value: required_string(object, "native_value", &path)?,
                    presentation_group: required_string(object, "presentation_group", &path)?,
                })
            })
            .collect::<Result<Vec<_>, NativeModelCatalogError>>()?;
            Ok(NativeThinkingCapability::Supported {
                default: required_string(object, "default", &thinking_path)?,
                options,
            })
        }
        availability => Err(NativeModelCatalogError::invalid(
            format!("{thinking_path}.availability"),
            format!("unknown thinking availability '{availability}'"),
        )),
    }
}

fn parse_speed_option(
    value: &Value,
    parent_path: &str,
    index: usize,
) -> Result<NativeSpeedOption, NativeModelCatalogError> {
    let path = format!("{parent_path}.speed_options[{index}]");
    let object = value_object(value, &path)?;
    Ok(NativeSpeedOption {
        availability: required_string(object, "availability", &path)?,
        consumption_basis: required_string(object, "consumption_basis", &path)?,
        consumption_multiplier: optional_f64(object, "consumption_multiplier", &path)?,
        input_consumption_multiplier: optional_f64(object, "input_consumption_multiplier", &path)?,
        output_consumption_multiplier: optional_f64(
            object,
            "output_consumption_multiplier",
            &path,
        )?,
        default: required_bool(object, "default", &path)?,
        description: required_string(object, "description", &path)?,
        disabled: optional_bool(object, "disabled", &path)?,
        id: required_string(object, "id", &path)?,
        label: required_string(object, "label", &path)?,
        native_value: required_string(object, "native_value", &path)?,
        source_url: optional_string(object, "source_url", &path)?,
        speed_multiplier: optional_f64(object, "speed_multiplier", &path)?,
        verified_at: optional_string(object, "verified_at", &path)?,
    })
}

fn parse_context_capability(
    value: &Value,
    parent_path: &str,
) -> Result<NativeContextWindowCapability, NativeModelCatalogError> {
    let path = format!("{parent_path}.context_window");
    let object = value_object(value, &path)?;
    let options = array(
        required_value(object, "options", &path)?,
        &format!("{path}.options"),
    )?
    .iter()
    .enumerate()
    .map(|(index, value)| {
        let path = format!("{path}.options[{index}]");
        let object = value_object(value, &path)?;
        let native_config = optional_value(object, "native_config")
            .map(|value| {
                let path = format!("{path}.native_config");
                let object = value_object(value, &path)?;
                Ok(NativeContextConfig {
                    model_context_window: required_u64(object, "model_context_window", &path)?,
                })
            })
            .transpose()?;
        Ok(NativeContextWindowOption {
            advisory: optional_string(object, "advisory", &path)?,
            description: optional_string(object, "description", &path)?,
            id: required_string(object, "id", &path)?,
            label: required_string(object, "label", &path)?,
            native_config,
            native_suffix: required_string_allow_empty(object, "native_suffix", &path)?,
            tokens: required_u64(object, "tokens", &path)?,
        })
    })
    .collect::<Result<Vec<_>, NativeModelCatalogError>>()?;
    Ok(NativeContextWindowCapability {
        availability: required_string(object, "availability", &path)?,
        default: required_string(object, "default", &path)?,
        options,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offline() -> NativeModelCatalog {
        NativeModelCatalog::offline().expect("the checked-in catalog must decode")
    }

    #[test]
    fn bundled_snapshot_preserves_real_manifest_provenance_and_shape() {
        let catalog = offline();
        assert_eq!(catalog.provenance.source, NATIVE_MODEL_CATALOG_SOURCE);
        assert_eq!(catalog.provenance.revision, NATIVE_MODEL_CATALOG_REVISION);
        assert_eq!(catalog.manifest.revision, NATIVE_MODEL_CATALOG_REVISION);
        assert_eq!(catalog.manifest.providers.len(), 16);
        assert_eq!(catalog.manifest.harnesses.len(), 6);
        assert_eq!(catalog.manifest.models.len(), 35);
        assert!(catalog.runnable_harness_ids.is_empty());
        assert!(catalog.routes.is_empty());
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
