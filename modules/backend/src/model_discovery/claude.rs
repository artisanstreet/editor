//! Claude Code model discovery from the published signed catalogue plus the
//! CLI's served (per-org) cache.
//!
//! The published document at `downloads.claude.ai/model-catalog/v1/` is the
//! anonymous baseline and is explicitly published for third-party Claude Code
//! clients. Org-scoped and confidential rows exist only in the served cache
//! the CLI writes for the signed-in account
//! (`~/.claude/cache/model-catalog/<org>-<scope>-cc.json`); those rows are
//! merged over the baseline and never leave this process except through the
//! catalogue itself.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;

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
pub(super) async fn discover_claude() -> Option<Vec<DiscoveredModel>> {
    let mut rows: BTreeMap<String, DiscoveredModel> = BTreeMap::new();

    if let Some(value) = fetch_published().await {
        for model in surface_models(&value) {
            if let Some(row) = map_model(model) {
                rows.insert(row.native_model_id.clone(), row);
            }
        }
    }
    for value in read_served_cache().await {
        for model in surface_models(&value) {
            if let Some(row) = map_model(model) {
                rows.insert(row.native_model_id.clone(), row);
            }
        }
    }

    if rows.is_empty() {
        return None;
    }
    Some(rows.into_values().collect())
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
    if value.get("version").and_then(Value::as_u64).is_none() {
        return None;
    }
    Some(value)
}

/// Returns the model rows of the `cc` surface, if present.
fn surface_models(value: &Value) -> Vec<&Value> {
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
    let hidden = value.get("confidential").and_then(Value::as_bool) == Some(true);
    Some(DiscoveredModel {
        engine_id: "claude",
        provider: "anthropic".to_owned(),
        native_model_id,
        upstream_model_id: None,
        name,
        description,
        hidden,
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

/// Reads every served `*-cc.json` cache entry the CLI has written.
async fn read_served_cache() -> Vec<Value> {
    let Some(directory) = claude_cache_directory() else {
        return Vec::new();
    };
    let Ok(mut entries) = tokio::fs::read_dir(&directory).await else {
        return Vec::new();
    };
    let mut values = Vec::new();
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
        if value
            .get("catalog")
            .and_then(|catalog| catalog.get("surface"))
            .and_then(Value::as_str)
            == Some(SURFACE)
        {
            values.push(value);
        }
    }
    values
}

fn claude_cache_directory() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("CLAUDE_CONFIG_DIR") {
        let home = home.trim();
        if !home.is_empty() {
            return Some(PathBuf::from(home).join("cache").join("model-catalog"));
        }
    }
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .ok()?;
    (!home.trim().is_empty()).then(|| {
        PathBuf::from(home)
            .join(".claude")
            .join("cache")
            .join("model-catalog")
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

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
                "max_input_tokens": 1000000,
                "max_output_tokens": 128000,
                "effort_levels": ["low", "medium", "high", "xhigh", "max"],
                "default_effort": "high"
            }
        }]));
        let models = surface_models(&value);
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
            "runtime": { "max_input_tokens": 200000, "max_output_tokens": 32000 }
        }]));
        assert!(
            surface_models(&value)
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
            "runtime": { "max_input_tokens": 200000, "max_output_tokens": 64000 }
        }]));
        let row = map_model(surface_models(&value)[0]).expect("row maps");
        assert_eq!(row.thinking, DiscoveredThinking::Unavailable);
        assert_eq!(row.context_window_tokens, Some(200_000));
    }
}
