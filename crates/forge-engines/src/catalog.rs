//! Models per engine, asked from the engine itself (or, for Claude, from
//! Anthropic's published catalog plus the CLI's cached copy). There is no
//! built-in list: the old catalog shipped none, and a stale list would offer
//! models the account can't use.

use crate::engines::{ModelSource, host_spec};
use anyhow::{Context, Result, bail, ensure};
use artisan_api::{EffortNote, EngineId, Model, Speed, Variant};
use serde_json::{Value, json};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

const CLAUDE_CATALOG: &str = "https://downloads.claude.ai/model-catalog/v1/catalog.json";
const DEADLINE: Duration = Duration::from_secs(15);
const MAX_OUTPUT_BYTES: usize = 4 * 1024 * 1024;
/// More than any engine lists; the rest are dropped with a warning.
const MAX_MODELS: usize = 1024;

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Listing {
    pub models: Vec<Model>,
    pub default: Option<String>,
}

/// How a probe runs the engine.
pub(crate) struct Program<'a> {
    pub binary: Option<&'a Path>,
    pub env: &'a [(String, String)],
    pub home: &'a Path,
}

pub(crate) async fn list(
    engine: EngineId,
    program: Program<'_>,
    http: &reqwest::Client,
) -> Result<Listing> {
    let listing = match host_spec(engine).models {
        ModelSource::ClaudeCatalog => claude(http, program.home).await?,
        ModelSource::CodexAppServer => codex(&program).await?,
        ModelSource::OpenCodeCli => opencode(&program).await?,
        ModelSource::CliLines(args) => parse_lines(&run(&program, args).await?),
    };
    Ok(finish(engine, listing))
}

/// What every engine's listing goes through before the picker sees it.
fn finish(engine: EngineId, mut listing: Listing) -> Listing {
    if engine == EngineId::Cursor {
        fold_fast_ids(&mut listing);
    }
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
    let mut child = command(program)?
        .args(args)
        .spawn()
        .context("starting the engine")?;
    let (mut stdout, mut stderr) = (
        child.stdout.take().context("stdout")?,
        child.stderr.take().context("stderr")?,
    );
    let output = tokio::time::timeout(DEADLINE, async {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let (mut stdout, mut stderr) = (
            (&mut stdout).take(MAX_OUTPUT_BYTES as u64),
            (&mut stderr).take(MAX_OUTPUT_BYTES as u64),
        );
        let (read_out, read_err) =
            tokio::join!(stdout.read_to_end(&mut out), stderr.read_to_end(&mut err));
        read_out?;
        read_err?;
        Ok::<_, std::io::Error>((child.wait().await?, out, err))
    })
    .await
    .with_context(|| {
        format!(
            "`{}` did not finish in {}s",
            args.join(" "),
            DEADLINE.as_secs()
        )
    })?
    .context("reading the engine output")?;
    let (status, out, err) = output;
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

// ---------------------------------------------------------------- one id per line (Grok, Cursor)

/// `[*|-] <id> [label] [(default)]` lines, after a `…models:` header when
/// there is one. `*` or `(default)` marks the default.
pub(crate) fn parse_lines(output: &str) -> Listing {
    let lines: Vec<&str> = output.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.trim().to_ascii_lowercase().ends_with("models:"))
        .map_or(0, |header| header + 1);
    let mut listing = Listing::default();
    for line in &lines[start..] {
        let marked =
            line.starts_with(char::is_whitespace) || line.starts_with('*') || line.starts_with('-');
        let trimmed = line.trim();
        let mut default = trimmed.starts_with('*');
        let rest = trimmed
            .strip_prefix(['*', '-'])
            .map_or(trimmed, str::trim_start);
        let mut parts = rest.split_whitespace();
        let Some(id) = parts.next() else { continue };
        let valid = id.len() <= 200
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._:-/[]".contains(c));
        let signal = id.chars().any(|c| c.is_ascii_digit() || "-._".contains(c));
        // Unindented prose ("not authenticated") is not a model.
        if !valid || !(marked || signal) {
            continue;
        }
        let label = parts
            .filter(|part| {
                let marker = part.eq_ignore_ascii_case("(default)");
                default |= marker;
                !marker
            })
            .collect::<Vec<_>>()
            .join(" ");
        if default && listing.default.is_none() {
            listing.default = Some(id.to_owned());
        }
        listing.models.push(Model {
            id: id.to_owned(),
            name: if label.is_empty() {
                title_case(id)
            } else {
                label
            },
            ..Model::default()
        });
    }
    listing
}

/// The `-fast` speed of a model Cursor lists under its own id
/// (`composer-2.5` and `composer-2.5-fast`): one model with a Fast tier, run
/// as the `-fast` id (`acp::cursor_model`). A `-fast` id without its base
/// stays a model of its own.
pub(crate) fn fold_fast_ids(listing: &mut Listing) {
    let ids: Vec<String> = listing
        .models
        .iter()
        .map(|model| model.id.clone())
        .collect();
    let mut folded = Vec::new();
    for fast in &ids {
        let Some(base) = fast.strip_suffix("-fast") else {
            continue;
        };
        if !ids.iter().any(|id| id == base) {
            continue;
        }
        let fast_default = listing.default.as_deref() == Some(fast.as_str());
        if let Some(model) = listing.models.iter_mut().find(|model| model.id == base) {
            model.speeds = vec![
                Speed {
                    id: STANDARD_SPEED.to_owned(),
                    label: "Standard".to_owned(),
                    description: None,
                    default: !fast_default,
                },
                Speed {
                    id: "fast".to_owned(),
                    label: "Fast".to_owned(),
                    description: None,
                    default: fast_default,
                },
            ];
        }
        if fast_default {
            listing.default = Some(base.to_owned());
        }
        folded.push(fast.clone());
    }
    listing.models.retain(|model| !folded.contains(&model.id));
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
pub(crate) const STANDARD_SPEED: &str = "standard";

fn title_case(id: &str) -> String {
    id.split('-')
        .map(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect::<String>())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// ---------------------------------------------------------------- Claude

async fn claude(http: &reqwest::Client, home: &Path) -> Result<Listing> {
    let published = async {
        let response = http
            .get(CLAUDE_CATALOG)
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)?;
        let text = response.text().await?;
        anyhow::Ok(serde_json::from_str::<Value>(&text)?)
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
    let served = claude_served_cache(&home.join(".claude/cache/model-catalog"));
    let listing = merge_claude(published.as_ref(), served.as_ref());
    ensure!(
        !listing.models.is_empty(),
        "neither the published nor the cached Claude catalog is available"
    );
    Ok(listing)
}

/// The newest `*-cc.json` the CLI cached: what this account is actually served.
fn claude_served_cache(dir: &Path) -> Option<Value> {
    let mut newest: Option<(u64, Value)> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        if !entry.file_name().to_string_lossy().ends_with("-cc.json") {
            continue;
        }
        let Ok(bytes) = std::fs::read(entry.path()) else {
            continue;
        };
        let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
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
        .map(|models| models.iter().collect())
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
        .collect()
}

/// The served list decides which models and in what order; the published
/// catalog fills in fields the cached copy lacks.
fn merge_claude(published: Option<&Value>, served: Option<&Value>) -> Listing {
    let published = published.map(claude_published_models).unwrap_or_default();
    let served = served.map(claude_served_models).unwrap_or_default();
    let rows: Vec<Value> = if served.is_empty() {
        published.iter().map(|model| (*model).clone()).collect()
    } else {
        served
            .iter()
            .map(|model| {
                let id = model.get("id");
                let mut merged = published
                    .iter()
                    .find(|base| id.is_some() && base.get("id") == id)
                    .and_then(|base| base.as_object().cloned())
                    .unwrap_or_default();
                merged.extend(model.as_object().cloned().unwrap_or_default());
                Value::Object(merged)
            })
            .collect()
    };
    let mut listing = Listing::default();
    for row in &rows {
        if let Some(model) = claude_model(row)
            && !listing
                .models
                .iter()
                .any(|existing| existing.id == model.id)
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
        context_window: runtime
            .and_then(|r| r.get("max_input_tokens"))
            .and_then(Value::as_u64),
        supports_images: value
            .pointer("/capabilities/mm_images")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        speeds: claude_speeds(value.get("fast_mode")),
        variants: Vec::new(),
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
pub(crate) const CLAUDE_FAST: &str = "fast";

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
    let mut child = command(program)?
        .args(["app-server", "--stdio"])
        .stdin(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().context("stdin")?;
    let mut lines = BufReader::new(child.stdout.take().context("stdout")?).lines();
    let rows = tokio::time::timeout(DEADLINE, async {
        send(&mut stdin, json!({
            "id": 0, "method": "initialize",
            "params": { "clientInfo": { "name": "artisan", "version": env!("CARGO_PKG_VERSION") },
                        "capabilities": { "experimentalApi": false } },
        }))
        .await?;
        let mut rows = Vec::new();
        let mut page = 0_u64;
        loop {
            let Some(line) = lines.next_line().await? else {
                bail!("codex app-server closed its output")
            };
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let Some(id) = message.get("id").and_then(Value::as_u64) else {
                continue;
            };
            if let Some(error) = message.get("error") {
                bail!("codex app-server: {error}");
            }
            let cursor = if id == 0 {
                send(&mut stdin, json!({ "method": "initialized" })).await?;
                None
            } else {
                let result = message
                    .get("result")
                    .context("model/list without a result")?;
                rows.extend(
                    result
                        .get("data")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default(),
                );
                match result
                    .get("nextCursor")
                    .and_then(Value::as_str)
                    .filter(|c| !c.is_empty())
                {
                    Some(cursor) if rows.len() < MAX_MODELS && page < 16 => Some(cursor.to_owned()),
                    _ => return Ok(rows),
                }
            };
            page += 1;
            let mut params = json!({ "includeHidden": true });
            if let Some(cursor) = cursor {
                params["cursor"] = json!(cursor);
            }
            send(
                &mut stdin,
                json!({ "id": page, "method": "model/list", "params": params }),
            )
            .await?;
        }
    })
    .await
    .context("codex model/list timed out")?;
    let _ = child.kill().await;
    let rows = rows?;
    let windows = codex_context_windows(&program.home.join(".codex/models_cache.json"));
    Ok(codex_listing(&rows, &windows))
}

/// `model/list` rows as models. Hidden rows are engine internals, never
/// offered.
fn codex_listing(rows: &[Value], windows: &[(String, u64)]) -> Listing {
    let mut listing = Listing::default();
    for row in rows {
        let Some(id) = row.get("id").and_then(Value::as_str) else {
            continue;
        };
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
                Some(EffortNote {
                    effort: option
                        .get("reasoningEffort")
                        .and_then(Value::as_str)?
                        .to_owned(),
                    description: Some(text(option.get("description"))?),
                    advisory: None,
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
            context_window: windows
                .iter()
                .find(|(slug, _)| slug == id)
                .map(|(_, tokens)| *tokens),
            supports_images: row
                .get("inputModalities")
                .and_then(Value::as_array)
                .is_some_and(|inputs| inputs.iter().any(|input| input.as_str() == Some("image"))),
            speeds: codex_speeds(row),
            variants: Vec::new(),
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

async fn send(stdin: &mut tokio::process::ChildStdin, value: Value) -> Result<()> {
    stdin.write_all(format!("{value}\n").as_bytes()).await?;
    stdin.flush().await?;
    Ok(())
}

/// `model/list` has no context windows; the CLI's own cache does.
fn codex_context_windows(path: &Path) -> Vec<(String, u64)> {
    let Ok(bytes) = std::fs::read(path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return Vec::new();
    };
    value
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|model| {
            Some((
                model.get("slug").and_then(Value::as_str)?.to_owned(),
                model.get("context_window").and_then(Value::as_u64)?,
            ))
        })
        .collect()
}

// ---------------------------------------------------------------- OpenCode

async fn opencode(program: &Program<'_>) -> Result<Listing> {
    let output = run(program, &["models"]).await?;
    let cache = [".cache", "cache"]
        .iter()
        .map(|dir| program.home.join(dir).join("opencode/models.json"))
        .find_map(|path| serde_json::from_slice::<Value>(&std::fs::read(path).ok()?).ok());
    let listing = opencode_listing(&output, cache.as_ref());
    ensure!(!listing.models.is_empty(), "opencode listed no models");
    Ok(listing)
}

/// `opencode2 models` lines (`provider/model`), named and described from
/// OpenCode's own `models.json` cache. A model's effort `reasoning_options`
/// are the variants OpenCode runs it as (`model.variant`).
fn opencode_listing(output: &str, cache: Option<&Value>) -> Listing {
    let mut listing = Listing::default();
    for line in output.lines().map(str::trim) {
        let Some((provider, model)) = line.split_once('/') else {
            continue;
        };
        if provider.is_empty() || model.is_empty() || line.contains(char::is_whitespace) {
            continue;
        }
        if listing.models.iter().any(|existing| existing.id == line) {
            continue;
        }
        let meta = cache.and_then(|cache| cache.get(provider)?.get("models")?.get(model));
        let mut variants: Vec<Variant> = Vec::new();
        for id in meta
            .and_then(|meta| meta.get("reasoning_options"))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|option| option.get("type").and_then(Value::as_str) == Some("effort"))
            .filter_map(|option| option.get("values").and_then(Value::as_array))
            .flatten()
            .filter_map(Value::as_str)
            .filter(|id| !id.is_empty() && *id != "default")
        {
            if !variants.iter().any(|variant| variant.id == id) {
                variants.push(Variant {
                    id: id.to_owned(),
                    label: variant_label(id),
                });
            }
        }
        listing.models.push(Model {
            id: line.to_owned(),
            name: meta
                .and_then(|m| m.get("name"))
                .and_then(Value::as_str)
                .unwrap_or(line)
                .to_owned(),
            description: text(meta.and_then(|m| m.get("description"))),
            context_window: meta
                .and_then(|m| m.pointer("/limit/context"))
                .and_then(Value::as_u64),
            supports_images: meta
                .and_then(|m| m.pointer("/modalities/input"))
                .and_then(Value::as_array)
                .is_some_and(|inputs| inputs.iter().any(|input| input.as_str() == Some("image"))),
            variants,
            ..Model::default()
        });
    }
    listing
}

/// A variant as the picker names it (the old Editor's `humanize_variant`).
fn variant_label(id: &str) -> String {
    match id {
        "xhigh" => "Extra High".to_owned(),
        "minimal" => "Minimal".to_owned(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grok_listing_marks_the_default_and_skips_prose() {
        let listing = parse_lines(
            "You are not authenticated.\n\nDefault model: grok-4.6\n\nAvailable models:\n  * grok-4.6 (default)\n  - grok-4.5\n",
        );
        let ids: Vec<_> = listing
            .models
            .iter()
            .map(|model| (model.id.as_str(), model.name.as_str()))
            .collect();
        assert_eq!(ids, [("grok-4.6", "Grok 4.6"), ("grok-4.5", "Grok 4.5")]);
        assert_eq!(listing.default.as_deref(), Some("grok-4.6"));

        let cursor = parse_lines(
            "* composer-2.5-fast Fast Composer\n  gpt-5.6-sol GPT 5.6 Sol\n\nnot a model id line\n",
        );
        let ids: Vec<_> = cursor
            .models
            .iter()
            .map(|model| (model.id.as_str(), model.name.as_str()))
            .collect();
        assert_eq!(
            ids,
            [
                ("composer-2.5-fast", "Fast Composer"),
                ("gpt-5.6-sol", "GPT 5.6 Sol")
            ]
        );
        assert_eq!(cursor.default.as_deref(), Some("composer-2.5-fast"));
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

    /// OpenCode's own `models.json` (as the old Forge's fixtures had it):
    /// effort reasoning options are the variants OpenCode runs.
    #[test]
    fn opencode_reasoning_options_become_variants() {
        let cache = serde_json::json!({ "anthropic": { "models": { "claude-opus-5": {
            "name": "Claude Opus 5",
            "description": "Anthropic's most capable model.",
            "limit": { "context": 1_000_000, "output": 128_000 },
            "modalities": { "input": ["text", "image"] },
            "reasoning_options": [
                { "type": "effort", "values": ["high", "max"] },
                { "type": "toggle" }
            ]
        } } } });
        let listing = opencode_listing(
            "anthropic/claude-opus-5\nopencode/big-pickle\n",
            Some(&cache),
        );
        let opus = model(&listing, "anthropic/claude-opus-5");
        let variants: Vec<_> = opus
            .variants
            .iter()
            .map(|variant| (variant.id.as_str(), variant.label.as_str()))
            .collect();
        assert_eq!(variants, [("high", "High"), ("max", "Max")]);
        assert!(opus.efforts.is_empty(), "variants are not efforts");
        assert_eq!(
            opus.description.as_deref(),
            Some("Anthropic's most capable model.")
        );
        assert!(model(&listing, "opencode/big-pickle").variants.is_empty());
    }

    #[test]
    fn cursor_fast_ids_become_the_fast_tier_of_their_model() {
        let mut listing = parse_lines(
            "* composer-2.5 Composer 2.5\n  composer-2.5-fast Composer 2.5 Fast\n  grok-code-fast Grok Code Fast\n",
        );
        fold_fast_ids(&mut listing);
        let ids: Vec<_> = listing
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect();
        assert_eq!(ids, ["composer-2.5", "grok-code-fast"]);
        assert_eq!(
            speeds(&listing.models[0]),
            [
                ("standard", "Standard", None, true),
                ("fast", "Fast", None, false)
            ]
        );
        assert!(listing.models[1].speeds.is_empty());
    }
}
