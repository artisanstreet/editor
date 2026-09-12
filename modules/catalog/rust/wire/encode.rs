//! Deterministic catalog wire encoding and `OpenCode2` identity construction.

use serde_json::{Value, json};

use super::decode::validate_wire_document;
use super::validate::{validate_catalog_runtime, validate_identifier_str};
use super::{MAX_WIRE_BYTES, MAX_WIRE_IDENTIFIER_BYTES, NativeModelCatalogWireError, WIRE_VERSION};
use crate::{
    NativeCatalogRuntime, NativeCatalogScope, NativeContextConfig, NativeContextSelection,
    NativeContextWindowCapability, NativeContextWindowOption, NativeHarness,
    NativeModelCapabilities, NativeModelCatalog, NativeModelCost, NativeModelDefaults,
    NativeModelDefinition, NativeModelGateway, NativeModelManifest, NativeModelProvider,
    NativeModelRoute, NativeModelRouteGroup, NativeModelRouteStatus, NativeModelRouting,
    NativeModelSelection, NativeOptionValue, NativePermissionOption, NativeSpeedOption,
    NativeThinkingCapability, NativeThinkingOption,
};
/// Encodes the complete static-plus-runtime catalog snapshot.
///
/// The emitted object is deterministic for a given catalog and contains no
/// debug representation or raw provider envelope. The encoded length is
/// checked after serialization and never exceeds [`MAX_WIRE_BYTES`].
///
/// # Errors
///
/// Returns [`NativeModelCatalogWireError`] when the runtime layer references
/// unknown identities, serialization fails, or the encoded snapshot exceeds
/// [`MAX_WIRE_BYTES`] or fails its semantic validation.
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

/// Computes the exact `OpenCode2` catalog identity used by the Electron
/// adapter. The JSON key order is `model_id`, `provider_route_id`, followed by
/// the optional `variant_id`; omitted variants remain omitted rather than
/// becoming `null`.
///
/// # Errors
///
/// Returns [`NativeModelCatalogWireError::InvalidIdentifier`] when any
/// supplied identifier violates the wire identifier contract, or
/// [`NativeModelCatalogWireError::EncodeFailed`] when JSON escaping fails.
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
        "hidden": harness.hidden,
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
