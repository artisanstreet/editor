//! Models per engine, asked from the engine itself (or, for Claude, from
//! Anthropic's published catalog plus the CLI's cached copy). There is no
//! built-in list: the old catalog shipped none, and a stale list would offer
//! models the account can't use.

use crate::engines::{ModelSource, host_spec};
use anyhow::{Context, Result, bail, ensure};
use artisan_api::{ContextWindow, EffortNote, EngineId, Model, SlashCommand, Speed, Variant};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

const CLAUDE_CATALOG: &str = "https://downloads.claude.ai/model-catalog/v1/catalog.json";
const DEADLINE: Duration = Duration::from_secs(15);
const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
/// More than any engine lists; the rest are dropped with a warning.
const MAX_MODELS: usize = 1024;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listing {
    pub models: Vec<Model>,
    pub default: Option<String>,
    /// The `/` commands, when this engine's probe can ask for them; the
    /// rest report theirs while they run.
    pub commands: Option<Vec<SlashCommand>>,
}

/// How a probe runs the engine.
pub struct Program<'a> {
    pub binary: Option<&'a Path>,
    pub env: &'a [(String, String)],
    pub home: &'a Path,
}

pub async fn list(
    engine: EngineId,
    program: Program<'_>,
    http: &reqwest::Client,
) -> Result<Listing> {
    let listing = match host_spec(engine).models {
        ModelSource::ClaudeCatalog => {
            let (listing, commands) =
                tokio::join!(claude(http, program.home), claude_commands(&program));
            let mut listing = listing?;
            match commands {
                Ok(commands) => listing.commands = Some(commands),
                Err(error) => {
                    tracing::debug!(error = format!("{error:#}"), "Claude commands unavailable");
                }
            }
            listing
        }
        ModelSource::CodexAppServer => codex(&program).await?,
        ModelSource::OpenCodeCli => opencode(&program, http).await?,
    };
    Ok(finish(engine, listing))
}

/// What every engine's listing goes through before the picker sees it.
fn finish(engine: EngineId, mut listing: Listing) -> Listing {
    for model in &mut listing.models {
        model.name = readable_name(&model.name);
    }
    if listing.models.len() > MAX_MODELS {
        tracing::warn!(%engine, count = listing.models.len(), "engine listed too many models; keeping {MAX_MODELS}");
        listing.models.truncate(MAX_MODELS);
    }
    listing
}

fn command(program: &Program<'_>) -> Result<tokio::process::Command> {
    let binary = program.binary.context("not installed")?;
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

/// Runs a listing subcommand and returns its stdout.
async fn run(program: &Program<'_>, args: &[&str]) -> Result<String> {
    let mut command = command(program)?;
    command.args(args);
    let output = crate::process::probe(command, DEADLINE, MAX_OUTPUT_BYTES).await?;
    let (status, out, err) = (output.status, output.stdout, output.stderr);
    if !status.success() {
        let stderr = String::from_utf8_lossy(&err);
        let stdout = String::from_utf8_lossy(&out);
        let message = if stderr.trim().is_empty() {
            stdout.trim()
        } else {
            stderr.trim()
        };
        bail!("`{}` failed ({status}): {message}", args.join(" "));
    }
    Ok(String::from_utf8_lossy(&out).into_owned())
}

fn read_json_file(path: &Path) -> Option<Value> {
    use std::io::Read as _;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(MAX_OUTPUT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > MAX_OUTPUT_BYTES {
        tracing::warn!(path = %path.display(), "model cache exceeds byte budget");
        return None;
    }
    serde_json::from_slice(&bytes).ok()
}

/// Engines report hyphenated names (`GPT-6-Astra`) where the picker reads
/// `GPT 6 Astra`: separator runs collapse to one space and the ends trim.
fn readable_name(raw: &str) -> String {
    raw.split(|c: char| c == '-' || c.is_whitespace())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The tier that sends nothing: the engine's own default speed.
pub const STANDARD_SPEED: &str = "standard";

// ---------------------------------------------------------------- Claude

async fn claude(http: &reqwest::Client, home: &Path) -> Result<Listing> {
    let published = async {
        let mut response = http
            .get(CLAUDE_CATALOG)
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                bytes.len().saturating_add(chunk.len()) <= MAX_OUTPUT_BYTES,
                "Claude catalog exceeds byte budget"
            );
            bytes.extend_from_slice(&chunk);
        }
        anyhow::Ok(serde_json::from_slice::<Value>(&bytes)?)
    }
    .await;
    let published = match published {
        Ok(value) if value.get("schema_version").and_then(Value::as_u64) == Some(1) => Some(value),
        Ok(_) => None,
        Err(error) => {
            tracing::debug!(%error, "Claude model catalog unavailable");
            None
        }
    };
    let directory = home.join(".claude/cache/model-catalog");
    let served = tokio::task::spawn_blocking(move || claude_served_cache(&directory))
        .await
        .context("read Claude model cache")?;
    let listing = merge_claude(published.as_ref(), served.as_ref());
    ensure!(
        !listing.models.is_empty(),
        "neither the published nor the cached Claude catalog is available"
    );
    Ok(listing)
}

/// The commands Claude reads, from its `initialize` reply: the same list its
/// own `/` menu shows, with descriptions. No model turn runs.
async fn claude_commands(program: &Program<'_>) -> Result<Vec<SlashCommand>> {
    let mut command = command(program)?;
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
    let mut process = crate::process::spawn_probe(command)?;
    let reply = tokio::time::timeout(DEADLINE, async {
        process
            .send(&json!({
                "type": "control_request",
                "request_id": "artisan-commands",
                "request": { "subtype": "initialize" },
            }))
            .await?;
        while let Some(line) = process.next_line().await? {
            let Ok(frame) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if frame["type"] == "control_response"
                && frame["response"]["request_id"] == "artisan-commands"
            {
                return Ok(frame["response"]["response"]["commands"].clone());
            }
        }
        bail!(
            "Claude closed before answering initialize: {}",
            process.stderr_tail()
        )
    })
    .await
    .context("Claude initialize timed out")??;
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

/// The newest `*-cc.json` the CLI cached: what this account is actually served.
fn claude_served_cache(dir: &Path) -> Option<Value> {
    let mut newest: Option<(u64, Value)> = None;
    let mut matched = 0;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
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

/// The served list decides which models and in what order; the published
/// catalog fills in fields the cached copy lacks.
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
        if let Some(model) = claude_model(row)
            && seen.insert(model.id.clone())
        {
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
    // The served catalog badges the default "Recommended"; older ones said
    // "Default".
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
    // An option's tooltip says what it does; a warning badge is the caution
    // the CLI shows beside it ("3.5× or more usage").
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

/// Claude's fast mode toggle (`fast_mode.options`: the fast option and
/// `off`) as two tiers. Forge turns it on with the `fastMode` setting.
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

/// The id of Claude's fast mode option, and the speed that turns it on.
pub const CLAUDE_FAST: &str = "fast";

/// A non-empty string.
fn text(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}

// ---------------------------------------------------------------- Codex

async fn codex(program: &Program<'_>) -> Result<Listing> {
    let rows = tokio::time::timeout(DEADLINE, async {
        let mut session = crate::rpc::Session::codex(command(program)?).await?;
        let mut rows = Vec::new();
        let mut cursor = None;
        for _ in 0..16 {
            let mut params = json!({ "includeHidden": true });
            if let Some(cursor) = cursor.take() {
                params["cursor"] = cursor;
            }
            let result = session.call("model/list", params).await?;
            let data = result["data"]
                .as_array()
                .context("model/list omitted data")?;
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
        Ok::<_, anyhow::Error>(rows)
    })
    .await
    .context("codex model/list timed out")??;
    let path = program.home.join(".codex/models_cache.json");
    let windows = tokio::task::spawn_blocking(move || codex_context_windows(&path))
        .await
        .context("read Codex model cache")?;
    Ok(codex_listing(&rows, &windows))
}

/// `model/list` rows as models. Hidden rows are engine internals, never
/// offered.
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

/// The model's service tiers (`serviceTiers`: `priority` is "Fast"). Forge
/// sends the chosen tier's id as `serviceTier`; Codex drops ids the model
/// doesn't advertise. Without a catalog default the model also runs at the
/// standard tier, which sends nothing. A model whose default is one of its
/// tiers can't be asked for less, so it has nothing to choose.
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
    // Before `serviceTiers`, Codex named its one extra tier here.
    if tiers.is_empty()
        && row
            .get("additionalSpeedTiers")
            .and_then(Value::as_array)
            .is_some_and(|tiers| tiers.iter().any(|tier| tier.as_str() == Some("fast")))
    {
        tiers.push(Speed {
            id: "fast".to_owned(),
            label: "Fast".to_owned(),
            description: None,
            default: false,
        });
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

/// `model/list` has no context windows; the CLI's own cache does.
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

// ---------------------------------------------------------------- OpenCode

/// OpenCode's own catalog source, for what its API leaves out: each
/// model's description and the lab that made it.
const MODELS_DEV: &str = "https://models.dev/api.json";
const MODELS_DEV_MAX_BYTES: usize = 32 * 1024 * 1024;
/// How long the digest of it serves before it is fetched again.
const MODELS_DEV_FRESH: Duration = Duration::from_hours(12);

async fn opencode(program: &Program<'_>, http: &reqwest::Client) -> Result<Listing> {
    // Private servers: the background service sits on one fixed port
    // shared with the user's own OpenCode, so whichever starts second fails.
    let models = run(program, &["api", "--standalone", "model.list"]).await?;
    let models: Value = serde_json::from_str(&models).context("parse OpenCode model.list")?;
    let gateways = match run(program, &["api", "--standalone", "integration.list"]).await {
        Ok(text) => serde_json::from_str(&text).ok(),
        Err(error) => {
            tracing::debug!(
                error = format!("{error:#}"),
                "OpenCode integrations unavailable"
            );
            None
        }
    };
    let digest = models_dev(http, program.home).await;
    let listing = opencode_listing(&models, gateways.as_ref(), digest.as_ref());
    ensure!(!listing.models.is_empty(), "opencode listed no models");
    Ok(listing)
}

/// OpenCode's `model.list`, its models named by the gateway that serves
/// them (`integration.list`) and, from models.dev, described and credited
/// to their lab. Grouped by gateway; within one, the lab with the latest
/// release first, each lab's models newest first.
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
        let default_variant = default_variant(&variants);
        let context = meta.pointer("/limit/context").and_then(Value::as_u64);
        let lab = opencode_lab(meta, known, digest);
        listed.push((
            Model {
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
    // Stable: the gateways keep OpenCode's order among themselves.
    let mut order: Vec<String> = Vec::new();
    for (model, _) in &listed {
        if let Some(group) = &model.group
            && !order.contains(group)
        {
            order.push(group.clone());
        }
    }
    // Within a gateway, the lab with the latest release leads, and each
    // lab's models run newest first.
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

/// Who made a model: OpenCode's own `canonical` when it fills it in, else
/// models.dev's credit for this model or, failing that, for its family.
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

/// The variants OpenCode runs a model as (`model.variant`), in its order.
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

/// The variant a model runs when none is chosen: medium, else the nearest
/// level above it (an on/off model thinks), else the highest below it.
fn default_variant(variants: &[Variant]) -> Option<String> {
    const LEVELS: [&str; 10] = [
        "none", "minimal", "light", "low", "medium", "thinking", "high", "xhigh", "max", "ultra",
    ];
    const MEDIUM: usize = 4;
    let ranked = || {
        variants.iter().filter_map(|variant| {
            Some((
                LEVELS.iter().position(|level| *level == variant.id)?,
                &variant.id,
            ))
        })
    };
    ranked()
        .filter(|(rank, _)| *rank >= MEDIUM)
        .min_by_key(|(rank, _)| *rank)
        .or_else(|| ranked().max_by_key(|(rank, _)| *rank))
        .map(|(_, id)| id.clone())
        .or_else(|| variants.first().map(|variant| variant.id.clone()))
}

/// The lab of a family models.dev doesn't credit directly, from the
/// family's other models, trimming it until one matches
/// (`gpt-codex-spark` → `gpt`, `qwen3.7-plus` → `qwen`).
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

/// A lab as people name it; models.dev's provider names are its hosts'
/// ("MiniMax (minimax.io)") and miss labs that host nothing.
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

/// models.dev's digest: a fresh cached copy, else a new fetch, else a
/// stale copy. Absent offline on first use; the listing stands without it.
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
    let fetched = async {
        let mut response = http
            .get(MODELS_DEV)
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                bytes.len().saturating_add(chunk.len()) <= MODELS_DEV_MAX_BYTES,
                "models.dev catalog exceeds byte budget"
            );
            bytes.extend_from_slice(&chunk);
        }
        anyhow::Ok(models_dev_digest(&serde_json::from_slice(&bytes)?))
    }
    .await;
    match fetched {
        Ok(digest) => {
            let bytes = serde_json::to_vec(&digest).unwrap_or_default();
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
            tracing::debug!(error = format!("{error:#}"), "models.dev unavailable");
            cached.map(|(digest, _)| digest)
        }
    }
}

/// What the listing needs of models.dev's catalog: per `provider/model`
/// its description, lab and family; each family's most credited lab; and
/// the providers' names.
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

/// A variant as the picker names it (the old Editor's `humanize_variant`).
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

/// Choices come from reported model limits, never a model-name guess.
fn context_choices(engine: EngineId, base: u64, max: u64) -> Vec<ContextWindow> {
    if base == 0 || base > 100_000_000 {
        return Vec::new();
    }
    let mut choices = Vec::new();
    if engine == EngineId::Claude && base >= 1_000_000 {
        choices.push(ContextWindow {
            tokens: 500_000,
            override_tokens: Some(500_000),
        });
    }
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
    choices
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_choices_follow_reported_limits_and_default_without_override() {
        let listing = codex_listing(
            &[json!({"id":"gpt-test"})],
            &[("gpt-test".to_owned(), 272_000, 1_000_000)],
        );
        assert_eq!(listing.models[0].context_window, Some(272_000));
        assert_eq!(
            listing.models[0].context_windows,
            [
                ContextWindow {
                    tokens: 272_000,
                    override_tokens: None
                },
                ContextWindow {
                    tokens: 1_000_000,
                    override_tokens: Some(1_000_000)
                },
            ]
        );
        assert!(
            codex_listing(&[json!({"id":"unknown"})], &[]).models[0]
                .context_windows
                .is_empty()
        );
        let claude =
            claude_model(&json!({"id":"claude-test", "runtime":{"max_input_tokens":1_000_000}}))
                .unwrap();
        assert_eq!(
            claude.context_windows,
            [
                ContextWindow {
                    tokens: 500_000,
                    override_tokens: Some(500_000)
                },
                ContextWindow {
                    tokens: 1_000_000,
                    override_tokens: None
                },
            ]
        );
        let small =
            claude_model(&json!({"id":"claude-small", "runtime":{"max_input_tokens":200_000}}))
                .unwrap();
        assert_eq!(
            small.context_windows,
            [ContextWindow {
                tokens: 200_000,
                override_tokens: None
            }]
        );
    }

    #[test]
    fn codex_cache_retains_base_and_extended_window() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("models_cache.json");
        std::fs::write(
            &path,
            json!({"models":[
                {"slug":"extended", "context_window":272_000, "max_context_window":1_000_000},
                {"slug":"standard", "context_window":400_000}
            ]})
            .to_string(),
        )
        .unwrap();
        assert_eq!(
            codex_context_windows(&path),
            [
                ("extended".to_owned(), 272_000, 1_000_000),
                ("standard".to_owned(), 400_000, 400_000),
            ]
        );
    }

    #[test]
    fn codex_catalog_has_unique_nonempty_model_identities() {
        let listing = codex_listing(
            &[
                json!({"id":"m1"}),
                json!({"id":"m1"}),
                json!({"id":""}),
                json!({"id":"m2"}),
            ],
            &[],
        );
        assert_eq!(
            listing
                .models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            ["m1", "m2"]
        );
    }

    #[test]
    fn oversized_claude_lists_are_capped_before_collecting_and_joining() {
        let models: Vec<_> = (0..MAX_MODELS * 2)
            .map(|index| json!({"id":format!("model-{index}")}))
            .collect();
        let published =
            json!({"surfaces":{"cc":{"model_selector_config":[{"id":"cc","models":models}]}}});
        let served = json!({"catalog":{"surface":"cc","config":{"id":"cc","models":models}}});
        assert_eq!(claude_published_models(&published).len(), MAX_MODELS);
        assert_eq!(claude_served_models(&served).len(), MAX_MODELS);
        let listing = merge_claude(Some(&published), Some(&served));
        assert_eq!(listing.models.len(), MAX_MODELS);
        assert_eq!(
            listing.models.last().unwrap().id,
            format!("model-{}", MAX_MODELS - 1)
        );
    }

    #[test]
    fn oversized_cached_catalogs_are_not_loaded() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("catalog.json");
        std::fs::write(&path, "x".repeat(MAX_OUTPUT_BYTES + 1)).unwrap();
        assert!(read_json_file(&path).is_none());
    }

    fn recording(path: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("transcripts")
            .join(path);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()))
    }

    fn model<'a>(listing: &'a Listing, id: &str) -> &'a Model {
        listing
            .models
            .iter()
            .find(|model| model.id == id)
            .unwrap_or_else(|| panic!("{id} is listed"))
    }

    fn speeds(model: &Model) -> Vec<(&str, &str, Option<&str>, bool)> {
        model
            .speeds
            .iter()
            .map(|speed| {
                (
                    speed.id.as_str(),
                    speed.label.as_str(),
                    speed.description.as_deref(),
                    speed.default,
                )
            })
            .collect()
    }

    /// Codex 0.162.1's `model/list`, recorded.
    #[test]
    fn codex_offers_its_fast_tier_and_says_what_each_effort_does() {
        let recording = recording("codex/model-list.jsonl");
        let rows: Vec<Value> = recording
            .lines()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|message| message.get("id").and_then(Value::as_u64) == Some(1))
            .filter_map(|message| message.pointer("/result/data").cloned())
            .filter_map(|data| data.as_array().cloned())
            .flatten()
            .collect();
        let listing = finish(EngineId::Codex, codex_listing(&rows, &[]));
        let sol = model(&listing, "gpt-6.1-sol");
        assert_eq!(sol.name, "GPT 6.1 Sol", "the picker reads names, not ids");
        assert_eq!(
            sol.description.as_deref(),
            Some("Latest workhorse model for coding and everyday work.")
        );
        assert_eq!(
            speeds(sol),
            [
                ("standard", "Standard", None, true),
                ("priority", "Fast", Some("2x speed, increased usage"), false),
            ],
            "Fast sends the tier id Codex advertises"
        );
        let note = |effort: &str| {
            sol.effort_notes
                .iter()
                .find(|note| note.effort == effort)
                .and_then(|note| note.description.clone())
        };
        assert_eq!(
            note("ultra").as_deref(),
            Some("Maximum reasoning with automatic task delegation")
        );
        assert_eq!(sol.effort_notes.len(), sol.efforts.len());
        // Already fast by default: there is no slower tier to ask for.
        assert!(model(&listing, "gpt-6-sol").speeds.is_empty());
        assert!(
            !listing
                .models
                .iter()
                .any(|model| model.id == "codex-auto-review"),
            "hidden rows stay hidden"
        );
    }

    /// The served catalog Claude Code 2.1.296 cached for a signed-in account.
    #[test]
    fn claude_offers_fast_mode_and_warns_about_max_effort() {
        let served: Value =
            serde_json::from_str(&recording("claude/model-catalog-cc.json")).unwrap();
        let listing = merge_claude(None, Some(&served));
        let opus = model(&listing, "claude-opus-5-5");
        assert_eq!(
            opus.description.as_deref(),
            Some("For complex work and everyday tasks")
        );
        assert_eq!(
            opus.default_effort.as_deref(),
            Some("medium"),
            "the picker starts at the effort the catalog recommends"
        );
        assert_eq!(
            model(&listing, "claude-opus-4-7").default_effort.as_deref(),
            Some("xhigh")
        );
        assert_eq!(
            speeds(opus),
            [
                ("standard", "Standard", None, true),
                ("fast", "Fast", None, false)
            ]
        );
        let max = opus
            .effort_notes
            .iter()
            .find(|note| note.effort == "max")
            .expect("max says what it costs");
        assert_eq!(max.advisory.as_deref(), Some("3.5\u{d7} or more usage"));
        assert_eq!(
            max.description.as_deref(),
            Some(
                "Claude thinks longer with Max effort and can consume 3.5\u{d7} or more usage than Medium."
            )
        );
        assert!(
            opus.effort_notes.iter().all(|note| note.effort == "max"),
            "a Recommended badge is no warning: {:?}",
            opus.effort_notes
        );
        assert!(
            model(&listing, "claude-sonnet-5-5").speeds.is_empty(),
            "fast mode only where the account is served it"
        );
    }

    /// OpenCode 2's `model.list`: variants are what it runs, the gateway
    /// names the group, and models.dev describes the model and its lab.
    #[test]
    fn opencode_models_are_grouped_by_gateway_and_credited_to_their_lab() {
        let models = serde_json::json!({ "data": [
            { "id": "glm-5.3", "providerID": "opencode-go", "family": "glm", "name": "GLM-5.3",
              "enabled": true, "status": "active", "time": { "released": 2 },
              "capabilities": { "input": ["text"] }, "limit": { "context": 1_000_000, "output": 131_072 },
              "variants": [{ "id": "low" }, { "id": "max" }] },
            { "id": "claude-opus-5-5", "providerID": "opencode", "family": "claude-opus",
              "name": "Claude Opus 5.5", "enabled": true, "status": "active", "time": { "released": 1 },
              "capabilities": { "input": ["text", "image"] }, "limit": { "context": 1_000_000, "output": 128_000 },
              "variants": [{ "id": "none" }, { "id": "high" }] },
            { "id": "gpt-5.3-codex-spark", "providerID": "opencode", "family": "gpt-codex-spark",
              "name": "GPT-5.3-Codex-Spark", "enabled": true, "status": "active", "time": { "released": 3 },
              "capabilities": { "input": ["text"] }, "limit": { "context": 400_000, "output": 128_000 },
              "variants": [] },
            { "id": "claude-sonnet-5-5", "providerID": "opencode", "family": "claude-sonnet",
              "name": "Claude Sonnet 5.5", "enabled": true, "status": "active", "time": { "released": 4 },
              "capabilities": { "input": ["text"] }, "limit": { "context": 1_000_000, "output": 64_000 },
              "variants": [] },
            { "id": "big-pickle", "providerID": "opencode", "name": "Big Pickle", "enabled": true,
              "status": "active", "time": { "released": 0 }, "capabilities": { "input": ["text"] },
              "limit": { "context": 200_000, "output": 32_000 }, "variants": [] },
            { "id": "gpt-4", "providerID": "opencode", "name": "GPT-4", "enabled": true,
              "status": "deprecated", "time": { "released": 0 }, "capabilities": { "input": ["text"] },
              "limit": { "context": 8_000, "output": 4_000 }, "variants": [] }
        ] });
        let gateways = serde_json::json!({ "data": [
            { "id": "opencode", "name": "OpenCode Console" },
            { "id": "opencode-go", "name": "OpenCode Go" }
        ] });
        let digest = models_dev_digest(&serde_json::json!({
            "opencode": { "name": "OpenCode", "models": {
                "claude-opus-5-5": { "description": "Claude for long-running work.",
                    "family": "claude-opus", "canonical_model_id": "anthropic/claude-opus-5-5" },
                "claude-sonnet-5-5": { "family": "claude-sonnet",
                    "canonical_model_id": "anthropic/claude-sonnet-5-5" },
                "gpt-5.5": { "family": "gpt", "canonical_model_id": "openai/gpt-5.5" }
            } },
            "opencode-go": { "name": "OpenCode Go", "models": {
                "glm-5.3": { "description": "Flagship GLM.", "family": "glm",
                    "canonical_model_id": "zhipuai/glm-5.3" }
            } }
        }));
        let listing = opencode_listing(&models, Some(&gateways), Some(&digest));
        let ids: Vec<_> = listing
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect();
        assert_eq!(
            ids,
            [
                "opencode-go/glm-5.3",
                "opencode/claude-sonnet-5-5",
                "opencode/claude-opus-5-5",
                "opencode/gpt-5.3-codex-spark",
                "opencode/big-pickle",
            ],
            "gateways in OpenCode's order, the lab with the latest release first, \
             each lab newest first, deprecated left out"
        );
        let glm = model(&listing, "opencode-go/glm-5.3");
        assert_eq!(glm.group.as_deref(), Some("OpenCode Go"));
        assert_eq!(glm.lab.as_deref(), Some("Z.ai"));
        assert_eq!(glm.description.as_deref(), Some("Flagship GLM."));
        let opus = model(&listing, "opencode/claude-opus-5-5");
        let variants: Vec<_> = opus
            .variants
            .iter()
            .map(|variant| (variant.id.as_str(), variant.label.as_str()))
            .collect();
        assert_eq!(variants, [("none", "Off"), ("high", "High")]);
        assert!(opus.efforts.is_empty(), "variants are not efforts");
        assert!(opus.supports_images);
        assert_eq!(opus.lab.as_deref(), Some("Anthropic"));
        assert_eq!(opus.group.as_deref(), Some("OpenCode Console"));
        let spark = model(&listing, "opencode/gpt-5.3-codex-spark");
        assert_eq!(spark.lab.as_deref(), Some("OpenAI"), "trimmed family");
        assert_eq!(model(&listing, "opencode/big-pickle").lab, None);
    }

    /// No OpenCode model offers a "Default": one without a chosen variant
    /// runs at medium, or the nearest level above it, so an on/off model
    /// thinks. Sets as OpenCode 2.1.0 lists them.
    #[test]
    fn opencode_models_default_to_medium_or_the_nearest_level_above() {
        let default = |ids: &[&str]| {
            let variants: Vec<Variant> = ids
                .iter()
                .map(|id| Variant {
                    id: (*id).to_owned(),
                    label: variant_label(id),
                })
                .collect();
            default_variant(&variants)
        };
        assert_eq!(
            default(&["low", "medium", "high", "xhigh", "max"]).as_deref(),
            Some("medium")
        );
        assert_eq!(
            default(&["none", "low", "medium", "high"]).as_deref(),
            Some("medium")
        );
        assert_eq!(default(&["low", "high", "max"]).as_deref(), Some("high"));
        assert_eq!(default(&["none", "thinking"]).as_deref(), Some("thinking"));
        assert_eq!(default(&["high", "max"]).as_deref(), Some("high"));
        assert_eq!(default(&["max"]).as_deref(), Some("max"));
        assert_eq!(default(&["none", "minimal", "low"]).as_deref(), Some("low"));
        assert_eq!(default(&[]), None);
    }
}
