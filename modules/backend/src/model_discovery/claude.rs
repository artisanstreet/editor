//! Claude Code model discovery from the published signed catalogue plus the
//! CLI's served (per-org) cache.
//!
//! The published document at `downloads.claude.ai/model-catalog/v1/` is the
//! anonymous baseline and is explicitly published for third-party Claude Code
//! clients. The served cache the CLI writes for the signed-in account
//! (`~/.claude/cache/model-catalog/<org>-<scope>-cc.json`) is the exact list
//! and order the CLI's own picker shows, including org-scoped and
//! confidential rows; it omits runtime limits, which come from the matching
//! published row. Rows never leave this process except through the catalogue
//! itself, and their order is always the provider's, never re-sorted here.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Map, Value};

use super::{
    DiscoveredModel, DiscoveredThinking, DiscoveredThinkingOption, artisan_level, effort_economics,
};

/// Published catalogue URL.
const CATALOG_URL: &str = "https://downloads.claude.ai/model-catalog/v1/catalog.json";
/// Hard bound for one catalogue body.
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
/// Surface this client renders.
const SURFACE: &str = "cc";

/// Probes Claude; `None` when neither the published document nor a served
/// cache is readable.
pub(super) async fn discover_claude(home: Option<PathBuf>) -> Option<Vec<DiscoveredModel>> {
    let published = fetch_published().await;
    let served = match home {
        Some(home) => read_served_cache(&home).await,
        None => None,
    };
    let rows = merge_rows(published.as_ref(), served.as_ref());
    (!rows.is_empty()).then_some(rows)
}

/// Builds rows in the provider's order: the account's served list when the
/// CLI has written one, otherwise the published list. Served fields win; the
/// published row with the same id fills what the served row omits.
fn merge_rows(published: Option<&Value>, served: Option<&Value>) -> Vec<DiscoveredModel> {
    let published = published.map(published_models).unwrap_or_default();
    let served = served.map(served_models).unwrap_or_default();
    let ordered = if served.is_empty() {
        published
            .iter()
            .map(|model| (*model).clone())
            .collect::<Vec<_>>()
    } else {
        served
            .iter()
            .map(|model| {
                let id = model.get("id").and_then(Value::as_str);
                match published.iter().find(|candidate| {
                    id.is_some() && candidate.get("id").and_then(Value::as_str) == id
                }) {
                    Some(base) => overlay(base, model),
                    None => (*model).clone(),
                }
            })
            .collect()
    };
    let mut rows: Vec<DiscoveredModel> = Vec::with_capacity(ordered.len());
    for model in &ordered {
        if let Some(row) = map_model(model)
            && !rows
                .iter()
                .any(|existing| existing.native_model_id == row.native_model_id)
        {
            rows.push(row);
        }
    }
    rows
}

/// Shallow-merges `top` over `base`, keeping base keys `top` does not carry.
fn overlay(base: &Value, top: &Value) -> Value {
    let mut merged: Map<String, Value> = base.as_object().cloned().unwrap_or_default();
    if let Some(top) = top.as_object() {
        for (key, value) in top {
            merged.insert(key.clone(), value.clone());
        }
    }
    Value::Object(merged)
}

async fn fetch_published() -> Option<Value> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(4))
        .user_agent("artisan-editor/model-discovery")
        .build()
        .ok()?;
    let response = client.get(CATALOG_URL).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_BODY_BYTES as u64)
    {
        return None;
    }
    let bytes = response.bytes().await.ok()?;
    if bytes.len() > MAX_BODY_BYTES {
        return None;
    }
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    // Version and schema are the document's own contract; a malformed or
    // future document is ignored rather than partially trusted.
    if value.get("schema_version").and_then(Value::as_u64) != Some(1) {
        return None;
    }
    value.get("version").and_then(Value::as_u64)?;
    Some(value)
}

/// Returns the served cache's model rows for the `cc` surface, in order.
fn served_models(value: &Value) -> Vec<&Value> {
    let Some(catalog) = value.get("catalog") else {
        return Vec::new();
    };
    if catalog.get("surface").and_then(Value::as_str) != Some(SURFACE) {
        return Vec::new();
    }
    catalog
        .get("config")
        .filter(|config| config.get("id").and_then(Value::as_str) == Some(SURFACE))
        .and_then(|config| config.get("models"))
        .and_then(Value::as_array)
        .map(|models| models.iter().collect())
        .unwrap_or_default()
}

/// Returns the published document's model rows of the `cc` surface, in order.
fn published_models(value: &Value) -> Vec<&Value> {
    let configs = value
        .get("surfaces")
        .and_then(|surfaces| surfaces.get(SURFACE))
        .and_then(|surface| surface.get("model_selector_config"))
        .and_then(Value::as_array);
    let Some(configs) = configs else {
        return Vec::new();
    };
    configs
        .iter()
        .filter(|config| config.get("id").and_then(Value::as_str) == Some(SURFACE))
        .flat_map(|config| {
            config
                .get("models")
                .and_then(Value::as_array)
                .map(|models| models.iter().collect::<Vec<_>>())
                .unwrap_or_default()
        })
        .collect()
}

fn map_model(value: &Value) -> Option<DiscoveredModel> {
    let native_model_id = value.get("id").and_then(Value::as_str)?.to_owned();
    if native_model_id.is_empty() {
        return None;
    }
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or(&native_model_id)
        .to_owned();
    let description = value
        .get("description")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_owned);
    let offered_on = value.get("offered_on").and_then(Value::as_array);
    if let Some(offered_on) = offered_on
        && !offered_on
            .iter()
            .any(|entry| entry.as_str() == Some("first_party"))
    {
        return None;
    }
    let runtime = value.get("runtime").unwrap_or(&Value::Null);
    let context_window_tokens = runtime.get("max_input_tokens").and_then(Value::as_u64);
    let output_tokens = runtime.get("max_output_tokens").and_then(Value::as_u64);
    let thinking = map_thinking(value, runtime);
    let capabilities = value.get("capabilities").unwrap_or(&Value::Null);
    let image_input = capabilities
        .get("mm_images")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let web_search = capabilities
        .get("web_search")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let fast = value.get("fast_mode").is_some_and(|mode| !mode.is_null());
    // Confidential rows are entitlement-scoped, not hidden: the CLI renders
    // them for the accounts that are allowed to see them.
    Some(DiscoveredModel {
        engine_id: "claude",
        provider: "anthropic".to_owned(),
        native_model_id,
        upstream_model_id: None,
        variant_id: None,
        name,
        description,
        hidden: false,
        default: false,
        thinking,
        fast,
        context_window_tokens,
        max_context_window_tokens: context_window_tokens,
        output_tokens,
        image_input,
        tools: true,
        web_search,
        cost: None,
        status: "active",
        metadata_confidence: "reported",
    })
}

fn map_thinking(value: &Value, runtime: &Value) -> DiscoveredThinking {
    let thinking = value.get("thinking").unwrap_or(&Value::Null);
    let kind = thinking
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("none");
    if kind == "none" {
        return DiscoveredThinking::Unavailable;
    }
    let default_effort = runtime
        .get("default_effort")
        .and_then(Value::as_str)
        .and_then(artisan_level);
    let mut default = default_effort.map(str::to_owned);
    let mut options = Vec::new();
    if let Some(effort_options) = thinking.get("effort_options").and_then(Value::as_array) {
        for entry in effort_options {
            let Some(native_value) = entry.get("id").and_then(Value::as_str) else {
                continue;
            };
            let Some(level) = artisan_level(native_value) else {
                continue;
            };
            let (economics, presentation_group) = effort_economics(level);
            let description = entry
                .get("description")
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty())
                .map(str::to_owned)
                .or_else(|| {
                    entry
                        .get("name")
                        .and_then(Value::as_str)
                        .filter(|text| !text.is_empty())
                        .map(str::to_owned)
                });
            let is_default = entry
                .get("badge")
                .and_then(|badge| badge.get("message"))
                .and_then(Value::as_str)
                == Some("Default");
            if is_default {
                default = Some(level.to_owned());
            }
            options.push(DiscoveredThinkingOption {
                id: level.to_owned(),
                native_value: native_value.to_owned(),
                description,
                economics,
                presentation_group,
            });
        }
    }
    let Some(default) = default.filter(|level| options.iter().any(|option| option.id == *level))
    else {
        return DiscoveredThinking::Unavailable;
    };
    if options.is_empty() {
        return DiscoveredThinking::Unavailable;
    }
    DiscoveredThinking::Supported { default, options }
}

/// Reads the most recently fetched served `*-cc.json` cache the CLI wrote,
/// which is the list its picker currently shows for the signed-in account.
async fn read_served_cache(home: &Path) -> Option<Value> {
    let directory = home.join(".claude").join("cache").join("model-catalog");
    let mut entries = tokio::fs::read_dir(&directory).await.ok()?;
    let mut newest: Option<(u64, Value)> = None;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.ends_with("-cc.json") {
            continue;
        }
        let Ok(bytes) = tokio::fs::read(entry.path()).await else {
            continue;
        };
        if bytes.len() > MAX_BODY_BYTES {
            continue;
        }
        let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        if served_models(&value).is_empty() {
            continue;
        }
        let fetched_at = value.get("fetchedAt").and_then(Value::as_u64).unwrap_or(0);
        if newest
            .as_ref()
            .is_none_or(|(newest_at, _)| fetched_at > *newest_at)
        {
            newest = Some((fetched_at, value));
        }
    }
    newest.map(|(_, value)| value)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[expect(
        clippy::needless_pass_by_value,
        reason = "test helper takes a JSON fixture by value for call-site symmetry with serde_json::json!"
    )]
    fn surface_value(models: serde_json::Value) -> Value {
        json!({
            "schema_version": 1,
            "version": 140,
            "surfaces": { "cc": { "model_selector_config": [{ "id": "cc", "models": models }] } }
        })
    }

    #[test]
    fn maps_published_rows_with_efforts_and_limits() {
        let value = surface_value(json!([{
            "id": "claude-fable-5-1",
            "name": "Fable 5.1",
            "description": "For your toughest challenges",
            "offered_on": ["first_party", "bedrock"],
            "capabilities": { "mm_images": true, "web_search": true },
            "thinking": {
                "type": "effort",
                "effort_options": [
                    { "id": "low", "name": "Low" },
                    { "id": "medium", "name": "Medium" },
                    { "id": "high", "name": "High", "badge": { "message": "Default", "variant": "neutral" } },
                    { "id": "xhigh", "name": "Extra" },
                    { "id": "max", "name": "Max" }
                ]
            },
            "fast_mode": { "type": "toggle" },
            "runtime": {
                "max_input_tokens": 1_000_000,
                "max_output_tokens": 128_000,
                "effort_levels": ["low", "medium", "high", "xhigh", "max"],
                "default_effort": "high"
            }
        }]));
        let models = published_models(&value);
        assert_eq!(models.len(), 1);
        let row = map_model(models[0]).expect("row maps");
        assert_eq!(row.native_model_id, "claude-fable-5-1");
        assert_eq!(
            row.description.as_deref(),
            Some("For your toughest challenges")
        );
        assert_eq!(row.context_window_tokens, Some(1_000_000));
        assert_eq!(row.output_tokens, Some(128_000));
        assert!(row.image_input);
        assert!(row.web_search);
        assert!(row.fast);
        let DiscoveredThinking::Supported { default, options } = &row.thinking else {
            panic!("expected supported thinking");
        };
        assert_eq!(default, "high");
        assert_eq!(
            options
                .iter()
                .map(|option| (option.id.as_str(), option.native_value.as_str()))
                .collect::<Vec<_>>(),
            [
                ("light", "low"),
                ("medium", "medium"),
                ("high", "high"),
                ("xhigh", "xhigh"),
                ("max", "max")
            ]
        );
    }

    #[test]
    fn non_first_party_rows_are_skipped() {
        let value = surface_value(json!([{
            "id": "claude-opus-4-1-20250805",
            "name": "Opus 4.1",
            "offered_on": ["bedrock", "vertex"],
            "thinking": { "type": "none" },
            "runtime": { "max_input_tokens": 200_000, "max_output_tokens": 32000 }
        }]));
        assert!(
            published_models(&value)
                .iter()
                .all(|model| map_model(model).is_none())
        );
    }

    #[test]
    fn thinking_none_maps_to_unavailable() {
        let value = surface_value(json!([{
            "id": "claude-haiku-4-5",
            "name": "Haiku 4.5",
            "offered_on": ["first_party"],
            "thinking": { "type": "none" },
            "runtime": { "max_input_tokens": 200_000, "max_output_tokens": 64000 }
        }]));
        let row = map_model(published_models(&value)[0]).expect("row maps");
        assert_eq!(row.thinking, DiscoveredThinking::Unavailable);
        assert_eq!(row.context_window_tokens, Some(200_000));
    }

    fn ids(rows: &[DiscoveredModel]) -> Vec<&str> {
        rows.iter()
            .map(|row| row.native_model_id.as_str())
            .collect()
    }

    fn published_row(id: &str, name: &str, context: u64) -> serde_json::Value {
        json!({
            "id": id,
            "name": name,
            "offered_on": ["first_party"],
            "thinking": { "type": "none" },
            "runtime": { "max_input_tokens": context, "max_output_tokens": 64000 }
        })
    }

    #[test]
    fn published_rows_keep_the_published_order() {
        let published = surface_value(json!([
            published_row("claude-opus-5-5", "Opus 5.5", 1_000_000),
            published_row("claude-sonnet-5", "Sonnet 5", 1_000_000),
            published_row("claude-fable-5-1", "Fable 5.1", 1_000_000),
            published_row("claude-fable-5", "Fable 5", 1_000_000),
        ]));
        let rows = merge_rows(Some(&published), None);
        assert_eq!(
            ids(&rows),
            [
                "claude-opus-5-5",
                "claude-sonnet-5",
                "claude-fable-5-1",
                "claude-fable-5"
            ]
        );
    }

    #[test]
    fn served_cache_sets_order_and_membership_and_published_fills_limits() {
        let published = surface_value(json!([
            published_row("claude-opus-5-5", "Opus 5.5", 1_000_000),
            published_row("claude-sonnet-5", "Sonnet 5", 1_000_000),
            published_row("claude-fable-5-1", "Fable 5.1", 1_000_000),
            published_row("claude-fable-5", "Fable 5", 1_000_000),
            published_row("claude-opus-4-1", "Opus 4.1", 200_000),
        ]));
        let served = json!({
            "version": 2,
            "fetchedAt": 1,
            "catalog": {
                "surface": "cc",
                "config": {
                    "id": "cc",
                    "models": [
                        { "id": "claude-opus-5-5", "name": "Opus 5.5", "section": "main", "thinking": { "type": "none" } },
                        { "id": "claude-fable-5-1", "name": "Fable 5.1", "section": "main", "thinking": { "type": "none" } },
                        { "id": "claude-sonnet-5", "name": "Sonnet 5", "section": "main", "thinking": { "type": "none" } },
                        { "id": "claude-fable-5", "name": "Fable 5", "section": "overflow", "thinking": { "type": "none" } },
                        { "id": "claude-confidential", "name": "Confidential", "section": "main", "thinking": { "type": "none" } }
                    ]
                }
            }
        });
        let rows = merge_rows(Some(&published), Some(&served));
        assert_eq!(
            ids(&rows),
            [
                "claude-opus-5-5",
                "claude-fable-5-1",
                "claude-sonnet-5",
                "claude-fable-5",
                "claude-confidential"
            ],
            "the account's served list decides order and membership"
        );
        assert_eq!(rows[1].context_window_tokens, Some(1_000_000));
        assert_eq!(rows[4].context_window_tokens, None);
    }
}
