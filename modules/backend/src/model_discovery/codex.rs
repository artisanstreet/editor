//! Codex app-server model discovery.
//!
//! Spawns `codex app-server --stdio`, completes the documented handshake,
//! and reads `model/list` pages with `includeHidden: true`. Entitlement is
//! server-owned: preview/stealth models for the signed-in account appear as
//! rows here, including rows the CLI keeps out of its own picker. Context
//! windows are not part of `model/list`; when the CLI's
//! `models_cache.json` is present it supplies `context_window` and
//! `max_context_window` per slug.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Instant;

use artisan_native_engine::codex::{
    CodexDiscoveryInput, codex_local_root, resolve_codex_executable,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};

use super::{
    DiscoveredModel, DiscoveredThinking, DiscoveredThinkingOption, artisan_level, effort_economics,
};

/// Maximum accepted models from one Codex catalog.
const MAX_MODELS: usize = 512;
/// Maximum accepted pages.
const MAX_PAGES: usize = 8;

/// Probes Codex; `None` when the executable is absent or the handshake fails.
pub(super) async fn discover_codex() -> Option<Vec<DiscoveredModel>> {
    let executable = resolve_codex_command();
    let deadline = Instant::now() + super::ENGINE_DEADLINE;
    let mut session = CodexSession::start(&executable).ok()?;
    let result = session.list_models(deadline).await;
    session.close().await;
    let rows = result?;
    let context = read_context_windows();
    Some(rows.into_iter().map(|row| map_row(row, &context)).collect())
}

/// Resolves the Codex executable with the same precedence the runtime uses:
/// explicit override, per-user install, WinGet, then eligible PATH entries.
fn resolve_codex_command() -> String {
    let configured_executable = std::env::var("ARTISAN_CODEX_EXECUTABLE").ok();
    let local_app_data = std::env::var("LOCALAPPDATA").ok().map(PathBuf::from);
    let path_entries = std::env::var("PATH")
        .map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
        .unwrap_or_default();
    let root = codex_local_root(local_app_data.as_deref());
    let directory_names = std::fs::read_dir(&root)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.path().is_dir())
                .filter_map(|entry| entry.file_name().into_string().ok())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let architecture = match std::env::consts::ARCH {
        "aarch64" => "arm64".to_owned(),
        other => other.to_owned(),
    };
    let input = CodexDiscoveryInput {
        architecture,
        configured_executable,
        local_app_data,
        platform_windows: cfg!(windows),
        path_entries,
        directory_names,
    };
    resolve_codex_executable(&input, &|candidate| candidate.is_file())
        .to_string_lossy()
        .into_owned()
}

struct RawCodexModel {
    id: String,
    display_name: String,
    description: Option<String>,
    hidden: bool,
    is_default: bool,
    default_effort: Option<String>,
    efforts: Vec<(String, String)>,
    input_modalities: Vec<String>,
    fast: bool,
    supports_search: bool,
}

struct CodexSession {
    child: Child,
    reader: BufReader<tokio::process::ChildStdout>,
}

impl CodexSession {
    fn start(executable: &str) -> Result<Self, ()> {
        let mut child = Command::new(executable)
            .args(["app-server", "--stdio"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| ())?;
        let stdout = child.stdout.take().ok_or(())?;
        Ok(Self {
            child,
            reader: BufReader::new(stdout),
        })
    }

    async fn send(&mut self, value: Value) -> Result<(), ()> {
        let stdin = self.child.stdin.as_mut().ok_or(())?;
        let mut line = serde_json::to_vec(&value).map_err(|_| ())?;
        line.push(b'\n');
        stdin.write_all(&line).await.map_err(|_| ())?;
        stdin.flush().await.map_err(|_| ())
    }

    /// Waits for one response by id, ignoring notifications and other ids.
    async fn response(&mut self, id: u64, deadline: Instant) -> Result<Value, ()> {
        loop {
            let mut line = String::new();
            let read = tokio::time::timeout_at(deadline.into(), self.reader.read_line(&mut line))
                .await
                .map_err(|_| ())?
                .map_err(|_| ())?;
            if read == 0 {
                return Err(());
            }
            let value: Value = serde_json::from_str(&line).map_err(|_| ())?;
            if value.get("id").and_then(Value::as_u64) == Some(id) {
                return Ok(value);
            }
        }
    }

    async fn list_models(&mut self, deadline: Instant) -> Option<Vec<RawCodexModel>> {
        self.send(json!({
            "id": 1,
            "method": "initialize",
            "params": {
                "clientInfo": { "name": "artisan-editor", "version": "0.3.0" },
                "capabilities": { "experimentalApi": false },
            },
        }))
        .await
        .ok()?;
        self.response(1, deadline).await.ok()?;
        self.send(json!({ "method": "initialized" })).await.ok()?;

        let mut models = Vec::new();
        let mut cursor: Option<String> = None;
        for page in 0..MAX_PAGES {
            let id = 2 + page as u64;
            let params = match cursor.as_ref() {
                Some(cursor) => json!({ "cursor": cursor, "includeHidden": true }),
                None => json!({ "includeHidden": true }),
            };
            self.send(json!({ "id": id, "method": "model/list", "params": params }))
                .await
                .ok()?;
            let response = self.response(id, deadline).await.ok()?;
            let result = response.get("result")?;
            let data = result.get("data").and_then(Value::as_array)?;
            if models.len() + data.len() > MAX_MODELS {
                return None;
            }
            for value in data {
                if let Some(model) = parse_model(value) {
                    models.push(model);
                }
            }
            cursor = result
                .get("nextCursor")
                .and_then(Value::as_str)
                .filter(|cursor| !cursor.is_empty())
                .map(str::to_owned);
            if cursor.is_none() {
                return Some(models);
            }
        }
        Some(models)
    }

    async fn close(&mut self) {
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
    }
}

fn parse_model(value: &Value) -> Option<RawCodexModel> {
    let id = value.get("id").and_then(Value::as_str)?.to_owned();
    let display_name = value
        .get("displayName")
        .and_then(Value::as_str)
        .unwrap_or(&id)
        .to_owned();
    let description = value
        .get("description")
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_owned);
    let hidden = value
        .get("hidden")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let is_default = value
        .get("isDefault")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let default_effort = value
        .get("defaultReasoningEffort")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let efforts = value
        .get("supportedReasoningEfforts")
        .and_then(Value::as_array)
        .map(|options| {
            options
                .iter()
                .filter_map(|option| {
                    let effort = option.get("reasoningEffort").and_then(Value::as_str)?;
                    let description = option
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    Some((effort.to_owned(), description.to_owned()))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let input_modalities = value
        .get("inputModalities")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let fast = value
        .get("additionalSpeedTiers")
        .and_then(Value::as_array)
        .is_some_and(|tiers| tiers.iter().any(|tier| tier.as_str() == Some("fast")))
        || value
            .get("serviceTiers")
            .and_then(Value::as_array)
            .is_some_and(|tiers| {
                tiers
                    .iter()
                    .any(|tier| tier.get("id").and_then(Value::as_str) == Some("priority"))
            });
    Some(RawCodexModel {
        id,
        display_name,
        description,
        hidden,
        is_default,
        default_effort,
        efforts,
        input_modalities,
        fast,
        supports_search: true,
    })
}

fn map_row(row: RawCodexModel, context: &HashMap<String, (u64, u64)>) -> DiscoveredModel {
    let options = row
        .efforts
        .iter()
        .filter_map(|(effort, description)| {
            let level = artisan_level(effort)?;
            let (economics, presentation_group) = effort_economics(level);
            Some(DiscoveredThinkingOption {
                id: level.to_owned(),
                native_value: effort.clone(),
                description: (!description.is_empty()).then(|| description.clone()),
                economics,
                presentation_group,
            })
        })
        .collect::<Vec<_>>();
    let default = row
        .default_effort
        .as_deref()
        .and_then(artisan_level)
        .filter(|level| options.iter().any(|option| option.id == *level))
        .map(str::to_owned)
        .or_else(|| options.first().map(|option| option.id.clone()));
    let thinking = match default {
        Some(default) if !options.is_empty() => DiscoveredThinking::Supported { default, options },
        _ => DiscoveredThinking::Unavailable,
    };
    let (context_window_tokens, max_context_window_tokens) = context
        .get(&row.id)
        .copied()
        .map_or((None, None), |(context, max)| (Some(context), Some(max)));
    DiscoveredModel {
        engine_id: "codex",
        provider: "openai".to_owned(),
        native_model_id: row.id.clone(),
        upstream_model_id: Some(row.id),
        name: row.display_name,
        description: row.description,
        hidden: row.hidden,
        default: row.is_default,
        thinking,
        fast: row.fast,
        context_window_tokens,
        max_context_window_tokens,
        output_tokens: None,
        image_input: row.input_modalities.iter().any(|item| item == "image"),
        tools: true,
        web_search: row.supports_search,
        cost: None,
        status: "active",
        metadata_confidence: "reported",
    }
}

/// Reads the CLI-maintained model cache for context windows.
fn read_context_windows() -> HashMap<String, (u64, u64)> {
    let Some(path) = codex_models_cache_path() else {
        return HashMap::new();
    };
    let Ok(bytes) = std::fs::read(path) else {
        return HashMap::new();
    };
    if bytes.len() > 4 * 1024 * 1024 {
        return HashMap::new();
    }
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return HashMap::new();
    };
    let mut map = HashMap::new();
    if let Some(models) = value.get("models").and_then(Value::as_array) {
        for model in models {
            let Some(slug) = model.get("slug").and_then(Value::as_str) else {
                continue;
            };
            let context = model.get("context_window").and_then(Value::as_u64);
            let max = model.get("max_context_window").and_then(Value::as_u64);
            if let (Some(context), Some(max)) = (context, max) {
                map.insert(slug.to_owned(), (context, max));
            }
        }
    }
    map
}

fn codex_models_cache_path() -> Option<PathBuf> {
    if let Ok(home) = std::env::var("CODEX_HOME") {
        let home = home.trim();
        if !home.is_empty() {
            return Some(PathBuf::from(home).join("models_cache.json"));
        }
    }
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .ok()?;
    (!home.trim().is_empty()).then(|| PathBuf::from(home).join(".codex").join("models_cache.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_rows_map_efforts_and_context() {
        let value: Value = serde_json::from_str(
            r#"{
                "id": "gpt-test",
                "displayName": "GPT Test",
                "description": "Test model.",
                "hidden": false,
                "isDefault": true,
                "defaultReasoningEffort": "medium",
                "supportedReasoningEfforts": [
                    {"reasoningEffort": "low", "description": "Quick"},
                    {"reasoningEffort": "medium", "description": "Balanced"},
                    {"reasoningEffort": "ultra", "description": "Delegates"}
                ],
                "inputModalities": ["text", "image"],
                "additionalSpeedTiers": ["fast"],
                "serviceTiers": [{"id": "priority", "name": "Fast", "description": "1.5x"}]
            }"#,
        )
        .expect("fixture decodes");
        let raw = parse_model(&value).expect("row parses");
        let context = HashMap::from([("gpt-test".to_owned(), (272_000_u64, 872_000_u64))]);
        let mapped = map_row(raw, &context);
        assert_eq!(mapped.native_model_id, "gpt-test");
        assert!(mapped.hidden == false);
        assert!(mapped.default);
        assert!(mapped.image_input);
        assert!(mapped.fast);
        assert_eq!(mapped.context_window_tokens, Some(272_000));
        assert_eq!(mapped.max_context_window_tokens, Some(872_000));
        let DiscoveredThinking::Supported { default, options } = &mapped.thinking else {
            panic!("expected supported thinking");
        };
        assert_eq!(default, "medium");
        assert_eq!(
            options
                .iter()
                .map(|option| (option.id.as_str(), option.native_value.as_str()))
                .collect::<Vec<_>>(),
            [("light", "low"), ("medium", "medium"), ("ultra", "ultra")]
        );
        let ultra = options.last().expect("ultra option");
        assert_eq!(ultra.economics, "harness-orchestration");
        assert_eq!(ultra.presentation_group, "special");
    }

    #[test]
    fn hidden_rows_are_preserved() {
        let value: Value = serde_json::from_str(
            r#"{
                "id": "gpt-reserve",
                "displayName": "GPT-Reserve",
                "hidden": true,
                "supportedReasoningEfforts": [
                    {"reasoningEffort": "low", "description": "Quick"}
                ]
            }"#,
        )
        .expect("fixture decodes");
        let mapped = map_row(parse_model(&value).expect("row parses"), &HashMap::new());
        assert!(mapped.hidden);
        assert!(!mapped.default);
    }
}
