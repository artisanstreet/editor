use crate::account::engine_command;
use artisan_api::{
    EngineId, SpendLine, Timestamp, UsageCadence, UsageLimits, UsageSpend, UsageWindow,
};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{
    Mutex, PoisonError,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;
use tokio::time::Instant;

const CLAUDE_TIMEOUT: Duration = Duration::from_secs(45);
const RETRY_AFTER: Duration = Duration::from_secs(60);
const CODEX_TIMEOUT: Duration = Duration::from_secs(15);
const OPENCODE_TIMEOUT: Duration = Duration::from_secs(30);
const OPENCODE_GO_USAGE: &str = "https://opencode.ai/zen/go/v1/usage";

#[derive(Debug, thiserror::Error)]
pub(crate) enum ReadUsageError {
    #[error("read Claude's usage")]
    Claude(#[source] ReadClaudeUsageError),
    #[error("read Codex's rate limits")]
    Codex(#[source] ReadCodexUsageError),
    #[error("read OpenCode's usage")]
    OpenCode(#[source] ReadOpenCodeUsageError),
}

async fn read_usage(engine: EngineId, binary: &Path, home: &Path) -> Option<UsageLimits> {
    let read = match engine {
        EngineId::Claude => read_claude(binary, home)
            .await
            .map(|windows| (windows, None))
            .map_err(ReadUsageError::Claude),
        EngineId::Codex => read_codex(binary, home)
            .await
            .map(|windows| (windows, None))
            .map_err(ReadUsageError::Codex),
        EngineId::OpenCode => read_opencode(binary, home)
            .await
            .map_err(ReadUsageError::OpenCode),
        EngineId::Antigravity => return None,
    };
    match read {
        Ok((windows, spend)) if !windows.is_empty() || spend.is_some() => Some(UsageLimits {
            windows,
            spend: spend.map(Box::new),
            read_at: artisan_api::now(),
        }),
        Ok(_) => None,
        Err(error) => {
            tracing::info!(engine = %engine, home = %home.display(), error = diagnostics::chain(&error), "usage unavailable");
            None
        }
    }
}

#[derive(Debug)]
pub struct UsageCache {
    ttl: Duration,
    slots: HashMap<EngineId, Slot>,
}

#[derive(Debug, Default)]
struct Slot {
    reading: tokio::sync::Mutex<()>,
    last: Mutex<Last>,
    generation: AtomicU64,
    reads: AtomicU64,
}

#[derive(Debug, Default)]
struct Last {
    good: Option<UsageLimits>,
    attempt: Option<(Instant, bool)>,
}

impl Slot {
    fn last(&self) -> std::sync::MutexGuard<'_, Last> {
        self.last.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl UsageCache {
    #[must_use]
    pub(crate) fn new(ttl: Duration) -> Self {
        let slots = EngineId::ALL
            .into_iter()
            .map(|engine| (engine, Slot::default()))
            .collect();
        Self { ttl, slots }
    }

    #[must_use]
    pub(crate) fn cached(&self, engine: EngineId) -> Option<UsageLimits> {
        self.slots.get(&engine)?.last().good.clone()
    }

    pub(crate) async fn read(
        &self,
        engine: EngineId,
        binary: &Path,
        home: &Path,
        force: bool,
    ) -> Option<UsageLimits> {
        let slot = self.slots.get(&engine)?;
        let observed_reads = slot.reads.load(Ordering::Acquire);
        let _reading = slot.reading.lock().await;
        let generation = slot.generation.load(Ordering::Acquire);
        let due = (force && slot.reads.load(Ordering::Acquire) == observed_reads)
            || slot.last().attempt.is_none_or(|(at, failed)| {
                at.elapsed() >= if failed { RETRY_AFTER } else { self.ttl }
            });
        if due {
            let usage = read_usage(engine, binary, home).await;
            let mut last = slot.last();
            if slot.generation.load(Ordering::Acquire) != generation {
                return last.good.clone();
            }
            slot.reads.fetch_add(1, Ordering::Release);
            last.attempt = Some((Instant::now(), usage.is_none()));
            if usage.is_some() {
                last.good = usage;
            }
        }
        slot.last().good.clone()
    }

    #[must_use]
    pub(crate) fn due_at(&self, engine: EngineId) -> Instant {
        self.slots
            .get(&engine)
            .expect("invariant: the usage cache has a slot for every engine")
            .last()
            .attempt
            .map_or_else(Instant::now, |(at, failed)| {
                at + if failed { RETRY_AFTER } else { self.ttl }
            })
    }

    pub(crate) fn unreadable(&self, engine: EngineId) {
        self.slots
            .get(&engine)
            .expect("invariant: the usage cache has a slot for every engine")
            .last()
            .attempt = Some((Instant::now(), true));
    }

    pub(crate) fn invalidate(&self, engine: EngineId) {
        if let Some(slot) = self.slots.get(&engine) {
            let mut last = slot.last();
            slot.generation.fetch_add(1, Ordering::Release);
            *last = Last::default();
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ReadClaudeUsageError {
    #[error("run claude /usage")]
    Probe(#[source] crate::process::ProbeError),
    #[error("claude /usage printed no JSON")]
    NoJson,
    #[error("decode claude /usage")]
    Decode(#[source] serde_json::Error),
    #[error("claude /usage has no result text")]
    NoResult,
}

async fn read_claude(binary: &Path, home: &Path) -> Result<Vec<UsageWindow>, ReadClaudeUsageError> {
    let mut command = engine_command(EngineId::Claude, binary, home);
    command
        .args([
            "-p",
            "/usage",
            "--output-format",
            "json",
            "--strict-mcp-config",
        ])
        .env("TZ", "UTC");
    let output = crate::process::probe(command, CLAUDE_TIMEOUT, 1024 * 1024)
        .await
        .map_err(ReadClaudeUsageError::Probe)?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let span = stdout
        .find('{')
        .zip(stdout.rfind('}'))
        .and_then(|(start, end)| stdout.get(start..=end))
        .ok_or(ReadClaudeUsageError::NoJson)?;
    let json: Value = serde_json::from_str(span).map_err(ReadClaudeUsageError::Decode)?;
    let text = json["result"]
        .as_str()
        .ok_or(ReadClaudeUsageError::NoResult)?;
    Ok(parse_claude_usage(text, artisan_api::now()))
}

fn parse_claude_usage(text: &str, now: Timestamp) -> Vec<UsageWindow> {
    let mut windows: Vec<UsageWindow> = Vec::new();
    for line in text.lines().map(str::trim) {
        let Some((head, rest)) = line.split_once(':') else {
            continue;
        };
        let (label, cadence, scope) = match head {
            "Current session" => ("Session".to_owned(), UsageCadence::Session, None),
            "Current week (all models)" => ("Week".to_owned(), UsageCadence::Week, None),
            _ => match head
                .strip_prefix("Current week (")
                .and_then(|model| model.strip_suffix(')'))
            {
                Some(model) if !model.trim().is_empty() => (
                    format!("Week ({})", model.trim()),
                    UsageCadence::Week,
                    Some(model.trim().to_owned()),
                ),
                _ => continue,
            },
        };
        let Some((percent, after)) = rest.split_once('%') else {
            continue;
        };
        let Ok(percent) = percent.trim().parse::<f64>() else {
            continue;
        };
        if !after.trim_start().starts_with("used")
            || windows.iter().any(|window| window.label == label)
        {
            continue;
        }
        windows.push(UsageWindow {
            label,
            cadence,
            scope,
            used_percent: percent.clamp(0.0, 100.0),
            resets_at: claude_reset(line, now),
        });
    }
    windows
}

fn claude_reset(line: &str, now: Timestamp) -> Option<Timestamp> {
    use chrono::{Datelike, NaiveDate, Utc};
    let (_, clause) = line.rsplit_once("resets ")?;
    let clause = clause.trim();
    let (when, zone) = clause.strip_suffix(')')?.rsplit_once('(')?;
    if !matches!(
        zone.trim().to_uppercase().as_str(),
        "UTC" | "GMT" | "ETC/UTC" | "Z"
    ) {
        return None;
    }
    let (date, time) = when.trim().split_once(',')?;
    let mut date = date.split_whitespace();
    let month_name = date.next()?.get(..3)?.to_lowercase();
    let month = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ]
    .iter()
    .zip(1_u32..)
    .find_map(|(name, number)| (*name == month_name).then_some(number))?;
    let day: u32 = date.next()?.parse().ok()?;
    let time = time.trim().to_lowercase();
    let (clock, pm) = match time.strip_suffix("pm") {
        Some(clock) => (clock, true),
        None => (time.strip_suffix("am")?, false),
    };
    let (hour, minute) = match clock.trim().split_once(':') {
        Some((hour, minute)) => (hour.parse::<u32>().ok()?, minute.parse::<u32>().ok()?),
        None => (clock.trim().parse::<u32>().ok()?, 0),
    };
    if !(1..=12).contains(&hour) || !(0..60).contains(&minute) || !(1..=31).contains(&day) {
        return None;
    }
    let hour = hour % 12 + if pm { 12 } else { 0 };
    let now_date = chrono::DateTime::<Utc>::from_timestamp_millis(now)?;
    [now_date.year() - 1, now_date.year(), now_date.year() + 1]
        .into_iter()
        .filter_map(|year| {
            NaiveDate::from_ymd_opt(year, month, day)?
                .and_hms_opt(hour, minute, 0)
                .map(|date| date.and_utc().timestamp_millis())
        })
        .min_by_key(|date| date.abs_diff(now))
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ReadCodexUsageError {
    #[error("codex rate limits timed out after {}s", CODEX_TIMEOUT.as_secs())]
    TimedOut,
    #[error("ask codex app-server")]
    Call(#[source] crate::rpc::CallError),
    #[error("codex is not signed in")]
    SignedOut,
}

async fn read_codex(binary: &Path, home: &Path) -> Result<Vec<UsageWindow>, ReadCodexUsageError> {
    tokio::time::timeout(CODEX_TIMEOUT, read_codex_inner(binary, home))
        .await
        .map_err(|_elapsed| ReadCodexUsageError::TimedOut)?
}

async fn read_codex_inner(
    binary: &Path,
    home: &Path,
) -> Result<Vec<UsageWindow>, ReadCodexUsageError> {
    let command = engine_command(EngineId::Codex, binary, home);
    let mut session = crate::rpc::Session::codex(command)
        .await
        .map_err(ReadCodexUsageError::Call)?;
    let account = session
        .call("account/read", json!({}))
        .await
        .map_err(ReadCodexUsageError::Call)?;
    if account["account"].is_null() {
        return Err(ReadCodexUsageError::SignedOut);
    }
    let limits = session
        .call("account/rateLimits/read", json!({}))
        .await
        .map_err(ReadCodexUsageError::Call)?;
    Ok(parse_codex_limits(&limits))
}

fn parse_codex_limits(result: &Value) -> Vec<UsageWindow> {
    let buckets: Vec<&Value> = result["rateLimitsByLimitId"].as_object().map_or_else(
        || result.get("rateLimits").into_iter().collect(),
        |map| map.values().collect(),
    );
    let mut windows = Vec::new();
    for bucket in buckets {
        let name = bucket["limitName"].as_str().filter(|name| !name.is_empty());
        for slot in ["primary", "secondary"] {
            let window = &bucket[slot];
            let Some(percent) = window["usedPercent"].as_f64() else {
                continue;
            };
            let span = window["windowDurationMins"]
                .as_u64()
                .map_or_else(|| slot.to_owned(), duration_label);
            windows.push(UsageWindow {
                label: name.map_or_else(|| span.clone(), |name| format!("{name} ({span})")),
                cadence: match window["windowDurationMins"].as_u64() {
                    Some(300) => UsageCadence::Session,
                    Some(10_080) => UsageCadence::Week,
                    _ => UsageCadence::Other,
                },
                scope: name.map(str::to_owned),
                used_percent: percent.clamp(0.0, 100.0),
                resets_at: window["resetsAt"].as_i64().map(|seconds| seconds * 1_000),
            });
        }
    }
    windows
}

fn duration_label(minutes: u64) -> String {
    match minutes {
        300 => "Session".to_owned(),
        10_080 => "Week".to_owned(),
        m if m % 1_440 == 0 => format!("{} days", m / 1_440),
        m if m % 60 == 0 => format!("{} hours", m / 60),
        m => format!("{m} minutes"),
    }
}

#[derive(Debug, thiserror::Error)]
#[error(
    "neither OpenCode Go limits nor spend could be read: {}",
    diagnostics::chain(go)
)]
pub(crate) struct ReadOpenCodeUsageError {
    go: Box<ReadGoUsageError>,
    #[source]
    spend: Box<ReadSpendError>,
}

async fn read_opencode(
    binary: &Path,
    home: &Path,
) -> Result<(Vec<UsageWindow>, Option<UsageSpend>), ReadOpenCodeUsageError> {
    let (windows, spend) = tokio::join!(
        read_opencode_go(binary, home),
        read_opencode_spend(binary, home)
    );
    match (windows, spend) {
        (Err(go), Err(spend)) => Err(ReadOpenCodeUsageError {
            go: Box::new(go),
            spend: Box::new(spend),
        }),
        (windows, spend) => {
            let windows = windows.unwrap_or_else(|error| {
                tracing::info!(home = %home.display(), error = diagnostics::chain(&error), "OpenCode Go limits unavailable");
                Vec::new()
            });
            let spend = match spend {
                Ok(spend) => Some(spend),
                Err(error) => {
                    tracing::info!(home = %home.display(), error = diagnostics::chain(&error), "OpenCode spend unavailable");
                    None
                }
            };
            Ok((windows, spend))
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ReadGoUsageError {
    #[error("list OpenCode credentials")]
    Run(#[source] crate::account::RunShortError),
    #[error("OpenCode could not list its credentials ({stderr_bytes} bytes on stderr)")]
    Refused { stderr_bytes: usize },
    #[error("read OpenCode's credentials")]
    Credentials(#[source] serde_json::Error),
    #[error("no OpenCode Console or Go credential")]
    NoCredential,
    #[error("build the HTTP client for {OPENCODE_GO_USAGE}")]
    Client(#[source] reqwest::Error),
    #[error("ask {OPENCODE_GO_USAGE} for Go usage")]
    Request(#[source] reqwest::Error),
    #[error("{OPENCODE_GO_USAGE} answered Go usage with {status}")]
    Status { status: reqwest::StatusCode },
    #[error("read Go usage from {OPENCODE_GO_USAGE}")]
    Body(#[source] reqwest::Error),
    #[error("decode Go usage from {OPENCODE_GO_USAGE}")]
    Decode(#[source] serde_json::Error),
}

async fn read_opencode_go(
    binary: &Path,
    home: &Path,
) -> Result<Vec<UsageWindow>, ReadGoUsageError> {
    let (success, stdout, stderr) = crate::account::run_short(
        EngineId::OpenCode,
        binary,
        home,
        &["api", "--standalone", "credential.list"],
        OPENCODE_TIMEOUT,
    )
    .await
    .map_err(ReadGoUsageError::Run)?;
    if !success {
        return Err(ReadGoUsageError::Refused {
            stderr_bytes: stderr.len(),
        });
    }
    let listing: Value = serde_json::from_str(&stdout).map_err(ReadGoUsageError::Credentials)?;
    let (token, org) =
        go_credential(&listing, artisan_api::now()).ok_or(ReadGoUsageError::NoCredential)?;
    let mut request = reqwest::Client::builder()
        .timeout(OPENCODE_TIMEOUT)
        .build()
        .map_err(ReadGoUsageError::Client)?
        .get(OPENCODE_GO_USAGE)
        .bearer_auth(token);
    if let Some(org) = org {
        request = request.header("x-org-id", org);
    }
    let response = request.send().await.map_err(ReadGoUsageError::Request)?;
    let status = response.status();
    if !status.is_success() {
        return Err(ReadGoUsageError::Status { status });
    }
    let bytes = response.bytes().await.map_err(ReadGoUsageError::Body)?;
    let body: Value = serde_json::from_slice(&bytes).map_err(ReadGoUsageError::Decode)?;
    Ok(parse_go_usage(&body))
}

fn go_credential(listing: &Value, now: Timestamp) -> Option<(String, Option<String>)> {
    let credentials = listing.get("data").unwrap_or(listing).as_array()?;
    let active = |integration: &str| {
        credentials.iter().find(|credential| {
            credential["integrationID"] == integration && credential["active"] != false
        })
    };
    for integration in ["opencode-go", "opencode"] {
        let Some(credential) = active(integration) else {
            continue;
        };
        let value = &credential["value"];
        let token = match value["type"].as_str() {
            Some("key") => value["key"].as_str(),
            Some("oauth") if value["expires"].as_i64().is_some_and(|at| at > now) => {
                value["access"].as_str()
            }
            _ => None,
        };
        if let Some(token) = token.filter(|token| !token.is_empty()) {
            let org = value["metadata"]["orgID"].as_str().map(str::to_owned);
            return Some((token.to_owned(), org));
        }
    }
    None
}

fn parse_go_usage(body: &Value) -> Vec<UsageWindow> {
    [
        ("rolling", "Session", UsageCadence::Session),
        ("weekly", "Week", UsageCadence::Week),
        ("monthly", "Month", UsageCadence::Month),
    ]
    .into_iter()
    .filter_map(|(key, label, cadence)| {
        let window = &body["usage"][key];
        let percent = if window["status"] == "rate-limited" {
            100.0
        } else {
            window["percent"].as_f64()?
        };
        Some(UsageWindow {
            label: label.to_owned(),
            cadence,
            scope: Some("OpenCode Go".to_owned()),
            used_percent: percent.clamp(0.0, 100.0),
            resets_at: window["resetsAt"]
                .as_str()
                .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
                .map(|at| at.timestamp_millis()),
        })
    })
    .collect()
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ReadSpendError {
    #[error("run opencode stats for {days} days")]
    Run {
        days: &'static str,
        #[source]
        source: crate::account::RunShortError,
    },
    #[error("opencode stats for {days} days failed ({stderr_bytes} bytes on stderr)")]
    Refused {
        days: &'static str,
        stderr_bytes: usize,
    },
    #[error("read opencode stats for {days} days")]
    Decode {
        days: &'static str,
        #[source]
        source: serde_json::Error,
    },
}

async fn read_opencode_spend(binary: &Path, home: &Path) -> Result<UsageSpend, ReadSpendError> {
    let mut spend = UsageSpend::default();
    let mut models = Vec::new();
    for (days, label) in [("0", "Today"), ("7", "Last 7 days"), ("30", "Last 30 days")] {
        let (success, stdout, stderr) = crate::account::run_short(
            EngineId::OpenCode,
            binary,
            home,
            &["stats", "--standalone", "--json", "--days", days],
            OPENCODE_TIMEOUT,
        )
        .await
        .map_err(|source| ReadSpendError::Run { days, source })?;
        if !success {
            return Err(ReadSpendError::Refused {
                days,
                stderr_bytes: stderr.len(),
            });
        }
        let stats: Value = serde_json::from_str(&stdout)
            .map_err(|source| ReadSpendError::Decode { days, source })?;
        spend.periods.push(spend_line(label, &stats));
        models = stats["models"].as_array().cloned().unwrap_or_default();
    }
    spend.models = spend_by_model(&models);
    Ok(spend)
}

fn spend_line(label: &str, entry: &Value) -> SpendLine {
    let tokens = &entry["tokens"];
    SpendLine {
        label: label.to_owned(),
        cost_usd: entry["cost"].as_f64().unwrap_or_default(),
        tokens: ["input", "output", "reasoning"]
            .iter()
            .filter_map(|key| tokens[key].as_u64())
            .sum(),
    }
}

fn spend_by_model(models: &[Value]) -> Vec<SpendLine> {
    let mut lines: Vec<SpendLine> = Vec::new();
    for entry in models {
        let Some(id) = entry["model"]["id"].as_str() else {
            continue;
        };
        let line = spend_line(id, entry);
        match lines.iter_mut().find(|known| known.label == id) {
            Some(known) => {
                known.cost_usd += line.cost_usd;
                known.tokens += line.tokens;
            }
            None => lines.push(line),
        }
    }
    lines.sort_by(|a, b| {
        b.cost_usd
            .total_cmp(&a.cost_usd)
            .then(b.tokens.cmp(&a.tokens))
    });
    lines
}

#[must_use]
pub(crate) const fn has_usage_reader(engine: EngineId) -> bool {
    matches!(
        engine,
        EngineId::Claude | EngineId::Codex | EngineId::OpenCode
    )
}

#[cfg(test)]
#[path = "../tests/usage/unit.rs"]
mod tests;
