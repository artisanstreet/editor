//! Static manifest types, snapshot decoding, and reference validation.

use std::collections::HashSet;

use serde_json::Value;
use thiserror::Error;

use crate::validation::{
    array, parse_harness, parse_model, required_string, required_value, value_object,
};
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
    pub(crate) fn invalid(path: impl Into<String>, message: impl Into<String>) -> Self {
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
    /// Whether the picker suppresses this harness (and its tab).
    ///
    /// Hidden harnesses stay fully decodable so stored selections still
    /// resolve; runtime discovery may additionally hide harnesses whose
    /// engine is not installed on this machine.
    pub hidden: bool,
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
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent provider capability flags, not mutually exclusive states"
)]
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
    ///
    /// # Errors
    ///
    /// Returns [`NativeModelCatalogError`] when the text is not valid JSON or
    /// violates any manifest schema or bound rule.
    pub fn from_json_str(json: &str) -> Result<Self, NativeModelCatalogError> {
        let value: Value = serde_json::from_str(json)
            .map_err(|error| NativeModelCatalogError::invalid("$", error.to_string()))?;
        Self::from_value(&value)
    }

    /// Decodes a manifest from an already parsed JSON value.
    ///
    /// # Errors
    ///
    /// Returns [`NativeModelCatalogError`] when the value violates any
    /// manifest schema or bound rule.
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
