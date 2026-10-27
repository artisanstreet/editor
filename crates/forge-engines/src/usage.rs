//! Best-effort rate-limit readers. Every failure is `None`; the Editor then
//! shows "usage unavailable". None of these calls a model.
//!
//! - Claude: `claude -p /usage --output-format json` (a local slash command),
//!   run in UTC so reset times resolve to instants.
//! - Codex: `account/rateLimits/read` over `codex app-server`.
//! - OpenCode: the Go plan's windows from `opencode.ai/zen/go/v1/usage`
//!   with the stored Console or Go credential, and spend from
//!   `opencode stats --json`.

use crate::account::engine_command;
use anyhow::{Context, Result, bail};
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
use std::time::{Duration, Instant};

/// `claude -p /usage` starts a whole CLI: about 5 s, far more on a busy
/// machine.
const CLAUDE_TIMEOUT: Duration = Duration::from_secs(45);
/// A failed read is tried again after this, not after the full TTL.
const RETRY_AFTER: Duration = Duration::from_secs(60);
const CODEX_TIMEOUT: Duration = Duration::from_secs(15);
/// Each `opencode` command starts a private server: well under a second.
const OPENCODE_TIMEOUT: Duration = Duration::from_secs(30);
const OPENCODE_GO_USAGE: &str = "https://opencode.ai/zen/go/v1/usage";

/// Reads usage without caching. `None` when the engine has no reader or the
/// read failed.
pub async fn read_usage(engine: EngineId, binary: &Path, home: &Path) -> Option<UsageLimits> {
    let read = match engine {
        EngineId::Claude => read_claude(binary, home)
            .await
            .map(|windows| (windows, None)),
        EngineId::Codex => read_codex(binary, home)
            .await
            .map(|windows| (windows, None)),
        EngineId::OpenCode => read_opencode(binary, home).await,
    };
    match read {
        Ok((windows, spend)) if !windows.is_empty() || spend.is_some() => Some(UsageLimits {
            windows,
            spend: spend.map(Box::new),
            read_at: artisan_api::now(),
        }),
        Ok(_) => None,
        Err(error) => {
            tracing::info!(engine = %engine, home = %home.display(), error = %format!("{error:#}"), "usage unavailable");
            None
        }
    }
}

/// Usage reads cached in memory for `ttl`, one read in flight per engine.
pub struct UsageCache {
    ttl: Duration,
    slots: HashMap<EngineId, Slot>,
}

#[derive(Default)]
struct Slot {
    reading: tokio::sync::Mutex<()>,
    last: Mutex<Last>,
    generation: AtomicU64,
    reads: AtomicU64,
}

/// One engine's reads: the last good one is kept through failures.
#[derive(Default)]
struct Last {
    good: Option<UsageLimits>,
    /// When it was last read, and whether that read failed.
    attempt: Option<(Instant, bool)>,
}

impl Slot {
    fn last(&self) -> std::sync::MutexGuard<'_, Last> {
        self.last.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl UsageCache {
    #[must_use]
    pub fn new(ttl: Duration) -> Self {
        let slots = EngineId::ALL
            .into_iter()
            .map(|engine| (engine, Slot::default()))
            .collect();
        Self { ttl, slots }
    }

    /// The last good read, however old; never reads.
    #[must_use]
    pub fn cached(&self, engine: EngineId) -> Option<UsageLimits> {
        self.slots.get(&engine)?.last().good.clone()
    }

    /// Reads the engine's usage when its last read is due (older than the
    /// TTL, or a minute after a failure), or always with `force`. Returns
    /// the last good read: a failed read keeps the one before, which shows
    /// how old it is.
    pub async fn read(
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

    /// Forgets the reads (after a sign-in change): another account's usage
    /// is not this one's.
    pub fn invalidate(&self, engine: EngineId) {
        if let Some(slot) = self.slots.get(&engine) {
            let mut last = slot.last();
            slot.generation.fetch_add(1, Ordering::Release);
            *last = Last::default();
        }
    }
}

// --- Claude ----------------------------------------------------------------

async fn read_claude(binary: &Path, home: &Path) -> Result<Vec<UsageWindow>> {
    let mut command = engine_command(EngineId::Claude, binary, home);
    command
        // MCP servers are no part of `/usage`; starting them took most of
        // its 30 s and timed it out.
        .args(["-p", "/usage", "--output-format", "json", "--strict-mcp-config"])
        .env("TZ", "UTC");
    let output = crate::process::probe(command, CLAUDE_TIMEOUT, 1024 * 1024).await?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let start = stdout.find('{').context("claude /usage printed no JSON")?;
    let end = stdout.rfind('}').context("claude /usage printed no JSON")?;
    let json: Value =
        serde_json::from_str(&stdout[start..=end]).context("decoding claude /usage")?;
    let text = json["result"]
        .as_str()
        .context("claude /usage has no result text")?;
    Ok(parse_claude_usage(text, artisan_api::now()))
}

/// Reads `Current session: 3% used · resets Oct 11, 11:49am (UTC)` lines.
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

/// `resets Oct 11, 11:49am (UTC)` → ms. Only UTC resolves; the reader runs
/// the CLI with `TZ=UTC` so that is what it prints.
fn claude_reset(line: &str, now: Timestamp) -> Option<Timestamp> {
    use chrono::{Datelike, NaiveDate, Utc};
    let clause = line[line.rfind("resets ")? + "resets ".len()..].trim();
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

// --- Codex -----------------------------------------------------------------

async fn read_codex(binary: &Path, home: &Path) -> Result<Vec<UsageWindow>> {
    tokio::time::timeout(CODEX_TIMEOUT, read_codex_inner(binary, home))
        .await
        .context("codex rate limits timed out")?
}

async fn read_codex_inner(binary: &Path, home: &Path) -> Result<Vec<UsageWindow>> {
    let mut session =
        crate::rpc::Session::codex(engine_command(EngineId::Codex, binary, home)).await?;
    let account = session.call("account/read", json!({})).await?;
    if account["account"].is_null() {
        bail!("codex is not signed in");
    }
    let limits = session.call("account/rateLimits/read", json!({})).await?;
    Ok(parse_codex_limits(&limits))
}

/// Maps `account/rateLimits/read` to windows, one per non-null slot.
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

// --- OpenCode --------------------------------------------------------------

/// The Go plan's windows and the spend `OpenCode` priced. Either may be
/// missing (no Go plan, an older `OpenCode`); only both failing fails.
async fn read_opencode(
    binary: &Path,
    home: &Path,
) -> Result<(Vec<UsageWindow>, Option<UsageSpend>)> {
    let (windows, spend) = tokio::join!(
        read_opencode_go(binary, home),
        read_opencode_spend(binary, home)
    );
    match (windows, spend) {
        (Err(windows), Err(spend)) => Err(spend.context(format!("{windows:#}"))),
        (windows, spend) => {
            if let Err(error) = &windows {
                tracing::info!(home = %home.display(), error = %format!("{error:#}"), "OpenCode Go limits unavailable");
            }
            if let Err(error) = &spend {
                tracing::info!(home = %home.display(), error = %format!("{error:#}"), "OpenCode spend unavailable");
            }
            Ok((windows.unwrap_or_default(), spend.ok()))
        }
    }
}

async fn read_opencode_go(binary: &Path, home: &Path) -> Result<Vec<UsageWindow>> {
    let (success, stdout, stderr) = crate::account::run_short(
        EngineId::OpenCode,
        binary,
        home,
        &["api", "--standalone", "credential.list"],
        OPENCODE_TIMEOUT,
    )
    .await?;
    if !success {
        bail!(
            "listing OpenCode credentials: {}",
            crate::account::strip_ansi(stderr.trim())
        );
    }
    let listing: Value = serde_json::from_str(&stdout).context("reading OpenCode's credentials")?;
    let (token, org) = go_credential(&listing, artisan_api::now())
        .context("no OpenCode Console or Go credential")?;
    let mut request = reqwest::Client::builder()
        .timeout(OPENCODE_TIMEOUT)
        .build()?
        .get(OPENCODE_GO_USAGE)
        .bearer_auth(token);
    if let Some(org) = org {
        request = request.header("x-org-id", org);
    }
    let response = request
        .send()
        .await
        .context("asking opencode.ai for Go usage")?;
    let status = response.status();
    if !status.is_success() {
        bail!("opencode.ai answered Go usage with {status}");
    }
    let body: Value =
        serde_json::from_slice(&response.bytes().await?).context("reading Go usage")?;
    Ok(parse_go_usage(&body))
}

/// The bearer for Go usage: a Go key, else the Console sign-in while its
/// access token is good (`OpenCode` refreshes it whenever it loads).
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

/// `{"usage":{"rolling":{"status","percent","resetsAt"},"weekly",..,"monthly"}}`;
/// a `rate-limited` window is spent whatever its percent says.
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

async fn read_opencode_spend(binary: &Path, home: &Path) -> Result<UsageSpend> {
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
        .await?;
        if !success {
            bail!(
                "opencode stats: {}",
                crate::account::strip_ansi(stderr.trim())
            );
        }
        let stats: Value = serde_json::from_str(&stdout).context("reading opencode stats")?;
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

/// One line per model, its variants summed, costliest first.
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

/// Whether the engine has a usage reader at all.
#[must_use]
pub const fn has_usage_reader(engine: EngineId) -> bool {
    matches!(
        engine,
        EngineId::Claude | EngineId::Codex | EngineId::OpenCode
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::sync::Arc;

    // 2026-10-11T09:00:00Z
    const NOW: Timestamp = 1_791_709_200_000;

    /// `claude -p /usage --output-format json` result text, recorded
    /// 2026-10-11 with Claude Code 2.1.296.
    #[test]
    fn claude_usage_text_becomes_windows() {
        let result = "You are currently using your subscription to power your Claude Code usage\n\nCurrent session: 3% used · resets Oct 11, 11:49am (UTC)\nCurrent week (all models): 34% used · resets Oct 13, 9:59pm (UTC)\nCurrent week (Fable): 0% used · resets Oct 13, 10pm (UTC)\n\nWhat's contributing to your limits usage?\nLast 24h · 3375 requests · 21 sessions\n  62% of your usage came from subagent-heavy sessions\n";
        insta::assert_debug_snapshot!(parse_claude_usage(result, NOW));
    }

    /// `account/rateLimits/read` recorded from Codex 0.162.1 (ids removed),
    /// plus a two-slot bucket in the shape older plans return.
    #[test]
    fn codex_rate_limits_become_windows() {
        let recorded = json!({
            "rateLimits": { "limitId": "codex", "limitName": null, "primary": { "usedPercent": 2, "windowDurationMins": 10080, "resetsAt": 1_792_296_124 }, "secondary": null },
            "rateLimitsByLimitId": { "codex": { "limitId": "codex", "limitName": null, "primary": { "usedPercent": 2, "windowDurationMins": 10080, "resetsAt": 1_792_296_124 }, "secondary": null } }
        });
        let two_slots = json!({
            "rateLimitsByLimitId": { "codex": { "limitName": "GPT-5", "primary": { "usedPercent": 42.5, "windowDurationMins": 300, "resetsAt": 1_791_720_000 }, "secondary": { "usedPercent": 150.0, "windowDurationMins": 10080 } } }
        });
        insta::assert_debug_snapshot!((
            parse_codex_limits(&recorded),
            parse_codex_limits(&two_slots)
        ));
    }

    /// `opencode.ai/zen/go/v1/usage`, recorded 2026-10-21 with a Console
    /// sign-in on the Go plan.
    #[test]
    fn opencode_go_usage_becomes_three_windows() {
        let recorded = json!({"usage":{"rolling":{"status":"ok","percent":0,"resetsAt":"2026-10-21T12:51:10.000Z"},"weekly":{"status":"ok","percent":0,"resetsAt":"2026-10-26T00:00:00.000Z"},"monthly":{"status":"ok","percent":25,"resetsAt":"2026-11-02T21:57:01.000Z"}}});
        let windows = parse_go_usage(&recorded);
        assert_eq!(
            windows
                .iter()
                .map(|window| (window.cadence, window.used_percent, window.resets_at))
                .collect::<Vec<_>>(),
            [
                (UsageCadence::Session, 0.0, Some(1_792_587_070_000)),
                (UsageCadence::Week, 0.0, Some(1_792_972_800_000)),
                (UsageCadence::Month, 25.0, Some(1_793_656_621_000)),
            ]
        );
        let limited =
            json!({"usage":{"rolling":{"status":"rate-limited","percent":97,"resetsAt":null}}});
        assert!(
            (parse_go_usage(&limited)[0].used_percent - 100.0).abs() < f64::EPSILON,
            "rate-limited usage reads as 100%"
        );
    }

    #[test]
    fn a_go_key_wins_and_an_expired_console_sign_in_reads_nothing() {
        let console = |expires: i64| json!({"integrationID":"opencode","active":true,"value":{"type":"oauth","access":"console","expires":expires,"metadata":{"orgID":"org"}}});
        let go =
            json!({"integrationID":"opencode-go","active":true,"value":{"type":"key","key":"go"}});
        assert_eq!(
            go_credential(&json!({"data":[console(NOW + 1), go]}), NOW),
            Some(("go".to_owned(), None))
        );
        assert_eq!(
            go_credential(&json!([console(NOW + 1)]), NOW),
            Some(("console".to_owned(), Some("org".to_owned())))
        );
        assert_eq!(go_credential(&json!([console(NOW - 1)]), NOW), None);
    }

    /// `opencode stats --json` models, recorded 2026-10-21 with OpenCode
    /// 2.1.0: a model's variants are one line.
    #[test]
    fn opencode_spend_sums_variants_costliest_first() {
        let models = json!([
            {"model":{"id":"minimax-m3.1-flash-free","providerID":"opencode-go","variant":"high"},"tokens":{"input":1_623_901,"output":54_112,"reasoning":78_255,"cache":{"read":71_616_226,"write":0}},"cost":0},
            {"model":{"id":"glm-5.2","providerID":"opencode-go"},"tokens":{"input":10,"output":5,"reasoning":0,"cache":{"read":0,"write":0}},"cost":0.42},
            {"model":{"id":"minimax-m3.1-flash-free","providerID":"opencode-go","variant":"max"},"tokens":{"input":374_992,"output":54_446,"reasoning":109_314,"cache":{"read":54_992_977,"write":0}},"cost":0}
        ]);
        let lines = spend_by_model(models.as_array().unwrap());
        assert_eq!(
            lines
                .iter()
                .map(|line| (line.label.as_str(), line.cost_usd, line.tokens))
                .collect::<Vec<_>>(),
            [
                ("glm-5.2", 0.42, 15),
                ("minimax-m3.1-flash-free", 0.0, 2_295_020),
            ]
        );
    }

    #[cfg(unix)]
    fn gated_usage_cli(home: &Path) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt as _;
        let binary = home.join("fake-usage");
        std::fs::write(&binary, r#"#!/bin/sh
percentage=$(cat "$PWD/percentage")
printf 'probe\n' >> "$PWD/calls"
touch "$PWD/started"
while [ ! -e "$PWD/release" ]; do sleep 0.01; done
printf '{"result":"Current week (all models): %s%% used · resets Oct 13, 9:59pm (UTC)"}\n' "$percentage"
"#).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(home.join("percentage"), "34").unwrap();
        binary
    }

    #[cfg(unix)]
    async fn wait_for_usage_probe(home: &Path) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while !home.join("started").exists() {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    #[expect(
        clippy::float_cmp,
        reason = "the percentage is parsed from an exact integer"
    )]
    async fn an_invalidated_old_account_read_cannot_repopulate_usage() {
        let directory = tempfile::tempdir().unwrap();
        let binary = gated_usage_cli(directory.path());
        let cache = Arc::new(UsageCache::new(Duration::from_secs(300)));
        let owner = Arc::clone(&cache);
        let home = directory.path().to_owned();
        let old_binary = binary.clone();
        let old_read =
            tokio::spawn(
                async move { owner.read(EngineId::Claude, &old_binary, &home, true).await },
            );
        wait_for_usage_probe(directory.path()).await;
        cache.invalidate(EngineId::Claude);
        std::fs::write(directory.path().join("percentage"), "15").unwrap();
        std::fs::write(directory.path().join("release"), "").unwrap();
        assert!(old_read.await.unwrap().is_none());
        assert!(cache.cached(EngineId::Claude).is_none());
        let current = cache
            .read(EngineId::Claude, &binary, directory.path(), true)
            .await
            .unwrap();
        assert_eq!(current.windows[0].used_percent, 15.0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn overlapping_forced_usage_reads_share_the_same_probe() {
        let directory = tempfile::tempdir().unwrap();
        let binary = gated_usage_cli(directory.path());
        let cache = Arc::new(UsageCache::new(Duration::from_secs(300)));
        let owner = Arc::clone(&cache);
        let home = directory.path().to_owned();
        let first_binary = binary.clone();
        let first = tokio::spawn(async move {
            owner
                .read(EngineId::Claude, &first_binary, &home, true)
                .await
        });
        wait_for_usage_probe(directory.path()).await;
        let owner = Arc::clone(&cache);
        let home = directory.path().to_owned();
        let (started, waiting) = tokio::sync::oneshot::channel();
        let second = tokio::spawn(async move {
            started.send(()).ok();
            owner.read(EngineId::Claude, &binary, &home, true).await
        });
        waiting.await.unwrap();
        // The current-thread runtime has polled the second read to its mutex wait.
        std::fs::write(directory.path().join("release"), "").unwrap();
        assert_eq!(first.await.unwrap(), second.await.unwrap());
        assert_eq!(
            std::fs::read_to_string(directory.path().join("calls"))
                .unwrap()
                .lines()
                .count(),
            1
        );
    }

    /// A usage read that fails (Claude timing out on a busy machine) keeps
    /// the last good reading; it was showing "checking" and then nothing.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_failed_read_keeps_the_last_good_one() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let fail = dir.path().join("fail");
        let binary = dir.path().join("claude");
        let result = "Current week (all models): 34% used · resets Oct 13, 9:59pm (UTC)";
        std::fs::write(
            &binary,
            format!(
                "#!/bin/sh\n[ -e {fail} ] && exit 1\necho '{{\"result\":\"{result}\"}}'\n",
                fail = fail.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        let cache = UsageCache::new(Duration::from_secs(300));
        let read = |force| cache.read(EngineId::Claude, &binary, dir.path(), force);
        let good = read(true).await.expect("the first read");
        std::fs::write(&fail, "").unwrap();
        let after = read(true).await;
        assert_eq!(
            after.as_ref(),
            Some(&good),
            "a failed read dropped the last good one"
        );
    }
}
