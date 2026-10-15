//! Best-effort rate-limit readers. Every failure is `None`; the Editor then
//! shows "usage unavailable". None of these calls a model.
//!
//! - Claude: `claude -p /usage --output-format json` (a local slash command),
//!   run in UTC so reset times resolve to instants.
//! - Codex: `account/rateLimits/read` over `codex app-server`.
//! - Cursor: the dashboard's `GetCurrentPeriodUsage` with the CLI's token.

use crate::account::engine_command;
use anyhow::{Context, Result, bail};
use artisan_api::{EngineId, Timestamp, UsageCadence, UsageLimits, UsageWindow};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
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
const CURSOR_TIMEOUT: Duration = Duration::from_secs(10);
const CURSOR_USAGE_URL: &str =
    "https://api2.cursor.sh/aiserver.v1.DashboardService/GetCurrentPeriodUsage";

/// Reads usage without caching. `None` when the engine has no reader or the
/// read failed.
pub async fn read_usage(engine: EngineId, binary: &Path, home: &Path) -> Option<UsageLimits> {
    let read = match engine {
        EngineId::Claude => read_claude(binary, home).await,
        EngineId::Codex => read_codex(binary, home).await,
        EngineId::Cursor => read_cursor(home).await,
        EngineId::Grok | EngineId::OpenCode => return None,
    };
    match read {
        Ok(windows) if !windows.is_empty() => Some(UsageLimits {
            windows,
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
    .zip(1_i64..)
    .find_map(|(name, number)| (*name == month_name).then_some(number))?;
    let day: i64 = date.next()?.parse().ok()?;
    let time = time.trim().to_lowercase();
    let (clock, pm) = match time.strip_suffix("pm") {
        Some(clock) => (clock, true),
        None => (time.strip_suffix("am")?, false),
    };
    let (hour, minute) = match clock.trim().split_once(':') {
        Some((hour, minute)) => (hour.parse::<i64>().ok()?, minute.parse::<i64>().ok()?),
        None => (clock.trim().parse::<i64>().ok()?, 0),
    };
    if !(1..=12).contains(&hour) || !(0..60).contains(&minute) || !(1..=31).contains(&day) {
        return None;
    }
    let hour = hour % 12 + if pm { 12 } else { 0 };
    use chrono::{Datelike, NaiveDate, Utc};
    let now_date = chrono::DateTime::<Utc>::from_timestamp_millis(now)?;
    [now_date.year() - 1, now_date.year(), now_date.year() + 1]
        .into_iter()
        .filter_map(|year| {
            NaiveDate::from_ymd_opt(year, month as u32, day as u32)?
                .and_hms_opt(hour as u32, minute as u32, 0)
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
    let buckets: Vec<&Value> = match result["rateLimitsByLimitId"].as_object() {
        Some(map) => map.values().collect(),
        None => result.get("rateLimits").into_iter().collect(),
    };
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

// --- Cursor ----------------------------------------------------------------

fn cursor_auth_file(home: &Path) -> PathBuf {
    if cfg!(windows) {
        home.join("AppData/Roaming/Cursor/auth.json")
    } else {
        home.join(".config/cursor/auth.json")
    }
}

async fn read_cursor(home: &Path) -> Result<Vec<UsageWindow>> {
    let path = cursor_auth_file(home);
    let auth: Value = serde_json::from_slice(
        &tokio::fs::read(&path)
            .await
            .with_context(|| format!("reading {}", path.display()))?,
    )
    .with_context(|| format!("decoding {}", path.display()))?;
    let token = auth["accessToken"]
        .as_str()
        .filter(|token| !token.is_empty())
        .context("cursor has no access token")?;
    let response = reqwest::Client::builder()
        .timeout(CURSOR_TIMEOUT)
        .build()?
        .post(CURSOR_USAGE_URL)
        .bearer_auth(token)
        .header("connect-protocol-version", "1")
        .header("x-cursor-client-type", "cli")
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .context("calling the cursor dashboard")?
        .error_for_status()
        .context("cursor dashboard")?;
    let body: Value = serde_json::from_slice(&response.bytes().await?)
        .context("decoding the cursor dashboard")?;
    parse_cursor_usage(&body)
}

/// Maps `GetCurrentPeriodUsage`. Protobuf JSON omits zeros.
fn parse_cursor_usage(response: &Value) -> Result<Vec<UsageWindow>> {
    let plan = response["planUsage"]
        .as_object()
        .context("cursor usage has no planUsage")?;
    let number = |value: &Value| value.as_f64().or_else(|| value.as_str()?.parse().ok());
    #[expect(clippy::cast_possible_truncation, reason = "a millisecond timestamp")]
    let resets_at = number(&response["billingCycleEnd"]).map(|ms| ms as Timestamp);
    let window = |label: &str, percent: f64| UsageWindow {
        label: label.to_owned(),
        cadence: UsageCadence::Other,
        scope: None,
        used_percent: percent.clamp(0.0, 100.0),
        resets_at,
    };
    let mut windows = Vec::new();
    let auto = plan.get("autoPercentUsed").and_then(number);
    let api = plan.get("apiPercentUsed").and_then(number);
    if auto.is_some() || api.is_some() || response["autoBucketModels"].is_array() {
        windows.push(window("Cursor models", auto.unwrap_or(0.0)));
        windows.push(window("Other models", api.unwrap_or(0.0)));
    } else {
        let spend = plan.get("totalSpend").and_then(number).unwrap_or(0.0);
        let limit = plan.get("limit").and_then(number).unwrap_or(0.0);
        let percent = if limit > 0.0 {
            spend / limit * 100.0
        } else {
            plan.get("totalPercentUsed")
                .and_then(number)
                .or_else(|| {
                    response["displayMessage"]
                        .as_str()
                        .and_then(percent_in_text)
                })
                .unwrap_or(0.0)
        };
        windows.push(window("Included usage", percent));
    }
    let spend = &response["spendLimitUsage"];
    for (limit, used, remaining) in [
        ("overallLimit", "overallUsed", "overallRemaining"),
        ("individualLimit", "individualUsed", "individualRemaining"),
        ("pooledLimit", "pooledUsed", "pooledRemaining"),
    ] {
        let Some(limit) = number(&spend[limit]).filter(|limit| *limit > 0.0) else {
            continue;
        };
        let used = number(&spend[used])
            .or_else(|| number(&spend[remaining]).map(|left| (limit - left).max(0.0)))
            .unwrap_or(0.0);
        windows.push(window("On-demand", used / limit * 100.0));
        break;
    }
    Ok(windows)
}

/// The first `N%` in display copy like "You have used 73% of your plan".
fn percent_in_text(text: &str) -> Option<f64> {
    text.split_whitespace().find_map(|word| {
        word.trim_end_matches(|c: char| !c.is_ascii_digit() && c != '%')
            .strip_suffix('%')?
            .parse()
            .ok()
    })
}

/// Whether the engine has a usage reader at all.
#[must_use]
pub fn has_usage_reader(engine: EngineId) -> bool {
    matches!(
        engine,
        EngineId::Claude | EngineId::Codex | EngineId::Cursor
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

    /// Old fixtures for `GetCurrentPeriodUsage` (no signed-in Cursor here).
    #[test]
    fn cursor_dashboard_becomes_windows() {
        let split = json!({ "billingCycleEnd": "1792000000000", "planUsage": { "totalSpend": 30.0, "limit": 100.0, "autoPercentUsed": 40.0, "apiPercentUsed": 10.0 } });
        let single = json!({ "planUsage": { "totalSpend": 1.0, "limit": 2.0 }, "spendLimitUsage": { "overallLimit": 100.0, "overallUsed": 25.0 } });
        let display =
            json!({ "planUsage": {}, "displayMessage": "You have used 73% of your plan" });
        insta::assert_debug_snapshot!(
            [split, single, display]
                .iter()
                .map(|r| parse_cursor_usage(r).unwrap())
                .collect::<Vec<_>>()
        );
        assert!(parse_cursor_usage(&json!({})).is_err());
    }

    #[cfg(unix)]
    fn gated_usage_cli(home: &Path) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt as _;
        let binary = home.join("fake-usage");
        std::fs::write(&binary, r#"#!/bin/sh
percentage=$(cat "$HOME/percentage")
printf 'probe\n' >> "$HOME/calls"
touch "$HOME/started"
while [ ! -e "$HOME/release" ]; do sleep 0.01; done
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
    async fn an_invalidated_old_account_read_cannot_repopulate_usage() {
        let directory = tempfile::tempdir().unwrap();
        let binary = gated_usage_cli(directory.path());
        let cache = Arc::new(UsageCache::new(Duration::from_secs(300)));
        let owner = cache.clone();
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
        let owner = cache.clone();
        let home = directory.path().to_owned();
        let first_binary = binary.clone();
        let first = tokio::spawn(async move {
            owner
                .read(EngineId::Claude, &first_binary, &home, true)
                .await
        });
        wait_for_usage_probe(directory.path()).await;
        let owner = cache.clone();
        let home = directory.path().to_owned();
        let (started, waiting) = tokio::sync::oneshot::channel();
        let second = tokio::spawn(async move {
            let _ = started.send(());
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

#[cfg(all(test, windows))]
mod windows_managed_home_tests {
    use super::*;
    #[test]
    fn native_cursor_credentials_stay_inside_the_supplied_engine_home() {
        let managed = tempfile::tempdir().unwrap();
        let path = cursor_auth_file(managed.path());
        assert_eq!(
            path,
            managed
                .path()
                .join("AppData")
                .join("Roaming")
                .join("Cursor")
                .join("auth.json")
        );
        assert!(path.starts_with(managed.path()));
        if let Some(global) = std::env::var_os("APPDATA") {
            assert_ne!(path, PathBuf::from(global).join("Cursor").join("auth.json"));
        }
    }
}
