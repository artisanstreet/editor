//! JSON-shape decoding helpers and parsers for the catalog manifest.

use serde_json::{Map, Value};

use crate::manifest::{
    NativeContextConfig, NativeContextWindowCapability, NativeContextWindowOption,
    NativeDisabledModel, NativeHarness, NativeModelCapabilities, NativeModelCatalogError,
    NativeModelCost, NativeModelDefinition, NativeModelGateway, NativeModelRouting,
    NativeModelSelection, NativePermissionCapability, NativePermissionOption, NativeSpeedOption,
    NativeThinkingCapability, NativeThinkingOption,
};
pub(crate) fn value_object<'a>(
    value: &'a Value,
    path: &str,
) -> Result<&'a Map<String, Value>, NativeModelCatalogError> {
    value
        .as_object()
        .ok_or_else(|| NativeModelCatalogError::invalid(path, "expected an object"))
}

pub(crate) fn array<'a>(
    value: &'a Value,
    path: &str,
) -> Result<&'a Vec<Value>, NativeModelCatalogError> {
    value
        .as_array()
        .ok_or_else(|| NativeModelCatalogError::invalid(path, "expected an array"))
}

pub(crate) fn required_value<'a>(
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

pub(crate) fn required_string(
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

pub(crate) fn parse_harness(
    value: &Value,
    index: usize,
) -> Result<NativeHarness, NativeModelCatalogError> {
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
        hidden: optional_bool(object, "hidden", &path)?.unwrap_or(false),
    })
}

pub(crate) fn parse_model(
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
    let capabilities = parse_capabilities(capabilities_object, &capabilities_path)?;
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
        capabilities,
    })
}

fn parse_capabilities(
    capabilities_object: &Map<String, Value>,
    capabilities_path: &str,
) -> Result<NativeModelCapabilities, NativeModelCatalogError> {
    let speed_options = array(
        required_value(capabilities_object, "speed_options", capabilities_path)?,
        &format!("{capabilities_path}.speed_options"),
    )?
    .iter()
    .enumerate()
    .map(|(index, value)| parse_speed_option(value, capabilities_path, index))
    .collect::<Result<Vec<_>, NativeModelCatalogError>>()?;
    let thinking = parse_thinking(
        required_value(capabilities_object, "thinking", capabilities_path)?,
        capabilities_path,
    )?;
    let context_window = optional_value(capabilities_object, "context_window")
        .map(|value| parse_context_capability(value, capabilities_path))
        .transpose()?;
    Ok(NativeModelCapabilities {
        context_window_tokens: optional_u64(
            capabilities_object,
            "context_window_tokens",
            capabilities_path,
        )?,
        context_window,
        image_input: required_bool(capabilities_object, "image_input", capabilities_path)?,
        local_tools: required_bool(capabilities_object, "local_tools", capabilities_path)?,
        mcp: required_bool(capabilities_object, "mcp", capabilities_path)?,
        output_tokens: optional_u64(capabilities_object, "output_tokens", capabilities_path)?,
        reasoning_display: optional_string(
            capabilities_object,
            "reasoning_display",
            capabilities_path,
        )?,
        speed_options,
        thinking,
        web_search: required_bool(capabilities_object, "web_search", capabilities_path)?,
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
