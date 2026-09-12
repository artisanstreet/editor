//! Versioned, bounded JSON transport for a complete native model catalog.
//!
//! The bundled manifest remains the baseline and is never replaced by this
//! module. A wire snapshot carries that baseline plus the owner-supplied
//! runtime layer, provenance, scope, and authoritative favorites; runtime
//! overlay may replace reported fields on bundled rows and append new rows,
//! but every bundled provider and model identity must survive. The schema
//! is intentionally explicit rather than derived from `Debug`: unknown keys,
//! malformed numbers, oversized collections, stale references, and mismatched
//! `OpenCode2` native identities are rejected before a snapshot is returned.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use thiserror::Error;

mod decode;
mod encode;
mod validate;

pub use decode::decode_catalog;
pub use encode::{encode_catalog, opencode2_catalog_id};

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

#[cfg(test)]
mod tests {
    use crate::{
        NativeCatalogScope, NativeContextSelection, NativeModelCapabilities, NativeModelCatalog,
        NativeModelDefaults, NativeModelDefinition, NativeModelProvider, NativeModelRoute,
        NativeModelRouteGroup, NativeModelRouteStatus, NativeModelRouting, NativeModelSelection,
        NativeOptionValue, NativeThinkingCapability,
    };
    use serde_json::{Value, json};

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
