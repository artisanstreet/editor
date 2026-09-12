//! Validated `model.options` inventory mappings from the live gateway.

use std::collections::HashSet;

use serde_json::Value;
use thiserror::Error;

/// One validated provider/model pair from `model.options`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InventoryEntry {
    provider: String,
    model: String,
    enabled: bool,
}

/// Validated live `model.options` inventory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ValidatedInventory {
    entries: Vec<InventoryEntry>,
}

/// Failure validating the live `model.options` inventory.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Error)]
pub(crate) enum InventoryError {
    #[error("hermes model inventory was malformed")]
    InvalidShape,
    #[error("hermes model inventory contained a duplicate model")]
    DuplicateModel,
}

/// Validates the live `model.options` inventory without inferring fields.
///
/// Requires the top-level `providers` array and well-typed required fields;
/// entries missing their slug, name, or model list are skipped because they
/// cannot be attributed, while present-but-mistyped fields reject the whole
/// inventory fail-closed. Duplicate `(provider, model)` pairs reject:
/// either the gateway or the transport tampered with the rows. Unknown extra
/// fields are ignored, never inferred into identities, prices, or flags.
///
/// # Errors
///
/// Returns [`InventoryError`] when the shape is not a provider inventory or
/// a provider/model pair repeats.
pub(crate) fn validate_model_options_inventory(
    value: &Value,
) -> Result<ValidatedInventory, InventoryError> {
    let providers = value
        .get("providers")
        .and_then(Value::as_array)
        .ok_or(InventoryError::InvalidShape)?;
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    for provider in providers {
        let Some(object) = provider.as_object() else {
            continue;
        };
        let (Some(slug), Some(name), Some(models)) = (
            object.get("slug").and_then(Value::as_str),
            object.get("name").and_then(Value::as_str),
            object.get("models").and_then(Value::as_array),
        ) else {
            continue;
        };
        if slug.is_empty() || name.is_empty() {
            continue;
        }
        let authenticated = object
            .get("authenticated")
            .map(|value| value.as_bool().ok_or(InventoryError::InvalidShape))
            .transpose()?
            .unwrap_or(true);
        let unavailable = match object.get("unavailable_models") {
            None => HashSet::new(),
            Some(list) => list
                .as_array()
                .ok_or(InventoryError::InvalidShape)?
                .iter()
                .map(|entry| {
                    entry
                        .as_str()
                        .filter(|text| !text.is_empty())
                        .ok_or(InventoryError::InvalidShape)
                        .map(str::to_owned)
                })
                .collect::<Result<HashSet<_>, _>>()?,
        };
        for model in models {
            let model_id = model.as_str().ok_or(InventoryError::InvalidShape)?;
            if model_id.is_empty() {
                return Err(InventoryError::InvalidShape);
            }
            if !seen.insert((slug.to_owned(), model_id.to_owned())) {
                return Err(InventoryError::DuplicateModel);
            }
            entries.push(InventoryEntry {
                provider: slug.to_owned(),
                model: model_id.to_owned(),
                enabled: authenticated && !unavailable.contains(model_id),
            });
        }
    }
    Ok(ValidatedInventory { entries })
}

/// Returns whether the validated inventory enables the selected route/model.
#[must_use]
pub(crate) fn inventory_supports(inventory: &ValidatedInventory, route: &str, model: &str) -> bool {
    inventory
        .entries
        .iter()
        .any(|entry| entry.enabled && entry.provider == route && entry.model == model)
}
