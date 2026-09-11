//! Versioned, bounded JSON transport for a complete native model catalog.
//!
//! The bundled manifest remains the baseline and is never replaced by this
//! module. A wire snapshot carries that baseline plus the owner-supplied
//! runtime layer, provenance, scope, and authoritative favorites; runtime
//! overlay may replace reported fields on bundled rows and append new rows,
//! but every bundled provider and model identity must survive. The schema
//! is intentionally explicit rather than derived from `Debug`: unknown keys,
//! malformed numbers, oversized collections, stale references, and mismatched
//! OpenCode2 native identities are rejected before a snapshot is returned.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use std::collections::HashSet;

use serde_json::{Map, Value, json};
use thiserror::Error;

use crate::{
    NativeCatalogProvenance, NativeCatalogRuntime, NativeCatalogScope, NativeContextConfig,
    NativeContextSelection, NativeContextWindowCapability, NativeContextWindowOption,
    NativeDisabledModel, NativeHarness, NativeModelCapabilities, NativeModelCatalog,
    NativeModelCost, NativeModelDefaults, NativeModelDefinition, NativeModelGateway,
    NativeModelManifest, NativeModelProvider, NativeModelRoute, NativeModelRouteGroup,
    NativeModelRouteStatus, NativeModelRouting, NativeModelSelection, NativeOptionValue,
    NativePermissionOption, NativeSpeedOption, NativeThinkingCapability, NativeThinkingOption,
};

/// Schema version for the complete catalog snapshot.
pub const WIRE_VERSION: u64 = 1;
/// Maximum encoded JSON payload accepted or emitted by this API.
pub const MAX_WIRE_BYTES: usize = 16 * 1024 * 1024;

const MAX_WIRE_PROVIDERS: usize = 1_024;
const MAX_WIRE_HARNESSES: usize = 64;
const MAX_WIRE_MODELS: usize = 4_096;
const MAX_WIRE_ROUTES: usize = 4_096;
const MAX_WIRE_GATEWAYS: usize = 256;
const MAX_WIRE_OPTIONS: usize = 256;
const MAX_WIRE_MODEL_DEFAULTS: usize = 4_096;
// Keep these protocol caps synchronized with artisan_domain::model_favorites;
// catalog cannot depend on domain without creating a crate cycle.
const MAX_FAVORITE_ID_BYTES: usize = 4_096;
const MAX_FAVORITES: usize = 1_024;
const MAX_FAVORITE_SNAPSHOT_BYTES: usize = 8 * 1024 * 1024;
const MAX_WIRE_IDENTIFIER_BYTES: usize = MAX_FAVORITE_ID_BYTES;
const MAX_NATIVE_IDENTIFIER_BYTES: usize = 128;
const MAX_WIRE_TEXT_BYTES: usize = 64 * 1024;
const MAX_SCOPE_PROFILE_BYTES: usize = 256;
const MAX_SCOPE_WORKING_DIRECTORY_BYTES: usize = 4_096;

/// Payload-free failure from catalog wire encoding or decoding.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum NativeModelCatalogWireError {
    /// The payload exceeded [`MAX_WIRE_BYTES`] before JSON parsing.
    #[error("model catalog wire payload is too large")]
    PayloadTooLarge,
    /// The payload was not valid JSON.
    #[error("model catalog wire payload is not valid JSON")]
    InvalidJson,
    /// The JSON shape did not match the versioned schema.
    #[error("model catalog wire shape is invalid")]
    InvalidShape,
    /// An object contained a key outside the explicit schema.
    #[error("model catalog wire contains an unknown field")]
    UnknownField,
    /// A bounded string or required value was malformed.
    #[error("model catalog wire contains an invalid value")]
    InvalidValue,
    /// A string used as an identifier was not a valid bounded identifier.
    #[error("model catalog wire contains an invalid identifier")]
    InvalidIdentifier,
    /// A JSON number was not an exact bounded integer or finite number.
    #[error("model catalog wire contains an invalid number")]
    InvalidNumber,
    /// A collection exceeded its fixed cardinality bound.
    #[error("model catalog wire collection is too large")]
    TooManyItems,
    /// A collection repeated a stable identity.
    #[error("model catalog wire contains a duplicate identity")]
    DuplicateIdentifier,
    /// A runtime value pointed outside the decoded static manifest.
    #[error("model catalog wire contains an unknown reference")]
    UnknownReference,
    /// A route/variant/native model identity was inconsistent.
    #[error("model catalog wire contains an invalid native identity")]
    InvalidNativeIdentity,
    /// A runtime scope was malformed or inconsistent.
    #[error("model catalog wire contains an invalid scope")]
    InvalidScope,
    /// The bundled/static catalog was not a valid complete catalog.
    #[error("model catalog wire contains an invalid catalog")]
    InvalidCatalog,
    /// Serialization failed or produced a payload outside the bound.
    #[error("model catalog wire encoding failed")]
    EncodeFailed,
}

/// Encodes the complete static-plus-runtime catalog snapshot.
///
/// The emitted object is deterministic for a given catalog and contains no
/// debug representation or raw provider envelope. The encoded length is
/// checked after serialization and never exceeds [`MAX_WIRE_BYTES`].
pub fn encode_catalog(
    catalog: &NativeModelCatalog,
) -> Result<Vec<u8>, NativeModelCatalogWireError> {
    validate_catalog_runtime(catalog)?;
    let value = catalog_value(catalog);
    let encoded =
        serde_json::to_vec(&value).map_err(|_| NativeModelCatalogWireError::EncodeFailed)?;
    if encoded.len() > MAX_WIRE_BYTES {
        return Err(NativeModelCatalogWireError::PayloadTooLarge);
    }
    validate_wire_document(&value)?;
    Ok(encoded)
}

/// Decodes and validates one complete catalog snapshot.
///
/// The byte ceiling is checked before `serde_json` sees the input. Every
/// collection is checked before its values are converted into owned domain
/// strings, and runtime references are checked against the decoded manifest.
pub fn decode_catalog(bytes: &[u8]) -> Result<NativeModelCatalog, NativeModelCatalogWireError> {
    if bytes.len() > MAX_WIRE_BYTES {
        return Err(NativeModelCatalogWireError::PayloadTooLarge);
    }
    let value: Value =
        serde_json::from_slice(bytes).map_err(|_| NativeModelCatalogWireError::InvalidJson)?;
    decode_wire_value(&value)
}

/// Computes the exact OpenCode2 catalog identity used by the Electron
/// adapter. The JSON key order is `model_id`, `provider_route_id`, followed by
/// the optional `variant_id`; omitted variants remain omitted rather than
/// becoming `null`.
pub fn opencode2_catalog_id(
    model_id: &str,
    provider_route_id: &str,
    variant_id: Option<&str>,
) -> Result<String, NativeModelCatalogWireError> {
    validate_identifier_str(model_id)?;
    validate_identifier_str(provider_route_id)?;
    if let Some(variant_id) = variant_id {
        validate_identifier_str(variant_id)?;
    }
    let model_id =
        serde_json::to_string(model_id).map_err(|_| NativeModelCatalogWireError::EncodeFailed)?;
    let provider_route_id = serde_json::to_string(provider_route_id)
        .map_err(|_| NativeModelCatalogWireError::EncodeFailed)?;
    let bytes = match variant_id {
        Some(variant_id) => {
            let variant_id = serde_json::to_string(variant_id)
                .map_err(|_| NativeModelCatalogWireError::EncodeFailed)?;
            format!(
                "{{\"model_id\":{model_id},\"provider_route_id\":{provider_route_id},\"variant_id\":{variant_id}}}"
            )
            .into_bytes()
        }
        None => format!("{{\"model_id\":{model_id},\"provider_route_id\":{provider_route_id}}}")
            .into_bytes(),
    };
    let catalog_id = format!("opencode2:{}", base64_url_no_pad(&bytes));
    if catalog_id.len() > MAX_WIRE_IDENTIFIER_BYTES {
        return Err(NativeModelCatalogWireError::InvalidIdentifier);
    }
    Ok(catalog_id)
}

fn catalog_value(catalog: &NativeModelCatalog) -> Value {
    json!({
        "version": WIRE_VERSION,
        "manifest": manifest_value(&catalog.manifest),
        "runtime": runtime_value(&catalog.runtime()),
        "provenance": {
            "source": catalog.provenance.source,
            "revision": catalog.provenance.revision,
        },
    })
}

fn manifest_value(manifest: &NativeModelManifest) -> Value {
    json!({
        "revision": manifest.revision,
        "providers": manifest.providers.iter().map(provider_value).collect::<Vec<_>>(),
        "harnesses": manifest.harnesses.iter().map(harness_value).collect::<Vec<_>>(),
        "models": manifest.models.iter().map(model_value).collect::<Vec<_>>(),
    })
}

fn provider_value(provider: &NativeModelProvider) -> Value {
    json!({"id": provider.id, "label": provider.label})
}

fn gateway_value(gateway: &NativeModelGateway) -> Value {
    json!({"id": gateway.id, "kind": gateway.kind, "label": gateway.label})
}

fn permission_option_value(option: &NativePermissionOption) -> Value {
    json!({
        "approval_behavior": option.approval_behavior,
        "availability": option.availability,
        "description": option.description,
        "edit_scope": option.edit_scope,
        "id": option.id,
        "label": option.label,
        "native_value": option.native_value,
        "safety_boundary": option.safety_boundary,
    })
}

fn harness_value(harness: &NativeHarness) -> Value {
    json!({
        "compaction_default_model_id": harness.compaction_default_model_id,
        "id": harness.id,
        "gateways": harness.gateways.iter().map(gateway_value).collect::<Vec<_>>(),
        "label": harness.label,
        "permissions": {
            "default": harness.permissions.default,
            "options": harness.permissions.options.iter().map(permission_option_value).collect::<Vec<_>>(),
        },
    })
}

fn routing_value(routing: &NativeModelRouting) -> Value {
    match routing {
        NativeModelRouting::Default => json!({"kind": "default"}),
        NativeModelRouting::Gateway { gateway_id } => {
            json!({"kind": "gateway", "gateway_id": gateway_id})
        }
        NativeModelRouting::ProviderRoute { provider_route_id } => {
            json!({"kind": "provider-route", "provider_route_id": provider_route_id})
        }
    }
}

fn selection_value(selection: &NativeModelSelection) -> Value {
    json!({
        "model_id": selection.model_id,
        "provider_route_id": selection.provider_route_id,
        "variant_id": selection.variant_id,
    })
}

fn context_config_value(config: &NativeContextConfig) -> Value {
    json!({"model_context_window": config.model_context_window})
}

fn context_option_value(option: &NativeContextWindowOption) -> Value {
    json!({
        "advisory": option.advisory,
        "description": option.description,
        "id": option.id,
        "label": option.label,
        "native_config": option.native_config.as_ref().map(context_config_value),
        "native_suffix": option.native_suffix,
        "tokens": option.tokens,
    })
}

fn context_capability_value(capability: &NativeContextWindowCapability) -> Value {
    json!({
        "availability": capability.availability,
        "default": capability.default,
        "options": capability.options.iter().map(context_option_value).collect::<Vec<_>>(),
    })
}

fn thinking_option_value(option: &NativeThinkingOption) -> Value {
    json!({
        "advisory": option.advisory,
        "description": option.description,
        "economics": option.economics,
        "id": option.id,
        "native_value": option.native_value,
        "presentation_group": option.presentation_group,
    })
}

fn thinking_value(thinking: &NativeThinkingCapability) -> Value {
    match thinking {
        NativeThinkingCapability::Unavailable => json!({"availability": "unavailable"}),
        NativeThinkingCapability::Native { description } => {
            json!({"availability": "native", "description": description})
        }
        NativeThinkingCapability::Supported { default, options } => json!({
            "availability": "supported",
            "default": default,
            "options": options.iter().map(thinking_option_value).collect::<Vec<_>>(),
        }),
    }
}

fn speed_option_value(option: &NativeSpeedOption) -> Value {
    json!({
        "availability": option.availability,
        "consumption_basis": option.consumption_basis,
        "consumption_multiplier": option.consumption_multiplier,
        "input_consumption_multiplier": option.input_consumption_multiplier,
        "output_consumption_multiplier": option.output_consumption_multiplier,
        "default": option.default,
        "description": option.description,
        "disabled": option.disabled,
        "id": option.id,
        "label": option.label,
        "native_value": option.native_value,
        "source_url": option.source_url,
        "speed_multiplier": option.speed_multiplier,
        "verified_at": option.verified_at,
    })
}

fn capabilities_value(capabilities: &NativeModelCapabilities) -> Value {
    json!({
        "context_window_tokens": capabilities.context_window_tokens,
        "context_window": capabilities.context_window.as_ref().map(context_capability_value),
        "image_input": capabilities.image_input,
        "local_tools": capabilities.local_tools,
        "mcp": capabilities.mcp,
        "output_tokens": capabilities.output_tokens,
        "reasoning_display": capabilities.reasoning_display,
        "speed_options": capabilities.speed_options.iter().map(speed_option_value).collect::<Vec<_>>(),
        "thinking": thinking_value(&capabilities.thinking),
        "web_search": capabilities.web_search,
    })
}

fn cost_value(cost: &NativeModelCost) -> Value {
    json!({
        "input_usd_per_million": cost.input_usd_per_million,
        "output_usd_per_million": cost.output_usd_per_million,
    })
}

fn model_value(model: &NativeModelDefinition) -> Value {
    json!({
        "id": model.id,
        "name": model.name,
        "native_model_id": model.native_model_id,
        "description": model.description,
        "harness": model.harness,
        "provider": model.provider,
        "routing": routing_value(&model.routing),
        "native_selection": model.native_selection.as_ref().map(selection_value),
        "status": model.status,
        "upstream_model_id": model.upstream_model_id,
        "metadata_confidence": model.metadata_confidence,
        "cost": model.cost.as_ref().map(cost_value),
        "disabled": model.disabled.as_ref().map(|disabled| json!({"reason": disabled.reason})),
        "capabilities": capabilities_value(&model.capabilities),
    })
}

fn route_group_value(group: &NativeModelRouteGroup) -> Value {
    json!({
        "id": group.id,
        "label": group.label,
        "order": group.order,
        "show_route_labels": group.show_route_labels,
    })
}

fn route_value(route: &NativeModelRoute) -> Value {
    json!({
        "engine_id": route.engine_id,
        "group": route_group_value(&route.group),
        "id": route.id,
        "label": route.label,
        "status": match &route.status {
            NativeModelRouteStatus::Available => "available",
            NativeModelRouteStatus::Unavailable => "unavailable",
        },
        "unavailable_reason": route.unavailable_reason,
    })
}

fn option_value(option: &NativeOptionValue) -> Value {
    json!({"id": option.id, "native_value": option.native_value})
}

fn context_selection_value(selection: &NativeContextSelection) -> Value {
    json!({
        "id": selection.id,
        "native_suffix": selection.native_suffix,
        "native_config": selection.native_config.as_ref().map(context_config_value),
    })
}

fn model_defaults_value(defaults: &NativeModelDefaults) -> Value {
    json!({
        "model_id": defaults.model_id,
        "reasoning_effort": defaults.reasoning_effort.as_ref().map(option_value),
        "speed": defaults.speed.as_ref().map(option_value),
        "context_window": defaults.context_window.as_ref().map(context_selection_value),
        "permission": defaults.permission.as_ref().map(option_value),
    })
}

fn scope_value(scope: &NativeCatalogScope) -> Value {
    json!({
        "profile_id": scope.profile_id,
        "working_directory": scope.working_directory,
        "workspace_trust": scope.workspace_trust,
    })
}

fn runtime_value(runtime: &NativeCatalogRuntime) -> Value {
    json!({
        "catalog_revision": runtime.catalog_revision,
        "runnable_harness_ids": runtime.runnable_harness_ids,
        "routes": runtime.routes.iter().map(route_value).collect::<Vec<_>>(),
        "default_model_id": runtime.default_model_id,
        "favorite_ids": runtime.favorite_ids,
        "model_defaults": runtime.model_defaults.iter().map(model_defaults_value).collect::<Vec<_>>(),
        "scope": runtime.scope.as_ref().map(scope_value),
    })
}

fn validate_wire_document(value: &Value) -> Result<(), NativeModelCatalogWireError> {
    let _ = decode_wire_value(value)?;
    Ok(())
}

fn decode_wire_value(value: &Value) -> Result<NativeModelCatalog, NativeModelCatalogWireError> {
    let object = checked_object(value)?;
    exact_keys(object, &["version", "manifest", "runtime", "provenance"])?;
    if read_u64(required(object, "version")?)? != WIRE_VERSION {
        return Err(NativeModelCatalogWireError::InvalidValue);
    }

    let manifest_value = required(object, "manifest")?;
    validate_manifest_value(manifest_value)?;
    let manifest = NativeModelManifest::from_value(manifest_value)
        .map_err(|_| NativeModelCatalogWireError::InvalidCatalog)?;
    validate_manifest_semantics(&manifest, None)?;

    let runtime = decode_runtime(required(object, "runtime")?, &manifest)?;
    let provenance = decode_provenance(required(object, "provenance")?, &manifest)?;
    let catalog = NativeModelCatalog::from_manifest(manifest, runtime);
    if catalog.provenance != provenance {
        return Err(NativeModelCatalogWireError::InvalidCatalog);
    }
    validate_catalog_runtime(&catalog)?;
    Ok(catalog)
}

fn decode_provenance(
    value: &Value,
    manifest: &NativeModelManifest,
) -> Result<NativeCatalogProvenance, NativeModelCatalogWireError> {
    let object = checked_object(value)?;
    exact_keys(object, &["source", "revision"])?;
    let source = read_text(required(object, "source")?)?;
    let revision = read_text(required(object, "revision")?)?;
    if source != crate::NATIVE_MODEL_CATALOG_SOURCE || revision != manifest.revision {
        return Err(NativeModelCatalogWireError::InvalidCatalog);
    }
    Ok(NativeCatalogProvenance {
        source: source.to_owned(),
        revision: revision.to_owned(),
    })
}

fn validate_catalog_counts(
    catalog: &NativeModelCatalog,
) -> Result<(), NativeModelCatalogWireError> {
    if catalog.manifest.providers.len() > MAX_WIRE_PROVIDERS
        || catalog.manifest.harnesses.len() > MAX_WIRE_HARNESSES
        || catalog.manifest.models.len() > MAX_WIRE_MODELS
        || catalog.routes.len() > MAX_WIRE_ROUTES
        || catalog.runnable_harness_ids.len() > MAX_WIRE_HARNESSES
        || catalog.favorite_ids.len() > MAX_FAVORITES
        || catalog.model_defaults.len() > MAX_WIRE_MODEL_DEFAULTS
    {
        return Err(NativeModelCatalogWireError::TooManyItems);
    }
    validate_favorite_snapshot_bytes(&catalog.favorite_ids)?;
    for harness in &catalog.manifest.harnesses {
        if harness.gateways.len() > MAX_WIRE_GATEWAYS
            || harness.permissions.options.len() > MAX_WIRE_OPTIONS
        {
            return Err(NativeModelCatalogWireError::TooManyItems);
        }
    }
    for model in &catalog.manifest.models {
        if model.capabilities.speed_options.len() > MAX_WIRE_OPTIONS {
            return Err(NativeModelCatalogWireError::TooManyItems);
        }
        if let Some(context) = &model.capabilities.context_window
            && context.options.len() > MAX_WIRE_OPTIONS
        {
            return Err(NativeModelCatalogWireError::TooManyItems);
        }
        if let NativeThinkingCapability::Supported { options, .. } = &model.capabilities.thinking
            && options.len() > MAX_WIRE_OPTIONS
        {
            return Err(NativeModelCatalogWireError::TooManyItems);
        }
    }
    Ok(())
}

fn validate_manifest_value(value: &Value) -> Result<(), NativeModelCatalogWireError> {
    let object = checked_object(value)?;
    exact_keys(object, &["revision", "providers", "harnesses", "models"])?;
    let _ = read_text(required(object, "revision")?)?;

    let providers = array(required(object, "providers")?)?;
    bounded_len(providers.len(), MAX_WIRE_PROVIDERS)?;
    for provider in providers {
        let provider = checked_object(provider)?;
        exact_keys(provider, &["id", "label"])?;
        let _ = read_identifier(required(provider, "id")?)?;
        let _ = read_text(required(provider, "label")?)?;
    }

    let harnesses = array(required(object, "harnesses")?)?;
    bounded_len(harnesses.len(), MAX_WIRE_HARNESSES)?;
    for harness in harnesses {
        validate_harness_value(harness)?;
    }

    let models = array(required(object, "models")?)?;
    bounded_len(models.len(), MAX_WIRE_MODELS)?;
    for model in models {
        validate_model_value(model)?;
    }
    Ok(())
}

fn validate_harness_value(value: &Value) -> Result<(), NativeModelCatalogWireError> {
    let object = checked_object(value)?;
    exact_keys(
        object,
        &[
            "compaction_default_model_id",
            "id",
            "gateways",
            "label",
            "permissions",
        ],
    )?;
    optional_identifier(object, "compaction_default_model_id")?;
    let _ = read_identifier(required(object, "id")?)?;
    let _ = read_text(required(object, "label")?)?;

    let gateways = array(required(object, "gateways")?)?;
    bounded_len(gateways.len(), MAX_WIRE_GATEWAYS)?;
    for gateway in gateways {
        let gateway = checked_object(gateway)?;
        exact_keys(gateway, &["id", "kind", "label"])?;
        let _ = read_identifier(required(gateway, "id")?)?;
        let _ = read_text(required(gateway, "kind")?)?;
        let _ = read_text(required(gateway, "label")?)?;
    }

    let permissions = checked_object(required(object, "permissions")?)?;
    exact_keys(permissions, &["default", "options"])?;
    let _ = read_identifier(required(permissions, "default")?)?;
    let options = array(required(permissions, "options")?)?;
    bounded_len(options.len(), MAX_WIRE_OPTIONS)?;
    for option in options {
        let option = checked_object(option)?;
        exact_keys(
            option,
            &[
                "approval_behavior",
                "availability",
                "description",
                "edit_scope",
                "id",
                "label",
                "native_value",
                "safety_boundary",
            ],
        )?;
        let _ = read_text(required(option, "approval_behavior")?)?;
        let _ = read_text(required(option, "availability")?)?;
        let _ = read_text(required(option, "description")?)?;
        let _ = read_text(required(option, "edit_scope")?)?;
        let _ = read_identifier(required(option, "id")?)?;
        let _ = read_text(required(option, "label")?)?;
        let _ = read_text(required(option, "native_value")?)?;
        let _ = read_text(required(option, "safety_boundary")?)?;
    }
    Ok(())
}

fn validate_model_value(value: &Value) -> Result<(), NativeModelCatalogWireError> {
    let object = checked_object(value)?;
    exact_keys(
        object,
        &[
            "id",
            "name",
            "native_model_id",
            "description",
            "harness",
            "provider",
            "routing",
            "native_selection",
            "status",
            "upstream_model_id",
            "metadata_confidence",
            "cost",
            "disabled",
            "capabilities",
        ],
    )?;
    let _ = read_identifier(required(object, "id")?)?;
    let _ = read_text(required(object, "name")?)?;
    let _ = read_identifier(required(object, "native_model_id")?)?;
    optional_text(object, "description")?;
    let _ = read_identifier(required(object, "harness")?)?;
    let _ = read_identifier(required(object, "provider")?)?;
    validate_routing_value(required(object, "routing")?)?;
    if let Some(selection) = optional_value(object, "native_selection") {
        validate_selection_value(selection)?;
    }
    let status = read_identifier(required(object, "status")?)?;
    if !matches!(status, "curated" | "dynamic" | "prototype") {
        return Err(NativeModelCatalogWireError::InvalidValue);
    }
    optional_identifier(object, "upstream_model_id")?;
    optional_text(object, "metadata_confidence")?;
    if let Some(cost) = optional_value(object, "cost") {
        validate_cost_value(cost)?;
    }
    if let Some(disabled) = optional_value(object, "disabled") {
        let disabled = checked_object(disabled)?;
        exact_keys(disabled, &["reason"])?;
        let _ = read_text(required(disabled, "reason")?)?;
    }
    validate_capabilities_value(required(object, "capabilities")?)
}

fn validate_routing_value(value: &Value) -> Result<(), NativeModelCatalogWireError> {
    let object = checked_object(value)?;
    let kind = read_identifier(required(object, "kind")?)?;
    match kind {
        "default" => exact_keys(object, &["kind"]),
        "gateway" => {
            exact_keys(object, &["kind", "gateway_id"])?;
            let _ = read_identifier(required(object, "gateway_id")?)?;
            Ok(())
        }
        "provider-route" => {
            exact_keys(object, &["kind", "provider_route_id"])?;
            let _ = read_identifier(required(object, "provider_route_id")?)?;
            Ok(())
        }
        _ => Err(NativeModelCatalogWireError::InvalidValue),
    }
}

fn validate_selection_value(value: &Value) -> Result<(), NativeModelCatalogWireError> {
    let object = checked_object(value)?;
    exact_keys(object, &["model_id", "provider_route_id", "variant_id"])?;
    let _ = read_identifier(required(object, "model_id")?)?;
    let _ = read_identifier(required(object, "provider_route_id")?)?;
    optional_identifier(object, "variant_id").map(|_| ())
}

fn validate_capabilities_value(value: &Value) -> Result<(), NativeModelCatalogWireError> {
    let object = checked_object(value)?;
    exact_keys(
        object,
        &[
            "context_window_tokens",
            "context_window",
            "image_input",
            "local_tools",
            "mcp",
            "output_tokens",
            "reasoning_display",
            "speed_options",
            "thinking",
            "web_search",
        ],
    )?;
    optional_u64(object, "context_window_tokens")?;
    if let Some(context) = optional_value(object, "context_window") {
        validate_context_capability_value(context)?;
    }
    let _ = read_bool(required(object, "image_input")?)?;
    let _ = read_bool(required(object, "local_tools")?)?;
    let _ = read_bool(required(object, "mcp")?)?;
    optional_u64(object, "output_tokens")?;
    optional_text(object, "reasoning_display")?;
    let speeds = array(required(object, "speed_options")?)?;
    bounded_len(speeds.len(), MAX_WIRE_OPTIONS)?;
    for speed in speeds {
        validate_speed_option_value(speed)?;
    }
    validate_thinking_value(required(object, "thinking")?)?;
    let _ = read_bool(required(object, "web_search")?)?;
    Ok(())
}

fn validate_context_capability_value(value: &Value) -> Result<(), NativeModelCatalogWireError> {
    let object = checked_object(value)?;
    exact_keys(object, &["availability", "default", "options"])?;
    let _ = read_text(required(object, "availability")?)?;
    let _ = read_identifier(required(object, "default")?)?;
    let options = array(required(object, "options")?)?;
    bounded_len(options.len(), MAX_WIRE_OPTIONS)?;
    for option in options {
        let option = checked_object(option)?;
        exact_keys(
            option,
            &[
                "advisory",
                "description",
                "id",
                "label",
                "native_config",
                "native_suffix",
                "tokens",
            ],
        )?;
        optional_text(option, "advisory")?;
        optional_text(option, "description")?;
        let _ = read_identifier(required(option, "id")?)?;
        let _ = read_text(required(option, "label")?)?;
        if let Some(config) = optional_value(option, "native_config") {
            let config = checked_object(config)?;
            exact_keys(config, &["model_context_window"])?;
            let _ = read_u64(required(config, "model_context_window")?)?;
        }
        let _ = read_text_allow_empty(required(option, "native_suffix")?)?;
        let _ = read_u64(required(option, "tokens")?)?;
    }
    Ok(())
}

fn validate_thinking_value(value: &Value) -> Result<(), NativeModelCatalogWireError> {
    let object = checked_object(value)?;
    let availability = read_identifier(required(object, "availability")?)?;
    match availability {
        "unavailable" => exact_keys(object, &["availability"]),
        "native" => {
            exact_keys(object, &["availability", "description"])?;
            let _ = read_text(required(object, "description")?)?;
            Ok(())
        }
        "supported" => {
            exact_keys(object, &["availability", "default", "options"])?;
            let _ = read_identifier(required(object, "default")?)?;
            let options = array(required(object, "options")?)?;
            bounded_len(options.len(), MAX_WIRE_OPTIONS)?;
            for option in options {
                let option = checked_object(option)?;
                exact_keys(
                    option,
                    &[
                        "advisory",
                        "description",
                        "economics",
                        "id",
                        "native_value",
                        "presentation_group",
                    ],
                )?;
                optional_text(option, "advisory")?;
                optional_text(option, "description")?;
                let _ = read_text(required(option, "economics")?)?;
                let _ = read_identifier(required(option, "id")?)?;
                let _ = read_text(required(option, "native_value")?)?;
                let _ = read_text(required(option, "presentation_group")?)?;
            }
            Ok(())
        }
        _ => Err(NativeModelCatalogWireError::InvalidValue),
    }
}

fn validate_speed_option_value(value: &Value) -> Result<(), NativeModelCatalogWireError> {
    let object = checked_object(value)?;
    exact_keys(
        object,
        &[
            "availability",
            "consumption_basis",
            "consumption_multiplier",
            "input_consumption_multiplier",
            "output_consumption_multiplier",
            "default",
            "description",
            "disabled",
            "id",
            "label",
            "native_value",
            "source_url",
            "speed_multiplier",
            "verified_at",
        ],
    )?;
    let _ = read_text(required(object, "availability")?)?;
    let _ = read_text(required(object, "consumption_basis")?)?;
    optional_f64(object, "consumption_multiplier")?;
    optional_f64(object, "input_consumption_multiplier")?;
    optional_f64(object, "output_consumption_multiplier")?;
    let _ = read_bool(required(object, "default")?)?;
    let _ = read_text(required(object, "description")?)?;
    optional_bool(object, "disabled")?;
    let _ = read_identifier(required(object, "id")?)?;
    let _ = read_text(required(object, "label")?)?;
    let _ = read_text(required(object, "native_value")?)?;
    optional_text(object, "source_url")?;
    optional_f64(object, "speed_multiplier")?;
    optional_text(object, "verified_at").map(|_| ())
}

fn validate_cost_value(value: &Value) -> Result<(), NativeModelCatalogWireError> {
    let object = checked_object(value)?;
    exact_keys(object, &["input_usd_per_million", "output_usd_per_million"])?;
    optional_nonnegative_f64(object, "input_usd_per_million")?;
    optional_nonnegative_f64(object, "output_usd_per_million")
}

fn decode_runtime(
    value: &Value,
    manifest: &NativeModelManifest,
) -> Result<NativeCatalogRuntime, NativeModelCatalogWireError> {
    let object = checked_object(value)?;
    exact_keys(
        object,
        &[
            "catalog_revision",
            "runnable_harness_ids",
            "routes",
            "default_model_id",
            "favorite_ids",
            "model_defaults",
            "scope",
        ],
    )?;

    let catalog_revision = Some(read_text(required(object, "catalog_revision")?)?.to_owned());

    let runnable_values = array(required(object, "runnable_harness_ids")?)?;
    bounded_len(runnable_values.len(), MAX_WIRE_HARNESSES)?;
    let mut runnable_harness_ids = Vec::with_capacity(runnable_values.len());
    let mut runnable_seen = HashSet::with_capacity(runnable_values.len());
    for value in runnable_values {
        let harness_id = read_identifier(value)?.to_owned();
        if !runnable_seen.insert(harness_id.clone()) {
            return Err(NativeModelCatalogWireError::DuplicateIdentifier);
        }
        if manifest.harness(&harness_id).is_none() {
            return Err(NativeModelCatalogWireError::UnknownReference);
        }
        runnable_harness_ids.push(harness_id);
    }

    let route_values = array(required(object, "routes")?)?;
    bounded_len(route_values.len(), MAX_WIRE_ROUTES)?;
    let mut routes = Vec::with_capacity(route_values.len());
    let mut route_seen = HashSet::with_capacity(route_values.len());
    for value in route_values {
        let route = decode_route(value, manifest)?;
        let identity = (route.engine_id.clone(), route.id.clone());
        if !route_seen.insert(identity) {
            return Err(NativeModelCatalogWireError::DuplicateIdentifier);
        }
        routes.push(route);
    }

    let default_model_id = optional_identifier(object, "default_model_id")?.map(str::to_owned);
    if let Some(model_id) = &default_model_id
        && manifest.model(model_id).is_none()
    {
        return Err(NativeModelCatalogWireError::UnknownReference);
    }

    let favorite_values = array(required(object, "favorite_ids")?)?;
    bounded_len(favorite_values.len(), MAX_FAVORITES)?;
    let mut favorite_ids = Vec::with_capacity(favorite_values.len());
    let mut favorite_seen = HashSet::with_capacity(favorite_values.len());
    for value in favorite_values {
        let favorite_id = read_identifier(value)?.to_owned();
        if !favorite_seen.insert(favorite_id.clone()) {
            return Err(NativeModelCatalogWireError::DuplicateIdentifier);
        }
        favorite_ids.push(favorite_id);
    }

    let defaults_values = array(required(object, "model_defaults")?)?;
    bounded_len(defaults_values.len(), MAX_WIRE_MODEL_DEFAULTS)?;
    let mut model_defaults = Vec::with_capacity(defaults_values.len());
    let mut defaults_seen = HashSet::with_capacity(defaults_values.len());
    for value in defaults_values {
        let defaults = decode_model_defaults(value, manifest)?;
        if !defaults_seen.insert(defaults.model_id.clone()) {
            return Err(NativeModelCatalogWireError::DuplicateIdentifier);
        }
        model_defaults.push(defaults);
    }

    let scope = optional_value(object, "scope")
        .map(decode_scope)
        .transpose()?;

    Ok(NativeCatalogRuntime {
        catalog_revision,
        runnable_harness_ids,
        routes,
        default_model_id,
        favorite_ids,
        model_defaults,
        scope,
    })
}

fn decode_route(
    value: &Value,
    manifest: &NativeModelManifest,
) -> Result<NativeModelRoute, NativeModelCatalogWireError> {
    let object = checked_object(value)?;
    exact_keys(
        object,
        &[
            "engine_id",
            "group",
            "id",
            "label",
            "status",
            "unavailable_reason",
        ],
    )?;
    let engine_id = read_identifier(required(object, "engine_id")?)?.to_owned();
    if manifest.harness(&engine_id).is_none() {
        return Err(NativeModelCatalogWireError::UnknownReference);
    }
    let group = decode_route_group(required(object, "group")?)?;
    let id = read_identifier(required(object, "id")?)?.to_owned();
    let label = read_text(required(object, "label")?)?.to_owned();
    let status = read_identifier(required(object, "status")?)?;
    let unavailable_reason = optional_text(object, "unavailable_reason")?.map(str::to_owned);
    let status = match status {
        "available" => {
            if unavailable_reason.is_some() {
                return Err(NativeModelCatalogWireError::InvalidValue);
            }
            NativeModelRouteStatus::Available
        }
        "unavailable" => {
            if unavailable_reason.is_none() {
                return Err(NativeModelCatalogWireError::InvalidValue);
            }
            NativeModelRouteStatus::Unavailable
        }
        _ => return Err(NativeModelCatalogWireError::InvalidValue),
    };
    Ok(NativeModelRoute {
        engine_id,
        group,
        id,
        label,
        status,
        unavailable_reason,
    })
}

fn decode_route_group(value: &Value) -> Result<NativeModelRouteGroup, NativeModelCatalogWireError> {
    let object = checked_object(value)?;
    exact_keys(object, &["id", "label", "order", "show_route_labels"])?;
    Ok(NativeModelRouteGroup {
        id: read_identifier(required(object, "id")?)?.to_owned(),
        label: read_text(required(object, "label")?)?.to_owned(),
        order: read_u32(required(object, "order")?)?,
        show_route_labels: read_bool(required(object, "show_route_labels")?)?,
    })
}

fn decode_model_defaults(
    value: &Value,
    manifest: &NativeModelManifest,
) -> Result<NativeModelDefaults, NativeModelCatalogWireError> {
    let object = checked_object(value)?;
    exact_keys(
        object,
        &[
            "model_id",
            "reasoning_effort",
            "speed",
            "context_window",
            "permission",
        ],
    )?;
    let model_id = read_identifier(required(object, "model_id")?)?.to_owned();
    let model = manifest
        .model(&model_id)
        .ok_or(NativeModelCatalogWireError::UnknownReference)?;
    let harness = manifest
        .harness(&model.harness)
        .ok_or(NativeModelCatalogWireError::UnknownReference)?;
    let reasoning_effort = optional_value(object, "reasoning_effort")
        .map(decode_option_value)
        .transpose()?;
    let speed = optional_value(object, "speed")
        .map(decode_option_value)
        .transpose()?;
    let context_window = optional_value(object, "context_window")
        .map(decode_context_selection)
        .transpose()?;
    let permission = optional_value(object, "permission")
        .map(decode_option_value)
        .transpose()?;
    let defaults = NativeModelDefaults {
        model_id,
        reasoning_effort,
        speed,
        context_window,
        permission,
    };
    validate_model_defaults(&defaults, model, harness)?;
    Ok(defaults)
}

fn decode_option_value(value: &Value) -> Result<NativeOptionValue, NativeModelCatalogWireError> {
    let object = checked_object(value)?;
    exact_keys(object, &["id", "native_value"])?;
    Ok(NativeOptionValue {
        id: read_identifier(required(object, "id")?)?.to_owned(),
        native_value: read_text(required(object, "native_value")?)?.to_owned(),
    })
}

fn decode_context_selection(
    value: &Value,
) -> Result<NativeContextSelection, NativeModelCatalogWireError> {
    let object = checked_object(value)?;
    exact_keys(object, &["id", "native_suffix", "native_config"])?;
    let native_config = optional_value(object, "native_config")
        .map(decode_context_config)
        .transpose()?;
    Ok(NativeContextSelection {
        id: read_identifier(required(object, "id")?)?.to_owned(),
        native_suffix: read_text_allow_empty(required(object, "native_suffix")?)?.to_owned(),
        native_config,
    })
}

fn decode_context_config(
    value: &Value,
) -> Result<NativeContextConfig, NativeModelCatalogWireError> {
    let object = checked_object(value)?;
    exact_keys(object, &["model_context_window"])?;
    Ok(NativeContextConfig {
        model_context_window: read_u64(required(object, "model_context_window")?)?,
    })
}

fn decode_scope(value: &Value) -> Result<NativeCatalogScope, NativeModelCatalogWireError> {
    let object = checked_object(value)?;
    exact_keys(
        object,
        &["profile_id", "working_directory", "workspace_trust"],
    )?;
    let profile_id =
        read_bounded_identifier(required(object, "profile_id")?, MAX_SCOPE_PROFILE_BYTES)?
            .to_owned();
    let working_directory = read_bounded_text(
        required(object, "working_directory")?,
        MAX_SCOPE_WORKING_DIRECTORY_BYTES,
        false,
    )?
    .to_owned();
    let workspace_trust = read_identifier(required(object, "workspace_trust")?)?;
    if !matches!(workspace_trust, "safe" | "trusted_project_config") {
        return Err(NativeModelCatalogWireError::InvalidScope);
    }
    Ok(NativeCatalogScope {
        profile_id,
        working_directory,
        workspace_trust: workspace_trust.to_owned(),
    })
}

fn validate_catalog_runtime(
    catalog: &NativeModelCatalog,
) -> Result<(), NativeModelCatalogWireError> {
    validate_catalog_counts(catalog)?;
    if catalog.provenance.source != crate::NATIVE_MODEL_CATALOG_SOURCE
        || catalog.provenance.revision != catalog.manifest.revision
        || catalog.catalog_revision.is_empty()
    {
        return Err(NativeModelCatalogWireError::InvalidCatalog);
    }
    validate_text_str(&catalog.catalog_revision)?;

    let harness_ids = catalog
        .manifest
        .harnesses
        .iter()
        .map(|harness| harness.id.as_str())
        .collect::<HashSet<_>>();
    for harness_id in &catalog.runnable_harness_ids {
        validate_identifier_str(harness_id)?;
        if !harness_ids.contains(harness_id.as_str()) {
            return Err(NativeModelCatalogWireError::UnknownReference);
        }
    }

    let mut route_ids = HashSet::with_capacity(catalog.routes.len());
    for route in &catalog.routes {
        validate_identifier_str(&route.engine_id)?;
        validate_native_identifier_str(&route.id)?;
        validate_text_str(&route.label)?;
        validate_route_group(&route.group)?;
        if !harness_ids.contains(route.engine_id.as_str()) {
            return Err(NativeModelCatalogWireError::UnknownReference);
        }
        if !route_ids.insert((route.engine_id.as_str(), route.id.as_str())) {
            return Err(NativeModelCatalogWireError::DuplicateIdentifier);
        }
        match (&route.status, &route.unavailable_reason) {
            (NativeModelRouteStatus::Available, Some(_))
            | (NativeModelRouteStatus::Unavailable, None) => {
                return Err(NativeModelCatalogWireError::InvalidValue);
            }
            (NativeModelRouteStatus::Available, None) => {}
            (NativeModelRouteStatus::Unavailable, Some(reason)) => {
                validate_text_str(reason)?;
            }
        }
    }

    if let Some(default_model_id) = &catalog.default_model_id {
        validate_identifier_str(default_model_id)?;
        if catalog.manifest.model(default_model_id).is_none() {
            return Err(NativeModelCatalogWireError::UnknownReference);
        }
    }

    let mut favorite_ids = HashSet::with_capacity(catalog.favorite_ids.len());
    for favorite_id in &catalog.favorite_ids {
        validate_identifier_str(favorite_id)?;
        if !favorite_ids.insert(favorite_id.as_str()) {
            return Err(NativeModelCatalogWireError::DuplicateIdentifier);
        }
    }

    let mut default_ids = HashSet::with_capacity(catalog.model_defaults.len());
    for defaults in &catalog.model_defaults {
        validate_identifier_str(&defaults.model_id)?;
        if !default_ids.insert(defaults.model_id.as_str()) {
            return Err(NativeModelCatalogWireError::DuplicateIdentifier);
        }
        let model = catalog
            .manifest
            .model(&defaults.model_id)
            .ok_or(NativeModelCatalogWireError::UnknownReference)?;
        let harness = catalog
            .manifest
            .harness(&model.harness)
            .ok_or(NativeModelCatalogWireError::UnknownReference)?;
        validate_model_defaults(defaults, model, harness)?;
    }

    if let Some(scope) = &catalog.scope {
        validate_scope(scope)?;
    }
    validate_manifest_semantics(&catalog.manifest, Some(&catalog.routes))
}

fn validate_manifest_semantics(
    manifest: &NativeModelManifest,
    runtime_routes: Option<&[NativeModelRoute]>,
) -> Result<(), NativeModelCatalogWireError> {
    validate_bundled_manifest_prefix(manifest)?;
    let mut provider_ids = HashSet::with_capacity(manifest.providers.len());
    for provider in &manifest.providers {
        validate_identifier_str(&provider.id)?;
        validate_text_str(&provider.label)?;
        if !provider_ids.insert(provider.id.as_str()) {
            return Err(NativeModelCatalogWireError::DuplicateIdentifier);
        }
    }

    let mut harness_ids = HashSet::with_capacity(manifest.harnesses.len());
    for harness in &manifest.harnesses {
        validate_identifier_str(&harness.id)?;
        validate_text_str(&harness.label)?;
        optional_identifier_str(harness.compaction_default_model_id.as_deref())?;
        if !harness_ids.insert(harness.id.as_str()) {
            return Err(NativeModelCatalogWireError::DuplicateIdentifier);
        }

        let mut gateway_ids = HashSet::with_capacity(harness.gateways.len());
        for gateway in &harness.gateways {
            validate_identifier_str(&gateway.id)?;
            validate_text_str(&gateway.kind)?;
            validate_text_str(&gateway.label)?;
            if !gateway_ids.insert(gateway.id.as_str()) {
                return Err(NativeModelCatalogWireError::DuplicateIdentifier);
            }
        }

        let mut permission_ids = HashSet::with_capacity(harness.permissions.options.len());
        validate_identifier_str(&harness.permissions.default)?;
        for option in &harness.permissions.options {
            validate_text_str(&option.approval_behavior)?;
            validate_text_str(&option.availability)?;
            validate_text_str(&option.description)?;
            validate_text_str(&option.edit_scope)?;
            validate_identifier_str(&option.id)?;
            validate_text_str(&option.label)?;
            validate_text_str(&option.native_value)?;
            validate_text_str(&option.safety_boundary)?;
            if !permission_ids.insert(option.id.as_str()) {
                return Err(NativeModelCatalogWireError::DuplicateIdentifier);
            }
        }
        if !permission_ids.contains(harness.permissions.default.as_str()) {
            return Err(NativeModelCatalogWireError::UnknownReference);
        }
    }

    let mut model_ids = HashSet::with_capacity(manifest.models.len());
    for model in &manifest.models {
        validate_identifier_str(&model.id)?;
        validate_text_str(&model.name)?;
        validate_native_identifier_str(&model.native_model_id)?;
        optional_text_str(model.description.as_deref())?;
        validate_identifier_str(&model.harness)?;
        validate_identifier_str(&model.provider)?;
        validate_identifier_str(&model.status)?;
        if !matches!(model.status.as_str(), "curated" | "dynamic" | "prototype") {
            return Err(NativeModelCatalogWireError::InvalidValue);
        }
        optional_identifier_str(model.upstream_model_id.as_deref())?;
        optional_text_str(model.metadata_confidence.as_deref())?;
        if !model_ids.insert(model.id.as_str()) {
            return Err(NativeModelCatalogWireError::DuplicateIdentifier);
        }
        if !provider_ids.contains(model.provider.as_str())
            || !harness_ids.contains(model.harness.as_str())
        {
            return Err(NativeModelCatalogWireError::UnknownReference);
        }

        let harness = manifest
            .harness(&model.harness)
            .ok_or(NativeModelCatalogWireError::UnknownReference)?;
        match &model.routing {
            NativeModelRouting::Default => {}
            NativeModelRouting::Gateway { gateway_id } => {
                validate_native_identifier_str(gateway_id)?;
                if !harness
                    .gateways
                    .iter()
                    .any(|gateway| gateway.id == *gateway_id)
                {
                    return Err(NativeModelCatalogWireError::UnknownReference);
                }
            }
            NativeModelRouting::ProviderRoute { provider_route_id } => {
                validate_native_identifier_str(provider_route_id)?;
            }
        }

        validate_model_capabilities(&model.capabilities)?;
        if let Some(selection) = &model.native_selection {
            validate_native_identifier_str(&selection.model_id)?;
            validate_native_identifier_str(&selection.provider_route_id)?;
            if let Some(variant_id) = selection.variant_id.as_deref() {
                validate_native_identifier_str(variant_id)?;
            }
            if selection.model_id != model.native_model_id {
                return Err(NativeModelCatalogWireError::InvalidNativeIdentity);
            }
            let NativeModelRouting::ProviderRoute { provider_route_id } = &model.routing else {
                return Err(NativeModelCatalogWireError::InvalidNativeIdentity);
            };
            if provider_route_id != &selection.provider_route_id {
                return Err(NativeModelCatalogWireError::InvalidNativeIdentity);
            }
            if model.harness == "opencode2" {
                let expected = opencode2_catalog_id(
                    &selection.model_id,
                    &selection.provider_route_id,
                    selection.variant_id.as_deref(),
                )?;
                if model.id != expected {
                    return Err(NativeModelCatalogWireError::InvalidNativeIdentity);
                }
                if let Some(runtime_routes) = runtime_routes
                    && !runtime_routes.iter().any(|route| {
                        route.engine_id == model.harness && route.id == selection.provider_route_id
                    })
                {
                    return Err(NativeModelCatalogWireError::UnknownReference);
                }
            }
        }
    }
    Ok(())
}

fn validate_bundled_manifest_prefix(
    manifest: &NativeModelManifest,
) -> Result<(), NativeModelCatalogWireError> {
    if manifest.revision != crate::NATIVE_MODEL_CATALOG_REVISION {
        return Err(NativeModelCatalogWireError::InvalidCatalog);
    }
    let bundled = NativeModelCatalog::offline()
        .map_err(|_| NativeModelCatalogWireError::InvalidCatalog)?
        .manifest;
    // The bundled baseline must survive a runtime overlay: harness policy is
    // immutable, every bundled provider identity is retained, and every
    // bundled model keeps its id, harness, and native identity. Reported
    // fields (name, description, capabilities) and the disabled flag may be
    // overlaid, and runtime rows may be appended after the baseline.
    if manifest.harnesses != bundled.harnesses
        || manifest.providers.len() < bundled.providers.len()
        || manifest.models.len() < bundled.models.len()
    {
        return Err(NativeModelCatalogWireError::InvalidCatalog);
    }
    for provider in &bundled.providers {
        if !manifest
            .providers
            .iter()
            .any(|candidate| candidate.id == provider.id && candidate.label == provider.label)
        {
            return Err(NativeModelCatalogWireError::InvalidCatalog);
        }
    }
    for model in &bundled.models {
        let Some(current) = manifest.model(&model.id) else {
            return Err(NativeModelCatalogWireError::InvalidCatalog);
        };
        if current.harness != model.harness || current.native_model_id != model.native_model_id {
            return Err(NativeModelCatalogWireError::InvalidCatalog);
        }
    }
    Ok(())
}

fn validate_model_capabilities(
    capabilities: &NativeModelCapabilities,
) -> Result<(), NativeModelCatalogWireError> {
    optional_nonnegative_u64(capabilities.context_window_tokens)?;
    optional_nonnegative_u64(capabilities.output_tokens)?;
    optional_text_str(capabilities.reasoning_display.as_deref())?;
    if let Some(context) = &capabilities.context_window {
        let mut ids = HashSet::with_capacity(context.options.len());
        validate_text_str(&context.availability)?;
        validate_identifier_str(&context.default)?;
        for option in &context.options {
            optional_text_str(option.advisory.as_deref())?;
            optional_text_str(option.description.as_deref())?;
            validate_identifier_str(&option.id)?;
            validate_text_str(&option.label)?;
            read_text_string(&option.native_suffix, true)?;
            if let Some(config) = &option.native_config {
                optional_nonnegative_u64(Some(config.model_context_window))?;
            }
            optional_nonnegative_u64(Some(option.tokens))?;
            if !ids.insert(option.id.as_str()) {
                return Err(NativeModelCatalogWireError::DuplicateIdentifier);
            }
        }
        if !ids.contains(context.default.as_str()) {
            return Err(NativeModelCatalogWireError::UnknownReference);
        }
    }

    let mut speed_ids = HashSet::with_capacity(capabilities.speed_options.len());
    for option in &capabilities.speed_options {
        validate_text_str(&option.availability)?;
        validate_text_str(&option.consumption_basis)?;
        optional_finite_f64(option.consumption_multiplier)?;
        optional_finite_f64(option.input_consumption_multiplier)?;
        optional_finite_f64(option.output_consumption_multiplier)?;
        validate_text_str(&option.description)?;
        validate_identifier_str(&option.id)?;
        validate_text_str(&option.label)?;
        validate_text_str(&option.native_value)?;
        optional_text_str(option.source_url.as_deref())?;
        optional_finite_f64(option.speed_multiplier)?;
        optional_text_str(option.verified_at.as_deref())?;
        if !speed_ids.insert(option.id.as_str()) {
            return Err(NativeModelCatalogWireError::DuplicateIdentifier);
        }
    }

    match &capabilities.thinking {
        NativeThinkingCapability::Unavailable => {}
        NativeThinkingCapability::Native { description } => validate_text_str(description)?,
        NativeThinkingCapability::Supported { default, options } => {
            validate_identifier_str(default)?;
            let mut ids = HashSet::with_capacity(options.len());
            for option in options {
                optional_text_str(option.advisory.as_deref())?;
                optional_text_str(option.description.as_deref())?;
                validate_text_str(&option.economics)?;
                validate_identifier_str(&option.id)?;
                validate_text_str(&option.native_value)?;
                validate_text_str(&option.presentation_group)?;
                if !ids.insert(option.id.as_str()) {
                    return Err(NativeModelCatalogWireError::DuplicateIdentifier);
                }
            }
            if !ids.contains(default.as_str()) {
                return Err(NativeModelCatalogWireError::UnknownReference);
            }
        }
    }
    Ok(())
}

fn validate_model_defaults(
    defaults: &NativeModelDefaults,
    model: &NativeModelDefinition,
    harness: &NativeHarness,
) -> Result<(), NativeModelCatalogWireError> {
    if let Some(value) = &defaults.reasoning_effort {
        let NativeThinkingCapability::Supported { options, .. } = &model.capabilities.thinking
        else {
            return Err(NativeModelCatalogWireError::UnknownReference);
        };
        if !options
            .iter()
            .any(|option| option.id == value.id && option.native_value == value.native_value)
        {
            return Err(NativeModelCatalogWireError::UnknownReference);
        }
    }
    if let Some(value) = &defaults.speed
        && !model
            .capabilities
            .speed_options
            .iter()
            .any(|option| option.id == value.id && option.native_value == value.native_value)
    {
        return Err(NativeModelCatalogWireError::UnknownReference);
    }
    if let Some(value) = &defaults.context_window {
        let Some(capability) = &model.capabilities.context_window else {
            return Err(NativeModelCatalogWireError::UnknownReference);
        };
        if !capability.options.iter().any(|option| {
            option.id == value.id
                && option.native_suffix == value.native_suffix
                && option.native_config == value.native_config
        }) {
            return Err(NativeModelCatalogWireError::UnknownReference);
        }
    }
    if let Some(value) = &defaults.permission
        && !harness
            .permissions
            .options
            .iter()
            .any(|option| option.id == value.id && option.native_value == value.native_value)
    {
        return Err(NativeModelCatalogWireError::UnknownReference);
    }
    Ok(())
}

fn validate_route_group(group: &NativeModelRouteGroup) -> Result<(), NativeModelCatalogWireError> {
    validate_identifier_str(&group.id)?;
    validate_text_str(&group.label)
}

fn validate_scope(scope: &NativeCatalogScope) -> Result<(), NativeModelCatalogWireError> {
    validate_bounded_identifier_str(&scope.profile_id, MAX_SCOPE_PROFILE_BYTES)?;
    validate_bounded_text_str(
        &scope.working_directory,
        MAX_SCOPE_WORKING_DIRECTORY_BYTES,
        false,
    )?;
    if !matches!(
        scope.workspace_trust.as_str(),
        "safe" | "trusted_project_config"
    ) {
        return Err(NativeModelCatalogWireError::InvalidScope);
    }
    Ok(())
}

fn checked_object(value: &Value) -> Result<&Map<String, Value>, NativeModelCatalogWireError> {
    value
        .as_object()
        .ok_or(NativeModelCatalogWireError::InvalidShape)
}

fn exact_keys(
    object: &Map<String, Value>,
    expected: &[&str],
) -> Result<(), NativeModelCatalogWireError> {
    if object.keys().any(|key| !expected.contains(&key.as_str())) {
        return Err(NativeModelCatalogWireError::UnknownField);
    }
    if object.len() != expected.len() || expected.iter().any(|key| !object.contains_key(*key)) {
        return Err(NativeModelCatalogWireError::InvalidShape);
    }
    Ok(())
}

fn required<'a>(
    object: &'a Map<String, Value>,
    key: &str,
) -> Result<&'a Value, NativeModelCatalogWireError> {
    object
        .get(key)
        .ok_or(NativeModelCatalogWireError::InvalidShape)
}

fn optional_value<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    object.get(key).filter(|value| !value.is_null())
}

fn array(value: &Value) -> Result<&Vec<Value>, NativeModelCatalogWireError> {
    value
        .as_array()
        .ok_or(NativeModelCatalogWireError::InvalidShape)
}

fn bounded_len(length: usize, maximum: usize) -> Result<(), NativeModelCatalogWireError> {
    if length > maximum {
        Err(NativeModelCatalogWireError::TooManyItems)
    } else {
        Ok(())
    }
}

fn read_identifier(value: &Value) -> Result<&str, NativeModelCatalogWireError> {
    read_bounded_identifier(value, MAX_WIRE_IDENTIFIER_BYTES)
}

fn read_bounded_identifier(
    value: &Value,
    maximum: usize,
) -> Result<&str, NativeModelCatalogWireError> {
    let string = value
        .as_str()
        .ok_or(NativeModelCatalogWireError::InvalidShape)?;
    validate_bounded_identifier_str(string, maximum)?;
    Ok(string)
}

fn read_text(value: &Value) -> Result<&str, NativeModelCatalogWireError> {
    read_bounded_text(value, MAX_WIRE_TEXT_BYTES, false)
}

fn read_bounded_text(
    value: &Value,
    maximum: usize,
    allow_empty: bool,
) -> Result<&str, NativeModelCatalogWireError> {
    let string = value
        .as_str()
        .ok_or(NativeModelCatalogWireError::InvalidShape)?;
    validate_bounded_text_str(string, maximum, allow_empty)?;
    Ok(string)
}

fn read_text_allow_empty(value: &Value) -> Result<&str, NativeModelCatalogWireError> {
    read_bounded_text(value, MAX_WIRE_TEXT_BYTES, true)
}

fn optional_identifier<'a>(
    object: &'a Map<String, Value>,
    key: &str,
) -> Result<Option<&'a str>, NativeModelCatalogWireError> {
    optional_value(object, key).map(read_identifier).transpose()
}

fn optional_text<'a>(
    object: &'a Map<String, Value>,
    key: &str,
) -> Result<Option<&'a str>, NativeModelCatalogWireError> {
    optional_value(object, key).map(read_text).transpose()
}

fn read_bool(value: &Value) -> Result<bool, NativeModelCatalogWireError> {
    value
        .as_bool()
        .ok_or(NativeModelCatalogWireError::InvalidShape)
}

fn optional_bool(
    object: &Map<String, Value>,
    key: &str,
) -> Result<Option<bool>, NativeModelCatalogWireError> {
    optional_value(object, key).map(read_bool).transpose()
}

fn read_u64(value: &Value) -> Result<u64, NativeModelCatalogWireError> {
    if let Some(number) = value.as_u64() {
        return Ok(number);
    }
    if value.is_number() {
        Err(NativeModelCatalogWireError::InvalidNumber)
    } else {
        Err(NativeModelCatalogWireError::InvalidShape)
    }
}

fn read_u32(value: &Value) -> Result<u32, NativeModelCatalogWireError> {
    let number = read_u64(value)?;
    u32::try_from(number).map_err(|_| NativeModelCatalogWireError::InvalidNumber)
}

fn optional_u64(
    object: &Map<String, Value>,
    key: &str,
) -> Result<Option<u64>, NativeModelCatalogWireError> {
    optional_value(object, key).map(read_u64).transpose()
}

fn optional_f64(
    object: &Map<String, Value>,
    key: &str,
) -> Result<Option<f64>, NativeModelCatalogWireError> {
    let Some(value) = optional_value(object, key) else {
        return Ok(None);
    };
    let number = value
        .as_f64()
        .ok_or(NativeModelCatalogWireError::InvalidNumber)?;
    if !number.is_finite() {
        return Err(NativeModelCatalogWireError::InvalidNumber);
    }
    Ok(Some(number))
}

fn validate_identifier_str(value: &str) -> Result<(), NativeModelCatalogWireError> {
    validate_bounded_identifier_str(value, MAX_WIRE_IDENTIFIER_BYTES)
}

fn validate_native_identifier_str(value: &str) -> Result<(), NativeModelCatalogWireError> {
    validate_bounded_identifier_str(value, MAX_NATIVE_IDENTIFIER_BYTES)
}

fn validate_bounded_identifier_str(
    value: &str,
    maximum: usize,
) -> Result<(), NativeModelCatalogWireError> {
    if value.is_empty()
        || value.len() > maximum
        || value
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err(NativeModelCatalogWireError::InvalidIdentifier);
    }
    Ok(())
}

fn validate_text_str(value: &str) -> Result<(), NativeModelCatalogWireError> {
    validate_bounded_text_str(value, MAX_WIRE_TEXT_BYTES, false)
}

fn validate_bounded_text_str(
    value: &str,
    maximum: usize,
    allow_empty: bool,
) -> Result<(), NativeModelCatalogWireError> {
    if (!allow_empty && value.is_empty())
        || value.len() > maximum
        || value.chars().any(char::is_control)
    {
        return Err(NativeModelCatalogWireError::InvalidValue);
    }
    Ok(())
}

fn read_text_string(value: &str, allow_empty: bool) -> Result<&str, NativeModelCatalogWireError> {
    validate_bounded_text_str(value, MAX_WIRE_TEXT_BYTES, allow_empty)?;
    Ok(value)
}

fn optional_identifier_str(value: Option<&str>) -> Result<(), NativeModelCatalogWireError> {
    if let Some(value) = value {
        validate_identifier_str(value)?;
    }
    Ok(())
}

fn optional_text_str(value: Option<&str>) -> Result<(), NativeModelCatalogWireError> {
    if let Some(value) = value {
        validate_text_str(value)?;
    }
    Ok(())
}

fn optional_nonnegative_u64(value: Option<u64>) -> Result<(), NativeModelCatalogWireError> {
    let _ = value;
    Ok(())
}

fn optional_finite_f64(value: Option<f64>) -> Result<(), NativeModelCatalogWireError> {
    if let Some(value) = value {
        if !value.is_finite() || value < 0.0 {
            return Err(NativeModelCatalogWireError::InvalidNumber);
        }
    }
    Ok(())
}

fn optional_nonnegative_f64(
    object: &Map<String, Value>,
    key: &str,
) -> Result<(), NativeModelCatalogWireError> {
    if let Some(value) = optional_f64(object, key)?
        && value < 0.0
    {
        return Err(NativeModelCatalogWireError::InvalidNumber);
    }
    Ok(())
}

fn validate_favorite_snapshot_bytes(
    favorite_ids: &[String],
) -> Result<(), NativeModelCatalogWireError> {
    let mut bytes = 2usize; // JSON array brackets.
    for (index, favorite_id) in favorite_ids.iter().enumerate() {
        validate_identifier_str(favorite_id)?;
        if index > 0 {
            bytes = bytes
                .checked_add(1)
                .ok_or(NativeModelCatalogWireError::PayloadTooLarge)?;
        }
        bytes = bytes
            .checked_add(2)
            .ok_or(NativeModelCatalogWireError::PayloadTooLarge)?;
        for byte in favorite_id.bytes() {
            let escaped_bytes = match byte {
                b'"' | b'\\' => 2,
                0x00..=0x1f => 6,
                _ => 1,
            };
            bytes = bytes
                .checked_add(escaped_bytes)
                .ok_or(NativeModelCatalogWireError::PayloadTooLarge)?;
        }
    }
    if bytes > MAX_FAVORITE_SNAPSHOT_BYTES {
        return Err(NativeModelCatalogWireError::PayloadTooLarge);
    }
    Ok(())
}

fn base64_url_no_pad(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let output_len = bytes.len().div_ceil(3) * 4
        - match bytes.len() % 3 {
            0 => 0,
            1 => 2,
            _ => 1,
        };
    let mut output = String::with_capacity(output_len);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        output.push(ALPHABET[(first >> 2) as usize] as char);
        if chunk.len() == 1 {
            output.push(ALPHABET[((first & 0x03) << 4) as usize] as char);
            continue;
        }
        let second = chunk[1];
        output.push(ALPHABET[(((first & 0x03) << 4) | (second >> 4)) as usize] as char);
        if chunk.len() == 2 {
            output.push(ALPHABET[((second & 0x0f) << 2) as usize] as char);
            continue;
        }
        let third = chunk[2];
        output.push(ALPHABET[(((second & 0x0f) << 2) | (third >> 6)) as usize] as char);
        output.push(ALPHABET[(third & 0x3f) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dynamic_catalog() -> NativeModelCatalog {
        let mut catalog = NativeModelCatalog::offline().expect("bundled catalog is valid");
        let model_id = opencode2_catalog_id("runtime-model", "opencode-go", Some("balanced"))
            .expect("fixture identity is valid");
        catalog.manifest.providers.push(NativeModelProvider {
            id: "opencode-go".to_owned(),
            label: "Go".to_owned(),
        });
        catalog.manifest.models.push(NativeModelDefinition {
            id: model_id.clone(),
            name: "Runtime model".to_owned(),
            native_model_id: "runtime-model".to_owned(),
            description: None,
            harness: "opencode2".to_owned(),
            provider: "opencode-go".to_owned(),
            routing: NativeModelRouting::ProviderRoute {
                provider_route_id: "opencode-go".to_owned(),
            },
            native_selection: Some(NativeModelSelection {
                model_id: "runtime-model".to_owned(),
                provider_route_id: "opencode-go".to_owned(),
                variant_id: Some("balanced".to_owned()),
            }),
            status: "dynamic".to_owned(),
            upstream_model_id: Some("upstream-runtime-model".to_owned()),
            metadata_confidence: Some("reported".to_owned()),
            cost: None,
            disabled: None,
            capabilities: NativeModelCapabilities {
                context_window_tokens: Some(64_000),
                context_window: None,
                image_input: false,
                local_tools: true,
                mcp: false,
                output_tokens: Some(4_096),
                reasoning_display: None,
                speed_options: Vec::new(),
                thinking: NativeThinkingCapability::Unavailable,
                web_search: false,
            },
        });
        catalog.catalog_revision = "opencode2-runtime-revision".to_owned();
        catalog.runnable_harness_ids = vec!["opencode2".to_owned()];
        catalog.routes = vec![NativeModelRoute {
            engine_id: "opencode2".to_owned(),
            group: NativeModelRouteGroup {
                id: "go".to_owned(),
                label: "Go".to_owned(),
                order: 0,
                show_route_labels: false,
            },
            id: "opencode-go".to_owned(),
            label: "Go".to_owned(),
            status: NativeModelRouteStatus::Available,
            unavailable_reason: None,
        }];
        catalog.default_model_id = Some(model_id.clone());
        catalog.favorite_ids = vec![model_id];
        catalog.model_defaults = vec![NativeModelDefaults {
            model_id: "codex-sol".to_owned(),
            reasoning_effort: Some(NativeOptionValue {
                id: "high".to_owned(),
                native_value: "high".to_owned(),
            }),
            speed: Some(NativeOptionValue {
                id: "standard".to_owned(),
                native_value: "standard".to_owned(),
            }),
            context_window: Some(NativeContextSelection {
                id: "standard".to_owned(),
                native_suffix: String::new(),
                native_config: None,
            }),
            permission: Some(NativeOptionValue {
                id: "autonomous".to_owned(),
                native_value: "workspace-write".to_owned(),
            }),
        }];
        catalog.scope = Some(NativeCatalogScope {
            profile_id: "profile-main".to_owned(),
            working_directory: "C:/workspace".to_owned(),
            workspace_trust: "safe".to_owned(),
        });
        catalog
    }

    fn value_bytes(value: &Value) -> Vec<u8> {
        serde_json::to_vec(value).expect("fixture value encodes")
    }

    fn catalog_value_for_test() -> Value {
        let catalog = dynamic_catalog();
        let bytes = encode_catalog(&catalog).expect("fixture catalog encodes");
        serde_json::from_slice(&bytes).expect("fixture wire is JSON")
    }

    #[test]
    fn complete_static_and_runtime_snapshot_roundtrips_losslessly() {
        let catalog = dynamic_catalog();
        let encoded = encode_catalog(&catalog).expect("complete catalog encodes");
        assert!(encoded.len() < MAX_WIRE_BYTES);
        let decoded = decode_catalog(&encoded).expect("complete catalog decodes");
        assert_eq!(decoded, catalog);
        assert_eq!(
            encode_catalog(&decoded).expect("decoded catalog re-encodes"),
            encoded
        );
    }

    #[test]
    fn opencode2_identity_matches_electron_key_order_and_omitted_variant() {
        assert_eq!(
            opencode2_catalog_id("x-preview-f-free", "opencode", None).expect("identity is valid"),
            "opencode2:eyJtb2RlbF9pZCI6IngtcHJldmlldy1mLWZyZWUiLCJwcm92aWRlcl9yb3V0ZV9pZCI6Im9wZW5jb2RlIn0"
        );
        let with_variant = opencode2_catalog_id("x-preview-f-free", "opencode", Some("high"))
            .expect("variant identity is valid");
        assert_eq!(
            with_variant,
            "opencode2:eyJtb2RlbF9pZCI6IngtcHJldmlldy1mLWZyZWUiLCJwcm92aWRlcl9yb3V0ZV9pZCI6Im9wZW5jb2RlIiwidmFyaWFudF9pZCI6ImhpZ2gifQ"
        );
    }

    #[test]
    fn wrong_scope_route_and_native_identity_are_rejected() {
        let mut wrong_scope = catalog_value_for_test();
        wrong_scope["runtime"]["scope"]["profile_id"] = json!("");
        assert_eq!(
            decode_catalog(&value_bytes(&wrong_scope)),
            Err(NativeModelCatalogWireError::InvalidIdentifier)
        );

        let mut wrong_route = catalog_value_for_test();
        wrong_route["runtime"]["routes"][0]["engine_id"] = json!("missing-engine");
        assert_eq!(
            decode_catalog(&value_bytes(&wrong_route)),
            Err(NativeModelCatalogWireError::UnknownReference)
        );

        let mut wrong_native_identity = catalog_value_for_test();
        let dynamic = wrong_native_identity["manifest"]["models"]
            .as_array_mut()
            .expect("models array")
            .last_mut()
            .expect("dynamic model");
        dynamic["native_selection"]["model_id"] = json!("other-model");
        assert_eq!(
            decode_catalog(&value_bytes(&wrong_native_identity)),
            Err(NativeModelCatalogWireError::InvalidNativeIdentity)
        );
    }

    #[test]
    fn malformed_numbers_oversize_unknown_fields_and_duplicate_ids_are_rejected() {
        let mut malformed_number = catalog_value_for_test();
        malformed_number["version"] = json!(1.5);
        assert_eq!(
            decode_catalog(&value_bytes(&malformed_number)),
            Err(NativeModelCatalogWireError::InvalidNumber)
        );

        assert_eq!(
            decode_catalog(&vec![b' '; MAX_WIRE_BYTES + 1]),
            Err(NativeModelCatalogWireError::PayloadTooLarge)
        );

        let mut unknown_field = catalog_value_for_test();
        unknown_field["runtime"]["unexpected"] = json!(true);
        assert_eq!(
            decode_catalog(&value_bytes(&unknown_field)),
            Err(NativeModelCatalogWireError::UnknownField)
        );

        let mut duplicate_favorite = catalog_value_for_test();
        let favorite = duplicate_favorite["runtime"]["favorite_ids"][0].clone();
        duplicate_favorite["runtime"]["favorite_ids"]
            .as_array_mut()
            .expect("favorite array")
            .push(favorite);
        assert_eq!(
            decode_catalog(&value_bytes(&duplicate_favorite)),
            Err(NativeModelCatalogWireError::DuplicateIdentifier)
        );
    }

    #[test]
    fn favorite_wire_snapshot_has_the_domain_byte_ceiling() {
        let mut catalog = NativeModelCatalog::offline().expect("bundled catalog is valid");
        catalog.favorite_ids = (0..MAX_FAVORITES)
            .map(|index| {
                format!(
                    "{index}{}",
                    "\\".repeat(MAX_FAVORITE_ID_BYTES - index.to_string().len())
                )
            })
            .collect();
        assert_eq!(
            encode_catalog(&catalog),
            Err(NativeModelCatalogWireError::PayloadTooLarge)
        );
    }
}
