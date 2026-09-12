//! Bounded `OpenCode2` model discovery and adapter-neutral normalization.
//!
//! The only accepted source is the authenticated `/api/model` response from
//! the certified engine owned by the sibling operation. This module keeps the
//! response parser independent of process and HTTP custody: no raw JSON is
//! retained in a result, and every returned string/count/byte allocation is
//! bounded before it is created.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use std::collections::{BTreeMap, HashSet};
use std::fmt;

use super::consts::HEX_UPPER;
use base64::Engine as _;
use serde_json::Value;
use thiserror::Error;

/// Hard upper bound for one runtime catalog response, independent of the
/// caller's larger generic JSON bound.
pub(crate) const MAX_CATALOG_RESPONSE_BYTES: usize = 8 * 1024 * 1024;
/// Maximum number of source models accepted from `OpenCode`.
pub(crate) const MAX_CATALOG_MODELS: usize = 512;
/// Maximum number of normalized rows after expanding variants.
pub(crate) const MAX_NORMALIZED_CATALOG_MODELS: usize = 2048;
/// Maximum number of variants on one source model.
pub(crate) const MAX_CATALOG_VARIANTS: usize = 64;
/// Maximum number of capability labels on one source model.
const MAX_CAPABILITY_LABELS: usize = 64;
/// Maximum number of cost records retained from one source model.
const MAX_COST_RECORDS: usize = 8;
/// Maximum UTF-8 bytes in one response identity/metadata string.
const MAX_CATALOG_STRING_BYTES: usize = 4096;
/// Maximum profile identifier bytes in a discovery scope.
const MAX_PROFILE_ID_BYTES: usize = 256;
/// Maximum working-directory bytes in a discovery scope.
const MAX_WORKING_DIRECTORY_BYTES: usize = 4096;
/// Maximum stable canonical bytes retained for revision accounting.
const MAX_CANONICAL_BYTES: usize = 4 * 1024 * 1024;

/// Typed, payload-free scope validation failure.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub(crate) enum CatalogScopeError {
    /// A required scope identity was blank.
    #[error("catalog scope identity is blank")]
    Blank,
    /// A scope identity exceeded its fixed byte bound.
    #[error("catalog scope identity exceeded its bound")]
    TooLong,
    /// A scope identity contained a control byte.
    #[error("catalog scope identity contains a control byte")]
    ControlByte,
    /// Workspace trust was not one of the supported owner values.
    #[error("catalog workspace trust is invalid")]
    InvalidWorkspaceTrust,
}

/// Payload-free failure while decoding or normalizing a runtime catalog.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub(crate) enum CatalogError {
    /// The response body exceeded the catalog-specific or owner bound.
    #[error("catalog response body exceeded limit")]
    BodyTooLarge,
    /// The response body was not valid JSON.
    #[error("catalog response was not valid json")]
    InvalidJson,
    /// The response shape did not match `OpenCode2`'s model schema.
    #[error("catalog response shape was invalid")]
    InvalidShape,
    /// A model field did not satisfy its bounded schema.
    #[error("catalog model field was invalid")]
    InvalidModel,
    /// The source model count exceeded the fixed bound.
    #[error("catalog response exceeded model bound")]
    TooManyModels,
    /// A model's variant count exceeded the fixed bound.
    #[error("catalog response exceeded variant bound")]
    TooManyVariants,
    /// A normalized model identity occurred more than once.
    #[error("catalog response contained duplicate model identity")]
    DuplicateModelIdentity,
    /// The route count exceeded the fixed bound.
    #[error("catalog response exceeded route bound")]
    TooManyRoutes,
    /// The normalized canonical representation exceeded its fixed bound.
    #[error("catalog canonical data exceeded limit")]
    CanonicalDataTooLarge,
    /// The loopback connection could not be opened.
    #[error("catalog tcp connect failed")]
    ConnectFailed,
    /// The HTTP/1 connection could not be established.
    #[error("catalog handshake failed")]
    HandshakeFailed,
    /// The request could not be sent.
    #[error("catalog request send failed")]
    SendFailed,
    /// The engine returned a non-success status.
    #[error("catalog response status was not success")]
    StatusNotSuccess,
    /// The response body could not be read.
    #[error("catalog response body read failed")]
    BodyReadFailed,
    /// The operation deadline elapsed.
    #[error("catalog deadline elapsed")]
    Timeout,
    /// The operation was cancelled.
    #[error("catalog was cancelled")]
    Cancelled,
    /// The owner is shutting down.
    #[error("catalog owner is shutting down")]
    Shutdown,
    /// The HTTP driver failed to join.
    #[error("catalog driver join failed")]
    DriverFailed,
}

/// Exact scope identity supplied by the parent for one catalog read.
#[derive(Clone, Eq, PartialEq)]
pub(crate) struct CatalogScope {
    profile_id: String,
    working_directory: String,
    workspace_trust: String,
}

impl CatalogScope {
    /// Validates and retains the exact profile, directory, and trust values.
    pub(crate) fn new(
        profile_id: impl Into<String>,
        working_directory: impl Into<String>,
        workspace_trust: impl Into<String>,
    ) -> Result<Self, CatalogScopeError> {
        let profile_id = bounded_scope_string(profile_id.into(), MAX_PROFILE_ID_BYTES)?;
        let working_directory =
            bounded_scope_string(working_directory.into(), MAX_WORKING_DIRECTORY_BYTES)?;
        let workspace_trust = workspace_trust.into();
        if !matches!(workspace_trust.as_str(), "safe" | "trusted_project_config") {
            return Err(CatalogScopeError::InvalidWorkspaceTrust);
        }
        Ok(Self {
            profile_id,
            working_directory,
            workspace_trust,
        })
    }

    /// Returns the exact certified profile identity.
    #[must_use]
    pub(crate) fn profile_id(&self) -> &str {
        &self.profile_id
    }

    /// Returns the exact directory used in the `OpenCode` location query.
    #[must_use]
    pub(crate) fn working_directory(&self) -> &str {
        &self.working_directory
    }

    /// Returns the exact parent trust spelling.
    #[must_use]
    pub(crate) fn workspace_trust(&self) -> &str {
        &self.workspace_trust
    }

    /// Ensures the query scope and certified launch describe the same owner
    /// profile and project root.
    #[must_use]
    pub(crate) fn matches_launch(&self, profile_id: &str, project_root: &str) -> bool {
        self.profile_id == profile_id && self.working_directory == project_root
    }
}

impl fmt::Debug for CatalogScope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CatalogScope")
            .field("profile_id", &"<redacted>")
            .field("working_directory", &"<redacted>")
            .field("workspace_trust", &self.workspace_trust)
            .finish()
    }
}

fn bounded_scope_string(value: String, maximum: usize) -> Result<String, CatalogScopeError> {
    if value.is_empty() {
        return Err(CatalogScopeError::Blank);
    }
    if value.len() > maximum {
        return Err(CatalogScopeError::TooLong);
    }
    if value.bytes().any(|byte| byte.is_ascii_control()) {
        return Err(CatalogScopeError::ControlByte);
    }
    Ok(value)
}

/// The schema status reported by `OpenCode2` for one source model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CatalogModelStatus {
    /// A generally available source model.
    Active,
    /// An alpha source model.
    Alpha,
    /// A beta source model.
    Beta,
    /// A deprecated source model.
    Deprecated,
}

impl CatalogModelStatus {
    fn parse(value: &str) -> Result<Self, CatalogError> {
        match value {
            "active" => Ok(Self::Active),
            "alpha" => Ok(Self::Alpha),
            "beta" => Ok(Self::Beta),
            "deprecated" => Ok(Self::Deprecated),
            _ => Err(CatalogError::InvalidModel),
        }
    }

    #[must_use]
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Alpha => "alpha",
            Self::Beta => "beta",
            Self::Deprecated => "deprecated",
        }
    }
}

/// `OpenCode`'s first reported cost record.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CatalogCost {
    /// Input USD per million tokens as reported by `OpenCode`.
    pub(crate) input_usd_per_million: f64,
    /// Output USD per million tokens as reported by `OpenCode`.
    pub(crate) output_usd_per_million: f64,
}

/// Explicit result of whether a runtime row may be selected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CatalogAvailability {
    /// The engine reported an enabled, non-deprecated row.
    Available,
    /// The row remains visible but is not selectable.
    Unavailable {
        /// Constant, truthful reason derived from the engine response.
        reason: &'static str,
    },
}

impl CatalogAvailability {
    /// Returns whether a row is selectable in this runtime snapshot.
    #[cfg(test)]
    #[must_use]
    pub(crate) const fn is_available(self) -> bool {
        matches!(self, Self::Available)
    }

    /// Returns the truthful unavailable reason, if any.
    #[must_use]
    pub(crate) const fn unavailable_reason(self) -> Option<&'static str> {
        match self {
            Self::Available => None,
            Self::Unavailable { reason } => Some(reason),
        }
    }
}

/// Thinking metadata from `/api/model`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CatalogThinkingMetadata {
    /// `OpenCode`'s model schema does not report selectable thinking levels.
    NotReported,
}

/// Capability metadata retained from one live model response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CatalogCapabilities {
    /// The reported input modality labels.
    pub(crate) input: Vec<String>,
    /// The reported output modality labels.
    pub(crate) output: Vec<String>,
    /// The reported context limit.
    pub(crate) context_window_tokens: u64,
    /// The reported output limit.
    pub(crate) output_tokens: u64,
    /// Whether `input` reported image support.
    pub(crate) image_input: bool,
    /// Whether `OpenCode` reported tool support.
    pub(crate) tools: bool,
    /// Explicitly non-inferred thinking metadata.
    pub(crate) thinking: CatalogThinkingMetadata,
}

/// Exact source model/route/variant identity.
#[expect(
    clippy::struct_field_names,
    reason = "fields mirror the provider model/route/variant identity vocabulary; renaming would obscure the mapping"
)]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct CatalogModelSelection {
    /// `OpenCode`'s native model ID.
    pub(crate) model_id: String,
    /// `OpenCode` provider route ID.
    pub(crate) provider_route_id: String,
    /// Optional exact `OpenCode` variant ID.
    pub(crate) variant_id: Option<String>,
}

/// One normalized model row, including unavailable runtime rows.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CatalogModel {
    /// Stable `opencode2:` identity for this route/variant row.
    pub(crate) catalog_id: String,
    /// Source model ID.
    pub(crate) model_id: String,
    /// Native model ID used by `OpenCode`.
    pub(crate) native_model_id: String,
    /// Provider route ID.
    pub(crate) provider_route_id: String,
    /// Exact route/variant selection.
    pub(crate) native_selection: CatalogModelSelection,
    /// Optional exact variant ID.
    pub(crate) variant_id: Option<String>,
    /// Source display name.
    pub(crate) name: String,
    /// Upstream provider model ID.
    pub(crate) upstream_model_id: String,
    /// Source lifecycle status.
    pub(crate) status: CatalogModelStatus,
    /// Source enabled flag.
    pub(crate) enabled: bool,
    /// Truthful runtime selectability.
    pub(crate) availability: CatalogAvailability,
    /// Capability metadata.
    pub(crate) capabilities: CatalogCapabilities,
    /// First reported cost record, if present.
    pub(crate) cost: Option<CatalogCost>,
    /// Provenance marker for all fields in this row.
    pub(crate) metadata_confidence: &'static str,
}

/// Route presentation group matching the Electron adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CatalogRouteGroup {
    /// Stable group identity.
    pub(crate) id: String,
    /// Display label.
    pub(crate) label: String,
    /// Display ordering.
    pub(crate) order: u32,
    /// Whether route labels should be displayed.
    pub(crate) show_route_labels: bool,
}

/// One deduplicated provider route reported by `OpenCode2`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct CatalogRoute {
    /// Owning engine identifier.
    pub(crate) engine_id: &'static str,
    /// Stable provider route ID.
    pub(crate) id: String,
    /// Display label.
    pub(crate) label: String,
    /// Electron-compatible presentation group.
    pub(crate) group: CatalogRouteGroup,
    /// Route availability. A returned route is available; disabled models are
    /// represented at row level instead of being invented as route failures.
    pub(crate) availability: CatalogAvailability,
}

/// Typed raw response after strict bounded schema decoding.
#[derive(Clone, PartialEq)]
pub(crate) struct RawOpenCode2Model {
    pub(crate) id: String,
    pub(crate) model_id: String,
    pub(crate) name: String,
    pub(crate) provider_route_id: String,
    pub(crate) status: CatalogModelStatus,
    pub(crate) enabled: bool,
    pub(crate) context_window_tokens: u64,
    pub(crate) output_tokens: u64,
    pub(crate) input: Vec<String>,
    pub(crate) output: Vec<String>,
    pub(crate) tools: bool,
    pub(crate) costs: Vec<CatalogCost>,
    pub(crate) variants: Vec<String>,
}

impl fmt::Debug for RawOpenCode2Model {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RawOpenCode2Model { <redacted> }")
    }
}

/// Typed raw model response and its bounded wire-byte accounting.
#[derive(Clone, PartialEq)]
pub(crate) struct RawOpenCode2ModelsResponse {
    pub(crate) models: Vec<RawOpenCode2Model>,
    pub(crate) response_bytes: usize,
}

impl fmt::Debug for RawOpenCode2ModelsResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RawOpenCode2ModelsResponse")
            .field("model_count", &self.models.len())
            .field("response_bytes", &self.response_bytes)
            .finish()
    }
}

/// Complete normalized result handed to the root adapter.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CatalogResult {
    /// Stable engine identifier.
    pub(crate) engine_id: String,
    /// Exact discovery scope.
    pub(crate) scope: CatalogScope,
    /// Revision derived from canonical normalized model/route data.
    pub(crate) catalog_revision: String,
    /// Normalized base and variant rows.
    pub(crate) models: Vec<CatalogModel>,
    /// Deduplicated provider routes.
    pub(crate) routes: Vec<CatalogRoute>,
    /// Actual response body bytes read.
    pub(crate) response_bytes: usize,
    /// Canonical bytes hashed for the revision.
    pub(crate) canonical_bytes: usize,
}

impl CatalogResult {
    /// Returns the number of normalized model rows.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn model_count(&self) -> usize {
        self.models.len()
    }

    /// Returns the number of deduplicated routes.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn route_count(&self) -> usize {
        self.routes.len()
    }
}

/// Decodes the exact `OpenCode2` `{ "data": [...] }` response shape.
pub(crate) fn decode_models_response(
    bytes: &[u8],
) -> Result<RawOpenCode2ModelsResponse, CatalogError> {
    if bytes.len() > MAX_CATALOG_RESPONSE_BYTES {
        return Err(CatalogError::BodyTooLarge);
    }
    let value: Value = serde_json::from_slice(bytes).map_err(|_| CatalogError::InvalidJson)?;
    let object = value.as_object().ok_or(CatalogError::InvalidShape)?;
    let data = object
        .get("data")
        .and_then(Value::as_array)
        .ok_or(CatalogError::InvalidShape)?;
    if data.len() > MAX_CATALOG_MODELS {
        return Err(CatalogError::TooManyModels);
    }
    let models = data
        .iter()
        .map(parse_raw_model)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(RawOpenCode2ModelsResponse {
        models,
        response_bytes: bytes.len(),
    })
}

fn parse_raw_model(value: &Value) -> Result<RawOpenCode2Model, CatalogError> {
    let object = value.as_object().ok_or(CatalogError::InvalidModel)?;
    let capabilities = object
        .get("capabilities")
        .and_then(Value::as_object)
        .ok_or(CatalogError::InvalidModel)?;
    let limits = object
        .get("limit")
        .and_then(Value::as_object)
        .ok_or(CatalogError::InvalidModel)?;
    let costs = object
        .get("cost")
        .and_then(Value::as_array)
        .ok_or(CatalogError::InvalidModel)?;
    if costs.len() > MAX_COST_RECORDS {
        return Err(CatalogError::InvalidModel);
    }
    let variants = object
        .get("variants")
        .and_then(Value::as_array)
        .ok_or(CatalogError::InvalidModel)?;
    if variants.len() > MAX_CATALOG_VARIANTS {
        return Err(CatalogError::TooManyVariants);
    }

    Ok(RawOpenCode2Model {
        id: required_string(object.get("id"))?,
        model_id: required_string(object.get("modelID"))?,
        name: required_string(object.get("name"))?,
        provider_route_id: required_string(object.get("providerID"))?,
        status: CatalogModelStatus::parse(&required_string(object.get("status"))?)?,
        enabled: object
            .get("enabled")
            .and_then(Value::as_bool)
            .ok_or(CatalogError::InvalidModel)?,
        context_window_tokens: required_u64(limits.get("context"))?,
        output_tokens: required_u64(limits.get("output"))?,
        input: string_array(capabilities.get("input"))?,
        output: string_array(capabilities.get("output"))?,
        tools: capabilities
            .get("tools")
            .and_then(Value::as_bool)
            .ok_or(CatalogError::InvalidModel)?,
        costs: costs
            .iter()
            .map(parse_cost)
            .collect::<Result<Vec<_>, _>>()?,
        variants: variants
            .iter()
            .map(|variant| {
                let object = variant.as_object().ok_or(CatalogError::InvalidModel)?;
                required_string(object.get("id"))
            })
            .collect::<Result<Vec<_>, _>>()?,
    })
}

fn required_string(value: Option<&Value>) -> Result<String, CatalogError> {
    let value = value
        .and_then(Value::as_str)
        .ok_or(CatalogError::InvalidModel)?;
    if value.is_empty() || value.len() > MAX_CATALOG_STRING_BYTES {
        return Err(CatalogError::InvalidModel);
    }
    if value.bytes().any(|byte| byte.is_ascii_control()) {
        return Err(CatalogError::InvalidModel);
    }
    Ok(value.to_owned())
}

fn required_u64(value: Option<&Value>) -> Result<u64, CatalogError> {
    value
        .and_then(Value::as_u64)
        .ok_or(CatalogError::InvalidModel)
}

fn string_array(value: Option<&Value>) -> Result<Vec<String>, CatalogError> {
    let values = value
        .and_then(Value::as_array)
        .ok_or(CatalogError::InvalidModel)?;
    if values.len() > MAX_CAPABILITY_LABELS {
        return Err(CatalogError::InvalidModel);
    }
    values
        .iter()
        .map(|value| required_string(Some(value)))
        .collect()
}

fn parse_cost(value: &Value) -> Result<CatalogCost, CatalogError> {
    let object = value.as_object().ok_or(CatalogError::InvalidModel)?;
    let input = object
        .get("input")
        .and_then(Value::as_f64)
        .ok_or(CatalogError::InvalidModel)?;
    let output = object
        .get("output")
        .and_then(Value::as_f64)
        .ok_or(CatalogError::InvalidModel)?;
    if !input.is_finite() || input < 0.0 || !output.is_finite() || output < 0.0 {
        return Err(CatalogError::InvalidModel);
    }
    Ok(CatalogCost {
        input_usd_per_million: input,
        output_usd_per_million: output,
    })
}

/// Normalizes a strict raw response into route/variant-aware rows.
pub(crate) fn normalize_catalog(
    raw: RawOpenCode2ModelsResponse,
    scope: CatalogScope,
) -> Result<CatalogResult, CatalogError> {
    let mut models = Vec::new();
    let mut identities = HashSet::new();
    let mut routes = BTreeMap::new();

    for raw_model in raw.models {
        routes
            .entry(raw_model.provider_route_id.clone())
            .or_insert_with(|| route_for(&raw_model.provider_route_id));

        let variants = std::iter::once(None).chain(raw_model.variants.iter().map(Some));
        for variant in variants {
            if models.len() >= MAX_NORMALIZED_CATALOG_MODELS {
                return Err(CatalogError::TooManyModels);
            }
            let variant_id = variant.cloned();
            let identity = (
                raw_model.id.clone(),
                raw_model.provider_route_id.clone(),
                variant_id.clone(),
            );
            if !identities.insert(identity) {
                return Err(CatalogError::DuplicateModelIdentity);
            }
            let selection = CatalogModelSelection {
                model_id: raw_model.id.clone(),
                provider_route_id: raw_model.provider_route_id.clone(),
                variant_id: variant_id.clone(),
            };
            let availability = if !raw_model.enabled {
                CatalogAvailability::Unavailable {
                    reason: "OpenCode reports this model is disabled.",
                }
            } else if raw_model.status == CatalogModelStatus::Deprecated {
                CatalogAvailability::Unavailable {
                    reason: "OpenCode reports this model is deprecated.",
                }
            } else {
                CatalogAvailability::Available
            };
            models.push(CatalogModel {
                catalog_id: catalog_id(&selection)?,
                model_id: raw_model.id.clone(),
                native_model_id: raw_model.id.clone(),
                provider_route_id: raw_model.provider_route_id.clone(),
                native_selection: selection,
                variant_id,
                name: raw_model.name.clone(),
                upstream_model_id: raw_model.model_id.clone(),
                status: raw_model.status,
                enabled: raw_model.enabled,
                availability,
                capabilities: CatalogCapabilities {
                    input: raw_model.input.clone(),
                    output: raw_model.output.clone(),
                    context_window_tokens: raw_model.context_window_tokens,
                    output_tokens: raw_model.output_tokens,
                    image_input: raw_model.input.iter().any(|input| input == "image"),
                    tools: raw_model.tools,
                    thinking: CatalogThinkingMetadata::NotReported,
                },
                cost: raw_model.costs.first().copied(),
                metadata_confidence: "reported",
            });
        }
    }

    let mut routes = routes.into_values().collect::<Vec<_>>();
    if routes.len() > MAX_CATALOG_MODELS {
        return Err(CatalogError::TooManyRoutes);
    }
    routes.sort_by(|left, right| {
        left.group
            .order
            .cmp(&right.group.order)
            .then_with(|| left.label.cmp(&right.label))
            .then_with(|| left.id.cmp(&right.id))
    });

    let canonical = canonical_bytes(&models, &routes)?;
    let catalog_revision = format!("opencode2-{hash:016x}", hash = stable_hash(&canonical));
    Ok(CatalogResult {
        engine_id: "opencode2".to_owned(),
        scope,
        catalog_revision,
        models,
        routes,
        response_bytes: raw.response_bytes,
        canonical_bytes: canonical.len(),
    })
}

pub(crate) fn route_for(provider_route_id: &str) -> CatalogRoute {
    let (group, label) = match provider_route_id {
        "opencode-go" => (
            CatalogRouteGroup {
                id: "go".to_owned(),
                label: "Go".to_owned(),
                order: 0,
                show_route_labels: false,
            },
            "Go".to_owned(),
        ),
        "opencode" => (
            CatalogRouteGroup {
                id: "zen".to_owned(),
                label: "Zen".to_owned(),
                order: 1,
                show_route_labels: false,
            },
            "Zen".to_owned(),
        ),
        _ => (
            CatalogRouteGroup {
                id: "custom".to_owned(),
                label: "Custom".to_owned(),
                order: 2,
                show_route_labels: true,
            },
            provider_route_id.to_owned(),
        ),
    };
    CatalogRoute {
        engine_id: "opencode2",
        id: provider_route_id.to_owned(),
        label,
        group,
        availability: CatalogAvailability::Available,
    }
}

fn catalog_id(selection: &CatalogModelSelection) -> Result<String, CatalogError> {
    // Keep the same insertion order as the Electron `JSON.stringify` tuple;
    // this is the externally useful `opencode2:` identity, not merely an
    // opaque hash. `serde_json::Map` ordering is feature-dependent, so build
    // this short object explicitly after escaping each bounded string.
    let model_id =
        serde_json::to_string(&selection.model_id).map_err(|_| CatalogError::InvalidModel)?;
    let provider_route_id = serde_json::to_string(&selection.provider_route_id)
        .map_err(|_| CatalogError::InvalidModel)?;
    let bytes = match selection.variant_id.as_deref() {
        Some(variant_id) => {
            let variant_id =
                serde_json::to_string(variant_id).map_err(|_| CatalogError::InvalidModel)?;
            format!(
                "{{\"model_id\":{model_id},\"provider_route_id\":{provider_route_id},\"variant_id\":{variant_id}}}"
            )
            .into_bytes()
        }
        None => format!("{{\"model_id\":{model_id},\"provider_route_id\":{provider_route_id}}}")
            .into_bytes(),
    };
    Ok(format!(
        "opencode2:{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
    ))
}

fn canonical_bytes(
    models: &[CatalogModel],
    routes: &[CatalogRoute],
) -> Result<Vec<u8>, CatalogError> {
    let value = serde_json::json!({
        "models": models.iter().map(canonical_model).collect::<Vec<_>>(),
        "routes": routes.iter().map(canonical_route).collect::<Vec<_>>(),
    });
    let bytes = serde_json::to_vec(&value).map_err(|_| CatalogError::InvalidModel)?;
    if bytes.len() > MAX_CANONICAL_BYTES {
        return Err(CatalogError::CanonicalDataTooLarge);
    }
    Ok(bytes)
}

fn canonical_model(model: &CatalogModel) -> Value {
    let availability = match model.availability {
        CatalogAvailability::Available => serde_json::json!({ "available": true }),
        CatalogAvailability::Unavailable { reason } => {
            serde_json::json!({ "available": false, "reason": reason })
        }
    };
    serde_json::json!({
        "catalog_id": model.catalog_id,
        "model_id": model.model_id,
        "native_model_id": model.native_model_id,
        "provider_route_id": model.provider_route_id,
        "variant_id": model.variant_id,
        "name": model.name,
        "upstream_model_id": model.upstream_model_id,
        "status": model.status.as_str(),
        "enabled": model.enabled,
        "availability": availability,
        "capabilities": {
            "input": model.capabilities.input,
            "output": model.capabilities.output,
            "context_window_tokens": model.capabilities.context_window_tokens,
            "output_tokens": model.capabilities.output_tokens,
            "image_input": model.capabilities.image_input,
            "tools": model.capabilities.tools,
            "thinking": "not_reported",
        },
        "cost": model.cost.map(|cost| serde_json::json!({
            "input_usd_per_million": cost.input_usd_per_million,
            "output_usd_per_million": cost.output_usd_per_million,
        })),
        "metadata_confidence": model.metadata_confidence,
    })
}

fn canonical_route(route: &CatalogRoute) -> Value {
    let availability = match route.availability {
        CatalogAvailability::Available => serde_json::json!({ "available": true }),
        CatalogAvailability::Unavailable { reason } => {
            serde_json::json!({ "available": false, "reason": reason })
        }
    };
    serde_json::json!({
        "engine_id": route.engine_id,
        "id": route.id,
        "label": route.label,
        "group": {
            "id": route.group.id,
            "label": route.group.label,
            "order": route.group.order,
            "show_route_labels": route.group.show_route_labels,
        },
        "availability": availability,
    })
}

fn stable_hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3_u64);
    }
    hash
}

/// Encodes the exact `OpenCode` location query used by the Electron adapter.
/// Spaces use `+`; all other non-unreserved UTF-8 bytes use uppercase `%XX`.
pub(crate) fn location_query(scope: &CatalogScope) -> String {
    let mut encoded = String::with_capacity(scope.working_directory.len().saturating_mul(3));
    for byte in scope.working_directory().as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(char::from(*byte));
            }
            b' ' => encoded.push('+'),
            byte => {
                encoded.push('%');
                encoded.push(char::from(HEX_UPPER[usize::from(byte >> 4)]));
                encoded.push(char::from(HEX_UPPER[usize::from(byte & 0x0f)]));
            }
        }
    }
    format!("location%5Bdirectory%5D={encoded}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope() -> CatalogScope {
        CatalogScope::new("profile-1", r"C:\work space", "safe")
            .expect("fixture scope should validate")
    }

    fn model(id: &str, provider: &str, enabled: bool, status: &str, variants: &[&str]) -> Value {
        serde_json::json!({
            "capabilities": {
                "input": ["text", "image"],
                "output": ["text"],
                "tools": true,
            },
            "cost": [{ "input": 1.25, "output": 2.5 }],
            "enabled": enabled,
            "id": id,
            "limit": { "context": 131_072, "output": 8_192 },
            "modelID": format!("upstream-{id}"),
            "name": format!("Model {id}"),
            "providerID": provider,
            "status": status,
            "variants": variants.iter().map(|id| serde_json::json!({ "id": id })).collect::<Vec<_>>(),
        })
    }

    #[test]
    fn location_query_matches_opencode_url_search_params() {
        assert_eq!(
            location_query(&scope()),
            "location%5Bdirectory%5D=C%3A%5Cwork+space"
        );
    }

    #[test]
    fn normalization_retains_route_variant_identity_and_reported_capabilities() {
        let body = serde_json::json!({
            "data": [model("model-a", "opencode", true, "active", &["high", "fast"])]
        });
        let raw = decode_models_response(&serde_json::to_vec(&body).expect("fixture json"))
            .expect("fixture response should decode");
        let result = normalize_catalog(raw, scope()).expect("fixture catalog should normalize");
        assert_eq!(result.engine_id, "opencode2");
        assert_eq!(result.model_count(), 3);
        assert_eq!(result.route_count(), 1);
        assert_eq!(result.routes[0].group.id, "zen");
        let variant = result
            .models
            .iter()
            .find(|model| model.variant_id.as_deref() == Some("high"))
            .expect("high variant row");
        assert_eq!(variant.native_selection.model_id, "model-a");
        assert_eq!(variant.native_selection.provider_route_id, "opencode");
        assert_eq!(variant.native_selection.variant_id.as_deref(), Some("high"));
        assert_eq!(
            variant.catalog_id,
            "opencode2:eyJtb2RlbF9pZCI6Im1vZGVsLWEiLCJwcm92aWRlcl9yb3V0ZV9pZCI6Im9wZW5jb2RlIiwidmFyaWFudF9pZCI6ImhpZ2gifQ"
        );
        assert_eq!(variant.capabilities.context_window_tokens, 131_072);
        assert!(variant.capabilities.image_input);
        assert!(variant.capabilities.tools);
        assert_eq!(
            variant.capabilities.thinking,
            CatalogThinkingMetadata::NotReported
        );
        assert_eq!(variant.metadata_confidence, "reported");
    }

    #[test]
    fn disabled_and_deprecated_rows_remain_visible_but_unavailable() {
        let body = serde_json::json!({
            "data": [
                model("disabled", "custom", false, "active", &[]),
                model("old", "custom", true, "deprecated", &[]),
            ]
        });
        let raw = decode_models_response(&serde_json::to_vec(&body).expect("fixture json"))
            .expect("fixture response should decode");
        let result = normalize_catalog(raw, scope()).expect("fixture catalog should normalize");
        assert_eq!(result.models.len(), 2);
        assert_eq!(
            result.models[0].availability.unavailable_reason(),
            Some("OpenCode reports this model is disabled.")
        );
        assert_eq!(
            result.models[1].availability.unavailable_reason(),
            Some("OpenCode reports this model is deprecated.")
        );
        assert!(!result.models[0].availability.is_available());
        assert!(!result.models[1].availability.is_available());
    }

    #[test]
    fn empty_catalog_is_valid_and_revision_is_stable() {
        let bytes = br#"{"data":[]}"#;
        let first = normalize_catalog(
            decode_models_response(bytes).expect("empty response should decode"),
            scope(),
        )
        .expect("empty catalog should normalize");
        let second = normalize_catalog(
            decode_models_response(bytes).expect("empty response should decode"),
            scope(),
        )
        .expect("empty catalog should normalize");
        assert!(first.models.is_empty());
        assert!(first.routes.is_empty());
        assert_eq!(first.catalog_revision, second.catalog_revision);
        assert_eq!(first.response_bytes, bytes.len());
    }

    #[test]
    fn malformed_and_overbound_responses_are_rejected_before_normalization() {
        assert_eq!(
            decode_models_response(br"not-json"),
            Err(CatalogError::InvalidJson)
        );
        let too_many = serde_json::json!({
            "data": (0..=MAX_CATALOG_MODELS)
                .map(|index| model(&format!("model-{index}"), "custom", true, "active", &[]))
                .collect::<Vec<_>>(),
        });
        assert_eq!(
            decode_models_response(&serde_json::to_vec(&too_many).expect("fixture json")),
            Err(CatalogError::TooManyModels)
        );
    }
}
