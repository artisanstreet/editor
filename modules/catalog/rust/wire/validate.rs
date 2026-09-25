//! Semantic catalog validation and bounded wire readers.

use std::collections::HashSet;

use serde_json::{Map, Value};

use super::encode::opencode2_catalog_id;
use super::{
    MAX_FAVORITE_SNAPSHOT_BYTES, MAX_FAVORITES, MAX_NATIVE_IDENTIFIER_BYTES,
    MAX_SCOPE_PROFILE_BYTES, MAX_SCOPE_WORKING_DIRECTORY_BYTES, MAX_WIRE_GATEWAYS,
    MAX_WIRE_HARNESSES, MAX_WIRE_IDENTIFIER_BYTES, MAX_WIRE_MODEL_DEFAULTS, MAX_WIRE_MODELS,
    MAX_WIRE_OPTIONS, MAX_WIRE_PROVIDERS, MAX_WIRE_ROUTES, MAX_WIRE_TEXT_BYTES,
    NativeModelCatalogWireError,
};
use crate::{
    NativeCatalogScope, NativeHarness, NativeModelCapabilities, NativeModelCatalog,
    NativeModelDefaults, NativeModelDefinition, NativeModelManifest, NativeModelRoute,
    NativeModelRouteGroup, NativeModelRouteStatus, NativeModelRouting, NativeThinkingCapability,
};
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

pub(super) fn validate_catalog_runtime(
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

pub(super) fn validate_manifest_semantics(
    manifest: &NativeModelManifest,
    runtime_routes: Option<&[NativeModelRoute]>,
) -> Result<(), NativeModelCatalogWireError> {
    validate_shipped_harnesses(manifest)?;
    let provider_ids = validate_manifest_providers(manifest)?;
    let harness_ids = validate_manifest_harnesses(manifest)?;
    validate_manifest_models(manifest, runtime_routes, &provider_ids, &harness_ids)
}

fn validate_manifest_providers(
    manifest: &NativeModelManifest,
) -> Result<HashSet<&str>, NativeModelCatalogWireError> {
    let mut provider_ids = HashSet::with_capacity(manifest.providers.len());
    for provider in &manifest.providers {
        validate_identifier_str(&provider.id)?;
        validate_text_str(&provider.label)?;
        if !provider_ids.insert(provider.id.as_str()) {
            return Err(NativeModelCatalogWireError::DuplicateIdentifier);
        }
    }
    Ok(provider_ids)
}

fn validate_manifest_harnesses(
    manifest: &NativeModelManifest,
) -> Result<HashSet<&str>, NativeModelCatalogWireError> {
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
    Ok(harness_ids)
}

fn validate_manifest_models(
    manifest: &NativeModelManifest,
    runtime_routes: Option<&[NativeModelRoute]>,
    provider_ids: &HashSet<&str>,
    harness_ids: &HashSet<&str>,
) -> Result<(), NativeModelCatalogWireError> {
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

/// Compares shipped and runtime harness descriptors by identity, ignoring the
/// presentation-only hidden flag that discovery may set.
fn harnesses_match_identity(runtime: &[NativeHarness], shipped: &[NativeHarness]) -> bool {
    runtime.len() == shipped.len()
        && runtime.iter().zip(shipped).all(|(runtime, shipped)| {
            let mut runtime = runtime.clone();
            let mut shipped = shipped.clone();
            runtime.hidden = false;
            runtime.compaction_default_model_id = None;
            shipped.compaction_default_model_id = None;
            shipped.hidden = false;
            runtime == shipped
        })
}

fn validate_shipped_harnesses(
    manifest: &NativeModelManifest,
) -> Result<(), NativeModelCatalogWireError> {
    if manifest.revision != crate::NATIVE_MODEL_CATALOG_REVISION {
        return Err(NativeModelCatalogWireError::InvalidCatalog);
    }
    let shipped = NativeModelCatalog::harnesses_only()
        .map_err(|_| NativeModelCatalogWireError::InvalidCatalog)?
        .manifest;
    if !harnesses_match_identity(&manifest.harnesses, &shipped.harnesses) {
        return Err(NativeModelCatalogWireError::InvalidCatalog);
    }

    Ok(())
}

fn validate_model_capabilities(
    capabilities: &NativeModelCapabilities,
) -> Result<(), NativeModelCatalogWireError> {
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

pub(super) fn validate_model_defaults(
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

pub(super) fn checked_object(
    value: &Value,
) -> Result<&Map<String, Value>, NativeModelCatalogWireError> {
    value
        .as_object()
        .ok_or(NativeModelCatalogWireError::InvalidShape)
}

pub(super) fn exact_keys(
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

pub(super) fn required<'a>(
    object: &'a Map<String, Value>,
    key: &str,
) -> Result<&'a Value, NativeModelCatalogWireError> {
    object
        .get(key)
        .ok_or(NativeModelCatalogWireError::InvalidShape)
}

pub(super) fn optional_value<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    object.get(key).filter(|value| !value.is_null())
}

pub(super) fn array(value: &Value) -> Result<&Vec<Value>, NativeModelCatalogWireError> {
    value
        .as_array()
        .ok_or(NativeModelCatalogWireError::InvalidShape)
}

pub(super) fn bounded_len(
    length: usize,
    maximum: usize,
) -> Result<(), NativeModelCatalogWireError> {
    if length > maximum {
        Err(NativeModelCatalogWireError::TooManyItems)
    } else {
        Ok(())
    }
}

pub(super) fn read_identifier(value: &Value) -> Result<&str, NativeModelCatalogWireError> {
    read_bounded_identifier(value, MAX_WIRE_IDENTIFIER_BYTES)
}

pub(super) fn read_bounded_identifier(
    value: &Value,
    maximum: usize,
) -> Result<&str, NativeModelCatalogWireError> {
    let string = value
        .as_str()
        .ok_or(NativeModelCatalogWireError::InvalidShape)?;
    validate_bounded_identifier_str(string, maximum)?;
    Ok(string)
}

pub(super) fn read_text(value: &Value) -> Result<&str, NativeModelCatalogWireError> {
    read_bounded_text(value, MAX_WIRE_TEXT_BYTES, false)
}

pub(super) fn read_bounded_text(
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

pub(super) fn read_text_allow_empty(value: &Value) -> Result<&str, NativeModelCatalogWireError> {
    read_bounded_text(value, MAX_WIRE_TEXT_BYTES, true)
}

pub(super) fn optional_identifier<'a>(
    object: &'a Map<String, Value>,
    key: &str,
) -> Result<Option<&'a str>, NativeModelCatalogWireError> {
    optional_value(object, key).map(read_identifier).transpose()
}

pub(super) fn optional_text<'a>(
    object: &'a Map<String, Value>,
    key: &str,
) -> Result<Option<&'a str>, NativeModelCatalogWireError> {
    optional_value(object, key).map(read_text).transpose()
}

pub(super) fn read_bool(value: &Value) -> Result<bool, NativeModelCatalogWireError> {
    value
        .as_bool()
        .ok_or(NativeModelCatalogWireError::InvalidShape)
}

pub(super) fn optional_bool(
    object: &Map<String, Value>,
    key: &str,
) -> Result<Option<bool>, NativeModelCatalogWireError> {
    optional_value(object, key).map(read_bool).transpose()
}

pub(super) fn read_u64(value: &Value) -> Result<u64, NativeModelCatalogWireError> {
    if let Some(number) = value.as_u64() {
        return Ok(number);
    }
    if value.is_number() {
        Err(NativeModelCatalogWireError::InvalidNumber)
    } else {
        Err(NativeModelCatalogWireError::InvalidShape)
    }
}

pub(super) fn read_u32(value: &Value) -> Result<u32, NativeModelCatalogWireError> {
    let number = read_u64(value)?;
    u32::try_from(number).map_err(|_| NativeModelCatalogWireError::InvalidNumber)
}

pub(super) fn optional_u64(
    object: &Map<String, Value>,
    key: &str,
) -> Result<Option<u64>, NativeModelCatalogWireError> {
    optional_value(object, key).map(read_u64).transpose()
}

pub(super) fn optional_f64(
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

pub(super) fn validate_identifier_str(value: &str) -> Result<(), NativeModelCatalogWireError> {
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

fn optional_finite_f64(value: Option<f64>) -> Result<(), NativeModelCatalogWireError> {
    if let Some(value) = value
        && (!value.is_finite() || value < 0.0)
    {
        return Err(NativeModelCatalogWireError::InvalidNumber);
    }
    Ok(())
}

pub(super) fn optional_nonnegative_f64(
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
