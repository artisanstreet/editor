use crate::engines::{ModelSource, host_spec};
use artisan_api::{ContextWindow, EffortNote, EngineId, Model, SlashCommand, Speed, Variant};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

const CLAUDE_CATALOG: &str = "https://downloads.claude.ai/model-catalog/v1/catalog.json";
const DEADLINE: Duration = Duration::from_secs(15);
const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
const MAX_MODELS: usize = 1024;

#[cfg(test)]
#[path = "../tests/catalog/speeds.rs"]
mod speed_tests;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Listing {
    pub models: Vec<Model>,
    pub default: Option<String>,
    pub commands: Option<Vec<SlashCommand>>,
}

pub(crate) struct Program<'a> {
    pub binary: Option<&'a Path>,
    pub env: &'a [(String, String)],
    pub home: &'a Path,
}

#[derive(Debug, thiserror::Error)]
#[error("the engine is not installed")]
pub(crate) struct NotInstalled;

#[derive(Debug, thiserror::Error)]
pub(crate) enum ListModelsError {
    #[error("read the Claude model cache")]
    ClaudeCache(#[source] tokio::task::JoinError),
    #[error("neither the published nor the cached Claude catalog is available")]
    NoClaudeCatalog,
    #[error(transparent)]
    NotInstalled(NotInstalled),
    #[error("ask codex app-server for its models")]
    CodexCall(#[source] crate::rpc::CallError),
    #[error("codex model/list omitted data")]
    CodexNoData,
    #[error("codex model/list timed out after {}s", DEADLINE.as_secs())]
    CodexTimedOut,
    #[error("read the Codex model cache")]
    CodexCache(#[source] tokio::task::JoinError),
    #[error("ask OpenCode for its models")]
    OpenCodeRun(#[source] RunError),
    #[error("parse OpenCode model.list")]
    OpenCodeDecode(#[source] serde_json::Error),
    #[error("opencode listed no models")]
    OpenCodeEmpty,
}

pub(crate) async fn list(
    engine: EngineId,
    program: Program<'_>,
    http: &reqwest::Client,
) -> Result<Listing, ListModelsError> {
    let listing = match host_spec(engine).models {
        ModelSource::ClaudeCatalog => {
            let (listing, commands) =
                tokio::join!(claude(http, program.home), claude_commands(&program));
            let mut listing = listing?;
            match commands {
                Ok(commands) => listing.commands = Some(commands),
                Err(error) => {
                    tracing::debug!(
                        error = diagnostics::chain(&error),
                        "Claude commands unavailable"
                    );
                }
            }
            listing
        }
        ModelSource::CodexAppServer => codex(&program).await?,
        ModelSource::OpenCodeCli => opencode(&program, http).await?,
    };
    Ok(finish(engine, listing))
}

fn finish(engine: EngineId, mut listing: Listing) -> Listing {
    listing
        .models
        .retain(|model| !model.id.is_empty() && !model.id.contains(char::is_whitespace));
    listing.default = listing
        .default
        .filter(|id| listing.models.iter().any(|model| &model.id == id));
    for model in &mut listing.models {
        model.name = readable_name(&model.name);
    }
    if listing.models.len() > MAX_MODELS {
        tracing::warn!(%engine, count = listing.models.len(), "engine listed too many models; keeping {MAX_MODELS}");
        listing.models.truncate(MAX_MODELS);
    }
    listing
}

fn command(program: &Program<'_>) -> Result<tokio::process::Command, NotInstalled> {
    let binary = program.binary.ok_or(NotInstalled)?;
    let mut command = tokio::process::Command::new(binary);
    command
        .env_remove("CLAUDECODE")
        .env_remove("CLAUDE_CODE_ENTRYPOINT")
        .envs(program.env.iter().map(|(key, value)| (key, value)))
        .current_dir(program.home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    Ok(command)
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum RunError {
    #[error(transparent)]
    NotInstalled(NotInstalled),
    #[error("run `{args}`")]
    Probe {
        args: String,
        #[source]
        source: crate::process::ProbeError,
    },
    #[error("`{args}` failed ({status}): {message}")]
    Failed {
        args: String,
        status: std::process::ExitStatus,
        message: String,
    },
}

async fn run(program: &Program<'_>, args: &[&str]) -> Result<String, RunError> {
    let mut command = command(program).map_err(RunError::NotInstalled)?;
    command.args(args);
    let output = crate::process::probe(command, DEADLINE, MAX_OUTPUT_BYTES)
        .await
        .map_err(|source| RunError::Probe {
            args: args.join(" "),
            source,
        })?;
    let (status, out, err) = (output.status, output.stdout, output.stderr);
    if !status.success() {
        let stderr = String::from_utf8_lossy(&err);
        let stdout = String::from_utf8_lossy(&out);
        let message = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        return Err(RunError::Failed {
            args: args.join(" "),
            status,
            message: message.to_owned(),
        });
    }
    Ok(String::from_utf8_lossy(&out).into_owned())
}

fn read_json_file(path: &Path) -> Option<Value> {
    use std::io::Read as _;
    let mut bytes = Vec::new();
    let read = std::fs::File::open(path).and_then(|file| {
        file.take(MAX_OUTPUT_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
    });
    match read {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            tracing::debug!(path = %path.display(), %error, "skipped an unreadable model cache");
            return None;
        }
    }
    if bytes.len() > MAX_OUTPUT_BYTES {
        tracing::warn!(path = %path.display(), "model cache exceeds byte budget");
        return None;
    }
    match serde_json::from_slice(&bytes) {
        Ok(value) => Some(value),
        Err(error) => {
            tracing::debug!(path = %path.display(), %error, "skipped a model cache that isn't JSON");
            None
        }
    }
}

fn readable_name(raw: &str) -> String {
    raw.split(|c: char| c == '-' || c.is_whitespace())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) const STANDARD_SPEED: &str = "standard";

#[derive(Debug, thiserror::Error)]
pub(crate) enum FetchCatalogError {
    #[error("fetch {url}")]
    Request {
        url: &'static str,
        #[source]
        source: reqwest::Error,
    },
    #[error("{url} exceeds {limit} bytes")]
    TooLarge { url: &'static str, limit: usize },
    #[error("decode {url}")]
    Decode {
        url: &'static str,
        #[source]
        source: serde_json::Error,
    },
}

async fn fetch_catalog(
    http: &reqwest::Client,
    url: &'static str,
    timeout: Duration,
    limit: usize,
) -> Result<Value, FetchCatalogError> {
    let request = |source| FetchCatalogError::Request { url, source };
    let mut response = http
        .get(url)
        .timeout(timeout)
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(request)?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(request)? {
        if bytes.len().saturating_add(chunk.len()) > limit {
            return Err(FetchCatalogError::TooLarge { url, limit });
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|source| FetchCatalogError::Decode { url, source })
}

async fn claude(http: &reqwest::Client, home: &Path) -> Result<Listing, ListModelsError> {
    let published = fetch_catalog(
        http,
        CLAUDE_CATALOG,
        Duration::from_secs(5),
        MAX_OUTPUT_BYTES,
    )
    .await;
    let published = match published {
        Ok(value) if value.get("schema_version").and_then(Value::as_u64) == Some(1) => Some(value),
        Ok(_) => None,
        Err(error) => {
            tracing::debug!(
                error = diagnostics::chain(&error),
                "Claude model catalog unavailable"
            );
            None
        }
    };
    let directory = home.join(".claude/cache/model-catalog");
    let served = tokio::task::spawn_blocking(move || claude_served_cache(&directory))
        .await
        .map_err(ListModelsError::ClaudeCache)?;
    let listing = merge_claude(published.as_ref(), served.as_ref());
    if listing.models.is_empty() {
        return Err(ListModelsError::NoClaudeCatalog);
    }
    Ok(listing)
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ClaudeCommandsError {
    #[error(transparent)]
    NotInstalled(NotInstalled),
    #[error("start Claude for its commands")]
    Spawn(#[source] crate::process::SpawnError),
    #[error("ask Claude to initialize")]
    Send(#[source] crate::process::SendError),
    #[error("read Claude's initialize answer")]
    Read(#[source] crate::process::ReadError),
    #[error("Claude closed before answering initialize: {stderr}")]
    Closed { stderr: String },
    #[error("Claude initialize timed out after {}s", DEADLINE.as_secs())]
    TimedOut,
}

async fn claude_commands(program: &Program<'_>) -> Result<Vec<SlashCommand>, ClaudeCommandsError> {
    let mut command = command(program).map_err(ClaudeCommandsError::NotInstalled)?;
    command
        .args([
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
        ])
        .stdin(Stdio::piped());
    let mut process = crate::process::spawn_probe(command).map_err(ClaudeCommandsError::Spawn)?;
    let reply = tokio::time::timeout(DEADLINE, async {
        process
            .send(&json!({
                "type": "control_request",
                "request_id": "artisan-commands",
                "request": { "subtype": "initialize" },
            }))
            .await
            .map_err(ClaudeCommandsError::Send)?;
        while let Some(line) = process
            .next_line()
            .await
            .map_err(ClaudeCommandsError::Read)?
        {
            let Ok(frame) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if frame["type"] == "control_response"
                && frame["response"]["request_id"] == "artisan-commands"
            {
                return Ok(frame["response"]["response"]["commands"].clone());
            }
        }
        Err(ClaudeCommandsError::Closed {
            stderr: process.stderr_tail(),
        })
    })
    .await
    .map_err(|_elapsed| ClaudeCommandsError::TimedOut)??;
    Ok(reply
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|command| {
            Some(SlashCommand {
                name: command["name"].as_str()?.to_owned(),
                description: text(command.get("description")).unwrap_or_default(),
                hint: text(command.get("argumentHint")),
            })
        })
        .collect())
}

fn claude_served_cache(dir: &Path) -> Option<Value> {
    let mut newest: Option<(u64, Value)> = None;
    let mut matched = 0;
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            tracing::debug!(directory = %dir.display(), %error, "the Claude catalog cache can't be listed");
            return None;
        }
    };
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().ends_with("-cc.json") {
            continue;
        }
        matched += 1;
        if matched > 256 {
            tracing::warn!(directory = %dir.display(), "too many cached Claude catalogs");
            return None;
        }
        let Some(value) = read_json_file(&entry.path()) else {
            continue;
        };
        if claude_served_models(&value).is_empty() {
            continue;
        }
        let fetched = value.get("fetchedAt").and_then(Value::as_u64).unwrap_or(0);
        if newest.as_ref().is_none_or(|(at, _)| fetched > *at) {
            newest = Some((fetched, value));
        }
    }
    newest.map(|(_, value)| value)
}

fn claude_served_models(value: &Value) -> Vec<&Value> {
    let Some(catalog) = value
        .get("catalog")
        .filter(|c| c.get("surface").and_then(Value::as_str) == Some("cc"))
    else {
        return Vec::new();
    };
    catalog
        .get("config")
        .filter(|config| config.get("id").and_then(Value::as_str) == Some("cc"))
        .and_then(|config| config.get("models"))
        .and_then(Value::as_array)
        .map(|models| models.iter().take(MAX_MODELS).collect())
        .unwrap_or_default()
}

fn claude_published_models(value: &Value) -> Vec<&Value> {
    value
        .pointer("/surfaces/cc/model_selector_config")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|config| config.get("id").and_then(Value::as_str) == Some("cc"))
        .filter_map(|config| config.get("models").and_then(Value::as_array))
        .flatten()
        .take(MAX_MODELS)
        .collect()
}

fn merge_claude(published: Option<&Value>, served: Option<&Value>) -> Listing {
    let published = published.map(claude_published_models).unwrap_or_default();
    let served = served.map(claude_served_models).unwrap_or_default();
    let published_index: HashMap<&str, &Value> = published
        .iter()
        .filter_map(|row| Some((row["id"].as_str()?, *row)))
        .collect();
    let rows: Vec<Value> = if served.is_empty() {
        published
            .iter()
            .take(MAX_MODELS)
            .map(|model| (*model).clone())
            .collect()
    } else {
        served
            .iter()
            .take(MAX_MODELS)
            .map(|model| {
                let id = model["id"].as_str();
                let mut merged = id
                    .and_then(|id| published_index.get(id))
                    .and_then(|base| base.as_object().cloned())
                    .unwrap_or_default();
                merged.extend(model.as_object().cloned().unwrap_or_default());
                Value::Object(merged)
            })
            .collect()
    };
    let mut listing = Listing::default();
    let mut seen = HashSet::new();
    for row in &rows {
        if let Some(mut model) = claude_model(row)
            && seen.insert(model.id.clone())
        {
            let account = served
                .iter()
                .find(|value| value.get("id").and_then(Value::as_str) == Some(model.id.as_str()));
            for source in &mut model.evidence.sources {
                let field = match source.field.as_str() {
                    "images" => "capabilities",
                    field => field,
                };
                source.source = if account.is_some_and(|value| value.get(field).is_some()) {
                    "claude.account_catalog_cache".into()
                } else {
                    "claude.published_catalog".into()
                };
            }
            listing.models.push(model);
        }
    }
    listing
}

fn claude_model(value: &Value) -> Option<Model> {
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())?;
    if let Some(offered) = value.get("offered_on").and_then(Value::as_array)
        && !offered
            .iter()
            .any(|surface| surface.as_str() == Some("first_party"))
    {
        return None;
    }
    let runtime = value.get("runtime");
    let thinking = value
        .get("thinking")
        .filter(|t| t.get("type").and_then(Value::as_str) != Some("none"));
    let options = thinking
        .and_then(|t| t.get("effort_options"))
        .and_then(Value::as_array);
    let efforts: Vec<String> = options
        .into_iter()
        .flatten()
        .filter_map(|o| o.get("id").and_then(Value::as_str))
        .map(str::to_owned)
        .collect();
    let badged = options.into_iter().flatten().find_map(|option| {
        matches!(
            option.pointer("/badge/message").and_then(Value::as_str),
            Some("Recommended" | "Default")
        )
        .then(|| option.get("id").and_then(Value::as_str))
        .flatten()
    });
    let default_effort = badged
        .or_else(|| {
            runtime
                .and_then(|r| r.get("default_effort"))
                .and_then(Value::as_str)
        })
        .filter(|effort| efforts.iter().any(|known| known == effort))
        .map(str::to_owned);
    let effort_notes = options
        .into_iter()
        .flatten()
        .filter_map(|option| {
            let effort = option.get("id").and_then(Value::as_str)?;
            let description = text(option.get("description"))
                .or_else(|| text(option.pointer("/tooltip/content")));
            let advisory = (option.pointer("/badge/variant").and_then(Value::as_str)
                == Some("warning"))
            .then(|| text(option.pointer("/badge/message")))
            .flatten();
            (description.is_some() || advisory.is_some()).then(|| EffortNote {
                special: false,
                effort: effort.to_owned(),
                description,
                advisory,
            })
        })
        .collect();
    Some(Model {
        evidence: evidence(
            value,
            "claude.catalog",
            &["id", "name", "thinking", "runtime", "fast_mode"],
        ),
        id: id.to_owned(),
        name: value
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(id)
            .to_owned(),
        description: text(value.get("description")),
        efforts,
        default_effort,
        effort_notes,
        context_windows: runtime
            .and_then(|r| r.get("max_input_tokens"))
            .and_then(Value::as_u64)
            .map_or_else(Vec::new, |base| {
                context_choices(EngineId::Claude, base, base)
            }),
        context_window: runtime
            .and_then(|r| r.get("max_input_tokens"))
            .and_then(Value::as_u64),
        supports_images: value
            .pointer("/capabilities/mm_images")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        speeds: claude_speeds(value.get("fast_mode")),
        variants: Vec::new(),
        default_variant: None,
        lab: None,
        group: None,
    })
}

fn claude_speeds(fast_mode: Option<&Value>) -> Vec<Speed> {
    let Some(fast) = fast_mode
        .and_then(|mode| mode.get("options"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|option| option.get("id").and_then(Value::as_str) == Some(CLAUDE_FAST))
    else {
        return Vec::new();
    };
    vec![
        Speed {
            id: STANDARD_SPEED.to_owned(),
            label: "Standard".to_owned(),
            description: None,
            default: true,
        },
        Speed {
            id: CLAUDE_FAST.to_owned(),
            label: text(fast.get("selector_label"))
                .or_else(|| text(fast.get("name")))
                .unwrap_or_else(|| "Fast".to_owned()),
            description: None,
            default: false,
        },
    ]
}

pub(crate) const CLAUDE_FAST: &str = "fast";

fn evidence(value: &Value, source: &str, fields: &[&str]) -> artisan_api::ModelEvidence {
    let observed_at = artisan_api::now();
    let mut sources: Vec<_> = fields
        .iter()
        .filter(|field| value.get(**field).is_some())
        .map(|field| artisan_api::FieldSource {
            field: (*field).into(),
            source: source.into(),
            observed_at,
        })
        .collect();
    let images = match source {
        "claude.catalog" => value.pointer("/capabilities/mm_images"),
        "opencode.model.list" => value.pointer("/capabilities/input"),
        _ => value.get("inputModalities"),
    };
    if images.is_some_and(|value| value.is_boolean() || value.is_array()) {
        sources.push(artisan_api::FieldSource {
            field: "images".into(),
            source: source.into(),
            observed_at,
        });
    }
    artisan_api::ModelEvidence { sources }
}

fn codex_evidence(value: &Value, has_window: bool) -> artisan_api::ModelEvidence {
    let mut evidence = evidence(
        value,
        "codex.model/list",
        &[
            "id",
            "displayName",
            "supportedReasoningEfforts",
            "defaultReasoningEffort",
            "serviceTiers",
            "additionalSpeedTiers",
        ],
    );
    if has_window {
        evidence.sources.push(artisan_api::FieldSource {
            field: "context_capacity".into(),
            source: "codex.models_cache".into(),
            observed_at: artisan_api::now(),
        });
    }
    evidence
}

fn text(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

async fn codex(program: &Program<'_>) -> Result<Listing, ListModelsError> {
    let rows = tokio::time::timeout(DEADLINE, async {
        let command = command(program).map_err(ListModelsError::NotInstalled)?;
        let mut session = crate::rpc::Session::codex(command)
            .await
            .map_err(ListModelsError::CodexCall)?;
        let mut rows = Vec::new();
        let mut cursor = None;
        for _ in 0..16 {
            let mut params = json!({ "includeHidden": true });
            if let Some(cursor) = cursor.take() {
                params["cursor"] = cursor;
            }
            let result = session
                .call("model/list", params)
                .await
                .map_err(ListModelsError::CodexCall)?;
            let data = result["data"]
                .as_array()
                .ok_or(ListModelsError::CodexNoData)?;
            rows.extend(data.iter().take(MAX_MODELS - rows.len()).cloned());
            if rows.len() >= MAX_MODELS {
                break;
            }
            cursor = result["nextCursor"]
                .as_str()
                .filter(|cursor| !cursor.is_empty())
                .map(|cursor| json!(cursor));
            if cursor.is_none() {
                break;
            }
        }
        Ok(rows)
    })
    .await
    .map_err(|_elapsed| ListModelsError::CodexTimedOut)??;
    let path = program.home.join(".codex/models_cache.json");
    let windows = tokio::task::spawn_blocking(move || codex_context_windows(&path))
        .await
        .map_err(ListModelsError::CodexCache)?;
    Ok(codex_listing(&rows, &windows))
}

fn codex_listing(rows: &[Value], windows: &[(String, u64, u64)]) -> Listing {
    let windows: HashMap<&str, (u64, u64)> = windows
        .iter()
        .take(MAX_MODELS)
        .map(|(id, base, max)| (id.as_str(), (*base, *max)))
        .collect();
    let mut listing = Listing::default();
    let mut seen = HashSet::new();
    for row in rows.iter().take(MAX_MODELS) {
        let Some(id) = row
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        else {
            continue;
        };
        if !seen.insert(id) {
            continue;
        }
        if row.get("hidden").and_then(Value::as_bool).unwrap_or(false) {
            continue;
        }
        if row
            .get("isDefault")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            listing.default = Some(id.to_owned());
        }
        let options = row
            .get("supportedReasoningEfforts")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        let efforts: Vec<String> = options
            .iter()
            .filter_map(|option| option.get("reasoningEffort").and_then(Value::as_str))
            .map(str::to_owned)
            .collect();
        let effort_notes = options
            .iter()
            .filter_map(|option| {
                let effort = option.get("reasoningEffort").and_then(Value::as_str)?;
                let description = text(option.get("description"));
                let special = effort == "ultra";
                (special || description.is_some()).then(|| EffortNote {
                    effort: effort.to_owned(),
                    description,
                    advisory: None,
                    special,
                })
            })
            .collect();
        listing.models.push(Model {
            evidence: codex_evidence(row, windows.contains_key(id)),
            id: id.to_owned(),
            name: row
                .get("displayName")
                .and_then(Value::as_str)
                .unwrap_or(id)
                .to_owned(),
            description: text(row.get("description")),
            default_effort: row
                .get("defaultReasoningEffort")
                .and_then(Value::as_str)
                .filter(|effort| efforts.iter().any(|known| known == effort))
                .map(str::to_owned),
            efforts,
            effort_notes,
            context_window: windows.get(id).map(|(base, _)| *base),
            context_windows: windows.get(id).map_or_else(Vec::new, |(base, max)| {
                context_choices(EngineId::Codex, *base, *max)
            }),
            supports_images: row
                .get("inputModalities")
                .and_then(Value::as_array)
                .is_some_and(|inputs| inputs.iter().any(|input| input.as_str() == Some("image"))),
            speeds: codex_speeds(row),
            variants: Vec::new(),
            default_variant: None,
            lab: None,
            group: None,
        });
    }
    listing
}

fn codex_speeds(row: &Value) -> Vec<Speed> {
    let mut tiers: Vec<Speed> = row
        .get("serviceTiers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|tier| {
            let id = text(tier.get("id"))?;
            Some(Speed {
                label: text(tier.get("name")).unwrap_or_else(|| id.clone()),
                description: text(tier.get("description")),
                default: false,
                id,
            })
        })
        .collect();
    if tiers.is_empty() {
        tiers.extend(
            row.get("additionalSpeedTiers")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter(|id| !id.trim().is_empty())
                .map(|id| Speed {
                    id: id.to_owned(),
                    label: id.to_owned(),
                    description: None,
                    default: false,
                }),
        );
    }
    let default = text(row.get("defaultServiceTier"));
    if let Some(default) = &default
        && tiers.iter().any(|tier| &tier.id == default)
    {
        for tier in &mut tiers {
            tier.default = &tier.id == default;
        }
    } else if !tiers.is_empty() {
        tiers.insert(
            0,
            Speed {
                id: STANDARD_SPEED.to_owned(),
                label: "Standard".to_owned(),
                description: None,
                default: true,
            },
        );
    }
    if tiers.len() < 2 {
        tiers.clear();
    }
    tiers
}

fn codex_context_windows(path: &Path) -> Vec<(String, u64, u64)> {
    let Some(value) = read_json_file(path) else {
        return Vec::new();
    };
    value
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(MAX_MODELS)
        .filter_map(|model| {
            Some((
                model.get("slug").and_then(Value::as_str)?.to_owned(),
                model.get("context_window").and_then(Value::as_u64)?,
                model
                    .get("max_context_window")
                    .and_then(Value::as_u64)
                    .or_else(|| model.get("context_window").and_then(Value::as_u64))?,
            ))
        })
        .collect()
}

const MODELS_DEV: &str = "https://models.dev/api.json";
const MODELS_DEV_MAX_BYTES: usize = 32 * 1024 * 1024;
const MODELS_DEV_FRESH: Duration = Duration::from_hours(12);

async fn opencode(
    program: &Program<'_>,
    http: &reqwest::Client,
) -> Result<Listing, ListModelsError> {
    let models = run(program, &["api", "--standalone", "model.list"])
        .await
        .map_err(ListModelsError::OpenCodeRun)?;
    let models: Value = serde_json::from_str(&models).map_err(ListModelsError::OpenCodeDecode)?;
    let gateways = match run(program, &["api", "--standalone", "integration.list"]).await {
        Ok(text) => match serde_json::from_str(&text) {
            Ok(gateways) => Some(gateways),
            Err(error) => {
                tracing::warn!(%error, "OpenCode listed its integrations in a shape Forge can't read");
                None
            }
        },
        Err(error) => {
            tracing::debug!(
                error = diagnostics::chain(&error),
                "OpenCode integrations unavailable"
            );
            None
        }
    };
    let digest = models_dev(http, program.home).await;
    let listing = opencode_listing(&models, gateways.as_ref(), digest.as_ref());
    if listing.models.is_empty() {
        return Err(ListModelsError::OpenCodeEmpty);
    }
    Ok(listing)
}

#[expect(
    clippy::too_many_lines,
    reason = "one pass over the listing: name, describe, credit, then order"
)]
fn opencode_listing(models: &Value, gateways: Option<&Value>, digest: Option<&Value>) -> Listing {
    let gateway_name = |provider: &str| {
        gateways
            .and_then(|gateways| gateways.get("data")?.as_array())
            .into_iter()
            .flatten()
            .find(|gateway| gateway.get("id").and_then(Value::as_str) == Some(provider))
            .and_then(|gateway| gateway.get("name")?.as_str())
            .or_else(|| digest?.pointer(&format!("/labs/{provider}"))?.as_str())
            .unwrap_or(provider)
            .to_owned()
    };
    let mut listed: Vec<(Model, u64)> = Vec::new();
    let mut seen = HashSet::new();
    for meta in models
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if listed.len() >= MAX_MODELS {
            break;
        }
        let (Some(provider), Some(model)) = (
            meta.get("providerID").and_then(Value::as_str),
            meta.get("id").and_then(Value::as_str),
        ) else {
            continue;
        };
        if provider.is_empty()
            || model.is_empty()
            || meta.get("enabled").and_then(Value::as_bool) == Some(false)
            || meta.get("status").and_then(Value::as_str) == Some("deprecated")
        {
            continue;
        }
        let id = format!("{provider}/{model}");
        if id.contains(char::is_whitespace) || !seen.insert(id.clone()) {
            continue;
        }
        let known = digest.and_then(|digest| digest.get("models")?.get(&id));
        let variants = opencode_variants(meta);
        let default_variant = meta
            .get("defaultVariant")
            .and_then(Value::as_str)
            .filter(|id| variants.iter().any(|variant| variant.id == *id))
            .map(str::to_owned);
        let context = meta.pointer("/limit/context").and_then(Value::as_u64);
        let lab = opencode_lab(meta, known, digest);
        listed.push((
            Model {
                evidence: {
                    let mut sources = evidence(
                        meta,
                        "opencode.model.list",
                        &["id", "name", "variants", "limit", "capabilities"],
                    );
                    if known.is_some() {
                        sources.sources.push(artisan_api::FieldSource {
                            field: "description_and_lab".into(),
                            source: "models.dev.digest".into(),
                            observed_at: artisan_api::now(),
                        });
                    }
                    sources
                },
                id,
                name: meta
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or(model)
                    .to_owned(),
                description: text(known.and_then(|known| known.get("description"))),
                context_windows: context.map_or_else(Vec::new, |base| {
                    context_choices(EngineId::OpenCode, base, base)
                }),
                context_window: context,
                supports_images: meta
                    .pointer("/capabilities/input")
                    .and_then(Value::as_array)
                    .is_some_and(|inputs| {
                        inputs.iter().any(|input| input.as_str() == Some("image"))
                    }),
                variants,
                default_variant,
                lab,
                group: Some(gateway_name(provider)),
                ..Model::default()
            },
            meta.pointer("/time/released")
                .and_then(Value::as_u64)
                .unwrap_or_default(),
        ));
    }
    let mut order: Vec<String> = Vec::new();
    for (model, _) in &listed {
        if let Some(group) = &model.group
            && !order.contains(group)
        {
            order.push(group.clone());
        }
    }
    let mut lab_latest: HashMap<(Option<String>, Option<String>), u64> = HashMap::new();
    for (model, released) in &listed {
        let latest = lab_latest
            .entry((model.group.clone(), model.lab.clone()))
            .or_default();
        *latest = (*latest).max(*released);
    }
    listed.sort_by_cached_key(|(model, released)| {
        let group = order
            .iter()
            .position(|group| Some(group) == model.group.as_ref());
        let lab = (model.group.clone(), model.lab.clone());
        (
            group,
            std::cmp::Reverse(lab_latest[&lab]),
            lab.1,
            std::cmp::Reverse(*released),
        )
    });
    Listing {
        models: listed.into_iter().map(|(model, _)| model).collect(),
        ..Listing::default()
    }
}

fn opencode_lab(meta: &Value, known: Option<&Value>, digest: Option<&Value>) -> Option<String> {
    meta.get("canonical")
        .and_then(Value::as_str)
        .or_else(|| known?.get("lab")?.as_str())
        .map(str::to_owned)
        .or_else(|| {
            family_lab(
                digest?,
                meta.get("family")
                    .or_else(|| known?.get("family"))?
                    .as_str()?,
            )
        })
        .map(|lab| lab_name(&lab, digest))
}

fn opencode_variants(meta: &Value) -> Vec<Variant> {
    let mut variants: Vec<Variant> = Vec::new();
    let mut seen = HashSet::new();
    for id in meta
        .get("variants")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|variant| variant.get("id")?.as_str())
        .filter(|id| !id.is_empty() && *id != "default")
    {
        if variants.len() == 128 {
            break;
        }
        if seen.insert(id) {
            variants.push(Variant {
                id: id.to_owned(),
                label: variant_label(id),
            });
        }
    }
    variants
}

fn family_lab(digest: &Value, family: &str) -> Option<String> {
    let families = digest.get("families")?;
    let mut family = family.to_ascii_lowercase();
    loop {
        if let Some(lab) = families.get(&family).and_then(Value::as_str) {
            return Some(lab.to_owned());
        }
        match family.rfind('-') {
            Some(cut) => family.truncate(cut),
            None => break,
        }
    }
    let stem: String = family
        .chars()
        .take_while(char::is_ascii_alphabetic)
        .collect();
    (stem != family && !stem.is_empty())
        .then(|| families.get(&stem)?.as_str().map(str::to_owned))
        .flatten()
}

fn lab_name(lab: &str, digest: Option<&Value>) -> String {
    let known = match lab {
        "alibaba" => "Alibaba",
        "anthropic" => "Anthropic",
        "deepseek" => "DeepSeek",
        "google" => "Google",
        "meituan" => "Meituan",
        "meta" => "Meta",
        "minimax" => "MiniMax",
        "mistral" => "Mistral",
        "moonshotai" => "Moonshot AI",
        "nvidia" => "NVIDIA",
        "openai" => "OpenAI",
        "reflection" => "Reflection AI",
        "stepfun" => "StepFun",
        "tencent" => "Tencent",
        "xai" => "xAI",
        "xiaomi" => "Xiaomi",
        "zhipuai" | "zai" => "Z.ai",
        _ => "",
    };
    if !known.is_empty() {
        return known.to_owned();
    }
    digest
        .and_then(|digest| digest.pointer(&format!("/labs/{lab}"))?.as_str())
        .map_or_else(
            || lab.to_owned(),
            |name| name.split(" (").next().unwrap_or(name).to_owned(),
        )
}

async fn models_dev(http: &reqwest::Client, home: &Path) -> Option<Value> {
    let path = home.join("cache/models-dev.json");
    let cached = {
        let path = path.clone();
        tokio::task::spawn_blocking(move || {
            let age = std::fs::metadata(&path)
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|modified| modified.elapsed().ok());
            read_json_file(&path).map(|digest| (digest, age))
        })
        .await
        .ok()
        .flatten()
    };
    if let Some((digest, Some(age))) = &cached
        && *age < MODELS_DEV_FRESH
    {
        return Some(digest.clone());
    }
    let fetched = fetch_catalog(
        http,
        MODELS_DEV,
        Duration::from_secs(10),
        MODELS_DEV_MAX_BYTES,
    )
    .await
    .map(|catalog| models_dev_digest(&catalog));
    match fetched {
        Ok(digest) => {
            let bytes =
                serde_json::to_vec(&digest).expect("invariant: a catalog digest serializes");
            tokio::task::spawn_blocking(move || {
                let written = path
                    .parent()
                    .map_or(Ok(()), std::fs::create_dir_all)
                    .and_then(|()| std::fs::write(&path, bytes));
                if let Err(error) = written {
                    tracing::debug!(%error, "models.dev digest not cached");
                }
            });
            Some(digest)
        }
        Err(error) => {
            tracing::debug!(error = diagnostics::chain(&error), "models.dev unavailable");
            cached.map(|(digest, _)| digest)
        }
    }
}

fn models_dev_digest(catalog: &Value) -> Value {
    let mut labs = serde_json::Map::new();
    let mut models = serde_json::Map::new();
    let mut credits: HashMap<String, HashMap<String, usize>> = HashMap::new();
    for (provider, entry) in catalog.as_object().into_iter().flatten() {
        if let Some(name) = entry.get("name").and_then(Value::as_str) {
            labs.insert(provider.clone(), json!(name));
        }
        for (id, model) in entry
            .get("models")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            let lab = model
                .get("canonical_model_id")
                .and_then(Value::as_str)
                .and_then(|canonical| canonical.split_once('/'))
                .map(|(lab, _)| lab.to_ascii_lowercase())
                .filter(|lab| !lab.is_empty());
            let family = model.get("family").and_then(Value::as_str);
            if let (Some(lab), Some(family)) = (&lab, family) {
                *credits
                    .entry(family.to_ascii_lowercase())
                    .or_default()
                    .entry(lab.clone())
                    .or_default() += 1;
            }
            let mut known = serde_json::Map::new();
            if let Some(description) = model.get("description").and_then(Value::as_str) {
                known.insert("description".into(), json!(description));
            }
            if let Some(lab) = lab {
                known.insert("lab".into(), json!(lab));
            }
            if let Some(family) = family {
                known.insert("family".into(), json!(family));
            }
            if !known.is_empty() {
                models.insert(format!("{provider}/{id}"), Value::Object(known));
            }
        }
    }
    let families: serde_json::Map<String, Value> = credits
        .into_iter()
        .filter_map(|(family, labs)| {
            let lab = labs
                .into_iter()
                .max_by(|(a, left), (b, right)| left.cmp(right).then_with(|| b.cmp(a)))?
                .0;
            Some((family, json!(lab)))
        })
        .collect();
    json!({ "labs": labs, "models": models, "families": families })
}

fn variant_label(id: &str) -> String {
    match id {
        "xhigh" => "Extra High".to_owned(),
        "minimal" => "Minimal".to_owned(),
        "none" => "Off".to_owned(),
        "medium" => "Medium".to_owned(),
        "high" => "High".to_owned(),
        "light" | "low" => "Light".to_owned(),
        "max" => "Max".to_owned(),
        "ultra" => "Ultra".to_owned(),
        other => {
            let mut chars = other.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(chars).collect()
            })
        }
    }
}

fn context_choices(engine: EngineId, base: u64, max: u64) -> Vec<ContextWindow> {
    if base == 0 || base > 100_000_000 {
        return Vec::new();
    }
    let mut choices = Vec::new();
    choices.push(ContextWindow {
        tokens: base,
        override_tokens: None,
    });
    if engine == EngineId::Codex && max > base && max <= 100_000_000 {
        choices.push(ContextWindow {
            tokens: max,
            override_tokens: Some(max),
        });
    }
    if engine == EngineId::Claude && base >= 1_000_000 {
        choices.push(ContextWindow {
            tokens: 500_000,
            override_tokens: Some(500_000),
        });
    }
    choices
}

#[cfg(test)]
#[path = "../tests/catalog/unit.rs"]
mod tests;
