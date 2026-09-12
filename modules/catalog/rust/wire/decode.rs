//! Catalog wire decoding, shape validation, and snapshot reconstruction.

use std::collections::HashSet;

use serde_json::Value;

use super::validate::{
    array, bounded_len, checked_object, exact_keys, optional_bool, optional_f64,
    optional_identifier, optional_nonnegative_f64, optional_text, optional_u64, optional_value,
    read_bool, read_bounded_identifier, read_bounded_text, read_identifier, read_text,
    read_text_allow_empty, read_u32, read_u64, required, validate_catalog_runtime,
    validate_manifest_semantics, validate_model_defaults,
};
use super::{
    MAX_FAVORITES, MAX_SCOPE_PROFILE_BYTES, MAX_SCOPE_WORKING_DIRECTORY_BYTES, MAX_WIRE_BYTES,
    MAX_WIRE_GATEWAYS, MAX_WIRE_HARNESSES, MAX_WIRE_MODEL_DEFAULTS, MAX_WIRE_MODELS,
    MAX_WIRE_OPTIONS, MAX_WIRE_PROVIDERS, MAX_WIRE_ROUTES, NativeModelCatalogWireError,
    WIRE_VERSION,
};
use crate::{
    NativeCatalogProvenance, NativeCatalogRuntime, NativeCatalogScope, NativeContextConfig,
    NativeContextSelection, NativeModelCatalog, NativeModelDefaults, NativeModelManifest,
    NativeModelRoute, NativeModelRouteGroup, NativeModelRouteStatus, NativeOptionValue,
};
/// Decodes and validates one complete catalog snapshot.
///
/// The byte ceiling is checked before `serde_json` sees the input. Every
/// collection is checked before its values are converted into owned domain
/// strings, and runtime references are checked against the decoded manifest.
///
/// # Errors
///
/// Returns [`NativeModelCatalogWireError`] when the payload exceeds
/// [`MAX_WIRE_BYTES`], is not valid JSON, or violates any schema, bound,
/// identity, or reference rule of the wire document.
pub fn decode_catalog(bytes: &[u8]) -> Result<NativeModelCatalog, NativeModelCatalogWireError> {
    if bytes.len() > MAX_WIRE_BYTES {
        return Err(NativeModelCatalogWireError::PayloadTooLarge);
    }
    let value: Value =
        serde_json::from_slice(bytes).map_err(|_| NativeModelCatalogWireError::InvalidJson)?;
    decode_wire_value(&value)
}

pub(super) fn validate_wire_document(value: &Value) -> Result<(), NativeModelCatalogWireError> {
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
            "hidden",
            "id",
            "gateways",
            "label",
            "permissions",
        ],
    )?;
    optional_identifier(object, "compaction_default_model_id")?;
    let _ = optional_bool(object, "hidden")?;
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
