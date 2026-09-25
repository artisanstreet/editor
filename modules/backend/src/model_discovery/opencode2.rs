//! `OpenCode2` model discovery via the installed `opencode2` CLI.
//!
//! `opencode2 models` prints the model identifiers the engine offers as
//! `provider/model` lines (the same route/model pairs the harness runs).
//! The CLI reports no names or limits, so the engine's own catalogue cache
//! (`~/.cache/opencode/models.json`, the models.dev data `OpenCode` ships and
//! refreshes for its own picker) supplies display metadata when present.
//! Missing metadata degrades to a humanized identifier, never to an invented
//! capability: thinking stays engine-managed and unexposed here.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;

use super::process::run_bounded;
use super::{DiscoveredModel, DiscoveredThinking};

/// Total deadline for listing, including the cold-start retry.
const DEADLINE: Duration = Duration::from_secs(15);
/// Output bound for the listing command.
const MAX_BYTES: usize = 1024 * 1024;
/// Maximum accepted model rows from one listing.
const MAX_MODELS: usize = 1024;
/// Maximum bytes in one listing line.
const MAX_LINE_BYTES: usize = 512;
/// Maximum catalogue cache size retained for metadata.
const MAX_CACHE_BYTES: u64 = 64 * 1024 * 1024;

/// Probes the `OpenCode2` CLI; `None` when it does not answer.
pub(super) async fn discover_opencode2(program: Option<&str>) -> Option<Vec<DiscoveredModel>> {
    let executable = program?;
    // The CLI can finish its cold initialization with an empty successful
    // response. Retry once; this is not an authoritative empty catalogue.
    let mut identifiers = Vec::new();
    let deadline = tokio::time::Instant::now() + DEADLINE;
    for _ in 0..2 {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if let Some(output) =
            Box::pin(run_bounded(executable, &["models"], remaining, MAX_BYTES)).await
            && output.success
        {
            identifiers = parse_model_list(&output.stdout);
            if !identifiers.is_empty() {
                break;
            }
        }
    }
    if identifiers.is_empty() {
        return None;
    }
    let metadata = read_models_cache();
    Some(
        identifiers
            .into_iter()
            .flat_map(|(provider, model)| {
                let entry = metadata.get(&(provider.clone(), model.clone()));
                rows_for(&provider, &model, entry)
            })
            .collect(),
    )
}

/// Builds the base row plus one row per engine-reported effort variant.
///
/// `OpenCode` exposes reasoning levels as native variants (its own config sets
/// `model.variant`), and the runtime path models each as its own identity
/// row. The cache's `reasoning_options` effort values are exactly those
/// variant ids; toggle and budget options are not derivable and stay out.
fn rows_for(provider: &str, model: &str, entry: Option<&CacheEntry>) -> Vec<DiscoveredModel> {
    let mut rows = vec![row(provider, model, entry, None)];
    if let Some(entry) = entry {
        for variant in &entry.variants {
            rows.push(row(provider, model, Some(entry), Some(variant.clone())));
        }
    }
    rows
}

/// Parses `provider/model` lines, skipping anything that is not one bounded
/// identifier pair. Duplicates collapse to the first occurrence.
fn parse_model_list(output: &str) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    let mut seen = HashSet::new();
    for line in output.lines() {
        if rows.len() >= MAX_MODELS {
            break;
        }
        let line = line.trim();
        if line.is_empty() || line.len() > MAX_LINE_BYTES {
            continue;
        }
        let Some((provider, model)) = line.split_once('/') else {
            continue;
        };
        if provider.is_empty()
            || model.is_empty()
            || provider.bytes().any(|byte| byte.is_ascii_control())
            || model.bytes().any(|byte| byte.is_ascii_control())
        {
            continue;
        }
        let pair = (provider.to_owned(), model.to_owned());
        if seen.insert(pair.clone()) {
            rows.push(pair);
        }
    }
    rows
}

/// Display metadata retained from `OpenCode`'s own catalogue cache.
#[derive(Default)]
struct CacheEntry {
    name: Option<String>,
    description: Option<String>,
    context: Option<u64>,
    output: Option<u64>,
    image: bool,
    tools: bool,
    cost: Option<(f64, f64)>,
    variants: Vec<String>,
}

/// Locates the engine's catalogue cache under the user home or an explicit
/// XDG/local cache root.
fn models_cache_path() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME") {
        candidates.push(PathBuf::from(xdg).join("opencode").join("models.json"));
    }
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        candidates.push(
            PathBuf::from(home)
                .join(".cache")
                .join("opencode")
                .join("models.json"),
        );
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(
            PathBuf::from(local)
                .join("opencode")
                .join("cache")
                .join("models.json"),
        );
    }
    candidates.into_iter().find(|path| path.is_file())
}

/// Reads `(provider, model) -> metadata` from the engine cache. Any failure
/// degrades to no metadata rather than to invented fields.
fn read_models_cache() -> HashMap<(String, String), CacheEntry> {
    let Some(path) = models_cache_path() else {
        return HashMap::new();
    };
    let Ok(metadata) = std::fs::metadata(&path) else {
        return HashMap::new();
    };
    if metadata.len() > MAX_CACHE_BYTES {
        return HashMap::new();
    }
    let Ok(bytes) = std::fs::read(&path) else {
        return HashMap::new();
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return HashMap::new();
    };
    let Some(providers) = value.as_object() else {
        return HashMap::new();
    };
    let mut entries = HashMap::new();
    for (provider_id, provider) in providers {
        let Some(models) = provider.get("models").and_then(Value::as_object) else {
            continue;
        };
        for (model_id, model) in models {
            entries.insert((provider_id.clone(), model_id.clone()), cache_entry(model));
        }
    }
    entries
}

fn cache_entry(model: &Value) -> CacheEntry {
    CacheEntry {
        name: text(model.get("name")),
        description: text(model.get("description")),
        context: u64_at(model, "limit", "context"),
        output: u64_at(model, "limit", "output"),
        image: model
            .get("modalities")
            .and_then(|modalities| modalities.get("input"))
            .and_then(Value::as_array)
            .is_some_and(|inputs| inputs.iter().any(|input| input.as_str() == Some("image"))),
        tools: model
            .get("tool_call")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        cost: cost_pair(model.get("cost")),
        variants: reasoning_variants(model.get("reasoning_options")),
    }
}

/// Effort values reported as reasoning options; these are the native variant
/// ids the engine accepts. Toggle/budget options are ignored, never guessed.
fn reasoning_variants(value: Option<&Value>) -> Vec<String> {
    /// Maximum retained variants for one model.
    const MAX_VARIANTS: usize = 8;
    let Some(options) = value.and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut variants = Vec::new();
    for option in options {
        if option.get("type").and_then(Value::as_str) != Some("effort") {
            continue;
        }
        let Some(values) = option.get("values").and_then(Value::as_array) else {
            continue;
        };
        for value in values {
            let Some(value) = value
                .as_str()
                .filter(|value| !value.is_empty() && value.len() <= 32)
            else {
                continue;
            };
            if variants.len() >= MAX_VARIANTS {
                return variants;
            }
            if !variants.iter().any(|existing| existing == value) {
                variants.push(value.to_owned());
            }
        }
    }
    variants
}

fn text(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

fn u64_at(value: &Value, table: &str, key: &str) -> Option<u64> {
    value.get(table).and_then(|table| table.get(key))?.as_u64()
}

fn cost_pair(value: Option<&Value>) -> Option<(f64, f64)> {
    let value = value?;
    let input = value.get("input").and_then(Value::as_f64)?;
    let output = value.get("output").and_then(Value::as_f64)?;
    (input.is_finite() && input >= 0.0 && output.is_finite() && output >= 0.0)
        .then_some((input, output))
}

fn row(
    provider: &str,
    model: &str,
    entry: Option<&CacheEntry>,
    variant_id: Option<String>,
) -> DiscoveredModel {
    DiscoveredModel {
        engine_id: "opencode2",
        provider: provider.to_owned(),
        native_model_id: model.to_owned(),
        upstream_model_id: Some(model.to_owned()),
        variant_id,
        name: entry
            .and_then(|entry| entry.name.clone())
            .unwrap_or_else(|| humanized_model_name(model)),
        description: entry.and_then(|entry| entry.description.clone()),
        hidden: false,
        default: false,
        thinking: DiscoveredThinking::Unavailable,
        fast: false,
        context_window_tokens: entry.and_then(|entry| entry.context),
        max_context_window_tokens: entry.and_then(|entry| entry.context),
        output_tokens: entry.and_then(|entry| entry.output),
        image_input: entry.is_some_and(|entry| entry.image),
        tools: entry.is_some_and(|entry| entry.tools),
        web_search: false,
        cost: entry.and_then(|entry| entry.cost),
        status: "active",
        metadata_confidence: "reported",
    }
}

/// Humanizes an identifier when the engine cache has no display name.
fn humanized_model_name(model_id: &str) -> String {
    let mut words = Vec::new();
    for segment in model_id.split('-') {
        if segment.is_empty() {
            continue;
        }
        let word = match segment.to_ascii_lowercase().as_str() {
            "gpt" => "GPT".to_owned(),
            "glm" => "GLM".to_owned(),
            "deepseek" => "DeepSeek".to_owned(),
            "qwen" => "Qwen".to_owned(),
            "grok" => "Grok".to_owned(),
            "kimi" => "Kimi".to_owned(),
            "claude" => "Claude".to_owned(),
            "gemini" => "Gemini".to_owned(),
            "muse" => "Muse".to_owned(),
            "spark" => "Spark".to_owned(),
            other => capitalize(other),
        };
        words.push(word);
    }
    words.join(" ")
}

fn capitalize(segment: &str) -> String {
    let mut chars = segment.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn retries_empty_cold_start_before_publishing_models() {
        use std::os::unix::fs::PermissionsExt;
        let directory =
            std::env::temp_dir().join(format!("artisan-model-probe-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let program = directory.join("models");
        std::fs::write(&program, "#!/bin/sh\nif [ ! -e \"$0.ready\" ]; then touch \"$0.ready\"; exit 0; fi\nprintf 'test-provider/test-model\\n'\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let models = discover_opencode2(program.to_str()).await.unwrap();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].provider, "test-provider");
        assert_eq!(models[0].native_model_id, "test-model");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn parses_provider_model_lines_and_skips_noise() {
        let rows = parse_model_list(
            "opencode-go/kimi-k3\n\nnot a model\nopencode/claude-opus-5\nopencode-go/kimi-k3\n",
        );
        assert_eq!(
            rows,
            vec![
                ("opencode-go".to_owned(), "kimi-k3".to_owned()),
                ("opencode".to_owned(), "claude-opus-5".to_owned()),
            ]
        );
    }

    #[test]
    fn cache_metadata_maps_reported_fields_only() {
        let model = serde_json::json!({
            "name": "Kimi K3",
            "description": "Frontier open model",
            "limit": { "context": 262_144, "output": 262_144 },
            "modalities": { "input": ["text", "image"] },
            "tool_call": true,
            "reasoning": true,
            "reasoning_options": [{ "type": "effort", "values": ["low", "high", "max"] }],
            "cost": { "input": 0.95, "output": 4.0 },
        });
        let entry = cache_entry(&model);
        assert_eq!(entry.name.as_deref(), Some("Kimi K3"));
        assert_eq!(entry.context, Some(262_144));
        assert_eq!(entry.output, Some(262_144));
        assert!(entry.image);
        assert!(entry.tools);
        assert_eq!(entry.cost, Some((0.95, 4.0)));
        assert_eq!(entry.variants, vec!["low", "high", "max"]);
    }

    #[test]
    fn variant_rows_expand_the_base_without_inventing_options() {
        let entry = CacheEntry {
            name: Some("Kimi K3".to_owned()),
            variants: vec!["high".to_owned(), "max".to_owned()],
            ..CacheEntry::default()
        };
        let rows = rows_for("opencode-go", "kimi-k3", Some(&entry));
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].variant_id, None);
        assert_eq!(rows[1].variant_id.as_deref(), Some("high"));
        assert_eq!(rows[2].variant_id.as_deref(), Some("max"));
        assert!(rows.iter().all(|row| row.native_model_id == "kimi-k3"));
    }

    #[test]
    fn missing_metadata_degrades_to_a_humanized_name() {
        assert_eq!(
            humanized_model_name("muse-spark-1.3-contributor"),
            "Muse Spark 1.3 Contributor"
        );
        assert_eq!(humanized_model_name("glm-5.3-flash"), "GLM 5.3 Flash");
        assert_eq!(
            humanized_model_name("deepseek-v4.1-flash"),
            "DeepSeek V4.1 Flash"
        );
    }
}
