//! Bounded non-billable Claude account-usage read.
//!
//! Mirrors `MakeClaudeUsage` in `modules/engines/src/claude/usage.ts`: run
//! the external `claude` executable once with `-p /usage --output-format
//! json`, close stdin immediately (its `-p` prompt would otherwise wait on
//! an open pipe), bound stdout/stderr to 1 MiB each, enforce one overall
//! deadline with kill plus reap, then parse the session and weekly lines
//! from the embedded result text.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use artisan_domain::{EngineUsageWindow, EngineUsageWindowKind, clamp_percent_used, utc_ymd};

use super::account_usage::{ProviderUsage, UsageReaderError};

/// Default overall deadline for one Claude usage read (20 seconds).
pub const CLAUDE_USAGE_TIMEOUT: Duration = Duration::from_secs(20);
/// Default per-stream byte ceiling for Claude CLI output (1 MiB).
pub const CLAUDE_USAGE_MAX_BYTES: usize = 1_048_576;
/// Arguments passing the usage slash command as JSON through `-p`.
pub const CLAUDE_USAGE_ARGS: &[&str] = &["-p", "/usage", "--output-format", "json"];

/// Configures one external Claude CLI account-usage read.
#[derive(Clone, Debug)]
pub struct ClaudeUsageConfig {
    /// Provider executable (absolute path or PATH-resolved name).
    pub executable: PathBuf,
    /// Extra arguments before the usage arguments.
    pub executable_args: Vec<String>,
    /// Extra environment for the spawned child (fixture seam).
    pub spawn_env: Vec<(String, String)>,
    /// Caps the whole spawn-read-wait sequence.
    pub timeout: Duration,
    /// Maximum stdout/stderr bytes accepted per stream.
    pub max_bytes: usize,
}

impl ClaudeUsageConfig {
    /// Creates a read configuration with the documented default bounds.
    #[must_use]
    pub fn new(executable: PathBuf) -> Self {
        Self {
            executable,
            executable_args: Vec::new(),
            spawn_env: Vec::new(),
            timeout: CLAUDE_USAGE_TIMEOUT,
            max_bytes: CLAUDE_USAGE_MAX_BYTES,
        }
    }
}

/// Reads Claude quota windows exclusively through the external CLI.
///
/// # Errors
///
/// Returns [`UsageReaderError`] for spawn, deadline, bound, exit, or
/// malformed-payload failures, including a successful run whose result text
/// carries no usable window.
pub fn read_claude_usage(config: &ClaudeUsageConfig) -> Result<ProviderUsage, UsageReaderError> {
    let deadline = Instant::now() + config.timeout;
    let mut child = Command::new(&config.executable)
        .args(&config.executable_args)
        .args(CLAUDE_USAGE_ARGS)
        .envs(config.spawn_env.iter().map(|(key, value)| (key, value)))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| UsageReaderError::Spawn)?;
    // `-p` reads its prompt from stdin; the slash command is already in
    // argv, so EOF goes immediately instead of paying the CLI grace period.
    drop(child.stdin.take());
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let max_bytes = config.max_bytes;
    let (sender, receiver) = mpsc::channel();
    let stdout_reader = match thread::Builder::new()
        .name("claude-usage-stdout".to_owned())
        .spawn(move || {
            let mut bytes = Vec::new();
            let outcome = read_bounded(stdout.as_mut(), &mut bytes, max_bytes);
            let _send_result = sender.send((outcome, bytes));
        }) {
        Ok(handle) => handle,
        Err(_) => {
            let _kill_result = child.kill();
            let _reap_result = child.wait();
            return Err(UsageReaderError::Spawn);
        }
    };
    let stderr_drain = match thread::Builder::new()
        .name("claude-usage-stderr".to_owned())
        .spawn(move || {
            let mut sink = Vec::new();
            read_bounded(stderr.as_mut(), &mut sink, max_bytes);
        }) {
        Ok(handle) => handle,
        Err(_) => {
            let _kill_result = child.kill();
            let _reap_result = child.wait();
            let _join_result = stdout_reader.join();
            return Err(UsageReaderError::Spawn);
        }
    };
    let status = loop {
        match child.try_wait().map_err(|_| UsageReaderError::Closed)? {
            Some(status) => break status,
            None => {
                if Instant::now() >= deadline {
                    let _kill_result = child.kill();
                    let _reap_result = child.wait();
                    let _join_result = stderr_drain.join();
                    return Err(UsageReaderError::Timeout);
                }
                thread::sleep(Duration::from_millis(10));
            }
        }
    };
    let _join_result = stderr_drain.join();
    if !status.success() {
        return Err(UsageReaderError::ExitStatus);
    }
    let remaining = deadline.saturating_duration_since(Instant::now());
    let (outcome, stdout_bytes) = receiver
        .recv_timeout(remaining)
        .map_err(|_| UsageReaderError::Timeout)?;
    outcome?;
    // The CLI's contract is one JSON object on stdout; slice it out of any
    // surrounding banner text (version notices in production, harness
    // preamble in fixtures) instead of failing on the first foreign byte.
    let response = String::from_utf8(stdout_bytes).map_err(|_| UsageReaderError::Malformed)?;
    let json = extract_json_object(&response).ok_or(UsageReaderError::Malformed)?;
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|_| UsageReaderError::Malformed)?;
    let result = value
        .get("result")
        .and_then(serde_json::Value::as_str)
        .ok_or(UsageReaderError::Malformed)?;
    let at_ms = system_millis();
    let windows = parse_claude_usage_windows(result, at_ms);
    if windows.is_empty() {
        return Err(UsageReaderError::Empty);
    }
    Ok(ProviderUsage::authenticated(windows))
}

fn read_bounded(
    stream: Option<&mut impl Read>,
    into: &mut Vec<u8>,
    maximum: usize,
) -> Result<(), UsageReaderError> {
    let Some(stream) = stream else {
        return Ok(());
    };
    let mut chunk = [0_u8; 8_192];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => return Ok(()),
            Ok(read) => {
                if into.len().saturating_add(read) > maximum {
                    return Err(UsageReaderError::TooLarge);
                }
                into.extend_from_slice(&chunk[..read]);
            }
            Err(_) => return Err(UsageReaderError::Closed),
        }
    }
}

fn system_millis() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

/// Slices the outermost JSON object out of surrounding banner text.
///
/// Returns the span from the first `{` to the last `}` when both exist in
/// order; otherwise `None`. The CLI contract is one object, so a missing
/// span is malformed rather than empty.
fn extract_json_object(output: &str) -> Option<&str> {
    let start = output.find('{')?;
    let end = output.rfind('}')?;
    if end < start {
        return None;
    }
    output.get(start..=end)
}

fn slugify_label(label: &str) -> String {
    let mut slug = String::new();
    let mut previous_dash = true;
    for character in label.trim().to_lowercase().chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character);
            previous_dash = false;
        } else if !previous_dash {
            slug.push('-');
            previous_dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    slug
}

fn claude_month_number(name: &str) -> Option<u32> {
    match name.get(..3)?.to_lowercase().as_str() {
        "jan" => Some(1),
        "feb" => Some(2),
        "mar" => Some(3),
        "apr" => Some(4),
        "may" => Some(5),
        "jun" => Some(6),
        "jul" => Some(7),
        "aug" => Some(8),
        "sep" => Some(9),
        "oct" => Some(10),
        "nov" => Some(11),
        "dec" => Some(12),
        _ => None,
    }
}

fn is_utc_zone(zone: &str) -> bool {
    matches!(
        zone.to_lowercase().as_str(),
        "utc" | "gmt" | "ut" | "z" | "etc/utc" | "etc/gmt" | "etc/ut" | "etc/zulu"
    )
}

/// Converts a Claude wall-clock reset clause to an ISO instant.
///
/// Only UTC-equivalent zones resolve: this build carries no IANA timezone
/// database, so any other zone yields `None` rather than a guessed instant.
/// The year follows the TypeScript adapter: the current UTC year, rolling to
/// the next year when now is December and the reset month is January.
fn parse_reset_at(line: &str, at_ms: i64) -> Option<String> {
    // ASCII-only search keeps the byte index valid for slicing the original
    // line even when earlier text contains multi-byte characters.
    let resets = line
        .as_bytes()
        .windows("resets".len())
        .rposition(|window| window.eq_ignore_ascii_case(b"resets"))?;
    let clause = line[resets + "resets".len()..].trim();
    let open = clause.rfind('(')?;
    let close = clause.rfind(')')?;
    if close != clause.len() - 1 {
        return None;
    }
    if !is_utc_zone(clause[open + 1..close].trim()) {
        return None;
    }
    let datetime = clause[..open].trim();
    let comma = datetime.find(',')?;
    let date_part = datetime[..comma].trim();
    let time_part = datetime[comma + 1..].trim();
    let mut date_words = date_part.split_whitespace();
    let month = claude_month_number(date_words.next()?)?;
    let day: u32 = date_words.next()?.parse().ok()?;
    if date_words.next().is_some() {
        return None;
    }
    let clock_lower = time_part.to_lowercase();
    let (clock, afternoon) = if let Some(clock) = clock_lower.strip_suffix("pm") {
        (clock, true)
    } else if let Some(clock) = clock_lower.strip_suffix("am") {
        (clock, false)
    } else {
        return None;
    };
    let clock = clock.trim();
    let (hour_text, minute) = match clock.split_once(':') {
        Some((hour, minute)) if minute.len() == 2 => {
            (hour.trim(), Some(minute.parse::<u32>().ok()?))
        }
        Some(_) => return None,
        None => (clock, None),
    };
    if hour_text.len() > 2 || hour_text.is_empty() {
        return None;
    }
    let hour12: u32 = hour_text.parse().ok()?;
    let minute = minute.unwrap_or(0);
    if !(1..=31).contains(&day) || !(1..=12).contains(&hour12) || minute > 59 {
        return None;
    }
    let hour = (hour12 % 12) + if afternoon { 12 } else { 0 };
    let (mut year, now_month, _) = utc_ymd(at_ms);
    if now_month == 12 && month == 1 {
        year += 1;
    }
    let days = days_from_civil(year, month, day)?;
    let millis = days
        .checked_mul(86_400_000)?
        .checked_add(i64::from(hour) * 3_600_000 + i64::from(minute) * 60_000)?;
    Some(artisan_domain::iso_millis(millis))
}

fn days_from_civil(year: i64, month: u32, day: u32) -> Option<i64> {
    if month == 0 || month > 12 || day == 0 || day > 31 {
        return None;
    }
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let february = if leap { 29 } else { 28 };
    let month_days = [31, february, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if day > month_days[(month - 1) as usize] {
        return None;
    }
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let day_of_year =
        (153 * (i64::from(month) + if month > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some(era * 146_097 + day_of_era - 719_468)
}

fn push_window(
    windows: &mut Vec<EngineUsageWindow>,
    id: String,
    kind: EngineUsageWindowKind,
    label: Option<String>,
    percent: f64,
    resets_at: Option<String>,
    window_minutes: u32,
) {
    if windows.iter().any(|window| window.id() == id) {
        return;
    }
    let Ok(percent) = clamp_percent_used(percent) else {
        return;
    };
    if let Ok(window) =
        EngineUsageWindow::new(id, kind, label, percent, resets_at, Some(window_minutes))
    {
        windows.push(window);
    }
}

/// Parses provider-owned CLI text without retaining credentials.
///
/// Recognizes the `Current session` line, the `Current week (all models)`
/// line, and per-model `Current week (<Label>)` lines, each with an optional
/// trailing reset clause. Pure and free of process I/O.
pub fn parse_claude_usage_windows(result_text: &str, at_ms: i64) -> Vec<EngineUsageWindow> {
    let mut windows = Vec::new();
    for raw_line in result_text.lines() {
        let line = raw_line.trim();
        if let Some(percent) = match_session_line(line) {
            push_window(
                &mut windows,
                "five_hour".to_owned(),
                EngineUsageWindowKind::Session,
                None,
                percent,
                parse_reset_at(line, at_ms),
                300,
            );
            continue;
        }
        if let Some(percent) = match_weekly_all_line(line) {
            push_window(
                &mut windows,
                "seven_day".to_owned(),
                EngineUsageWindowKind::Weekly,
                None,
                percent,
                parse_reset_at(line, at_ms),
                10_080,
            );
            continue;
        }
        if let Some((label, percent)) = match_weekly_labeled_line(line) {
            let slug = slugify_label(&label);
            if slug.is_empty() {
                continue;
            }
            push_window(
                &mut windows,
                format!("seven_day:{slug}"),
                EngineUsageWindowKind::Weekly,
                Some(label),
                percent,
                parse_reset_at(line, at_ms),
                10_080,
            );
        }
    }
    windows
}

fn match_session_line(line: &str) -> Option<f64> {
    percent_after_prefix(line, "Current session:")
}

fn match_weekly_all_line(line: &str) -> Option<f64> {
    percent_after_prefix(line, "Current week (all models):")
}

/// Matches `<integer>% used` with a word boundary after `used`, mirroring
/// the TypeScript `(\d+)%\s*used\b` line patterns.
fn percent_after_prefix(line: &str, prefix: &str) -> Option<f64> {
    let rest = line.strip_prefix(prefix)?.trim();
    let percent_end = rest.find('%')?;
    let digits = rest[..percent_end].trim();
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let percent: u32 = digits.parse().ok()?;
    let after = rest[percent_end + 1..].trim_start();
    let tail = after.strip_prefix("used")?;
    if tail
        .chars()
        .next()
        .is_some_and(|next| next.is_alphanumeric())
    {
        return None;
    }
    Some(f64::from(percent))
}

fn match_weekly_labeled_line(line: &str) -> Option<(String, f64)> {
    let rest = line.strip_prefix("Current week (")?;
    let close = rest.find(')')?;
    let label = rest[..close].trim().to_owned();
    if label.is_empty() || label == "all models" {
        return None;
    }
    let after = rest[close + 1..].trim();
    let after = after.strip_prefix(':')?.trim();
    let percent_end = after.find('%')?;
    let digits = after[..percent_end].trim();
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let percent: u32 = digits.parse().ok()?;
    let tail = after[percent_end + 1..].trim_start().strip_prefix("used")?;
    if tail
        .chars()
        .next()
        .is_some_and(|next| next.is_alphanumeric())
    {
        return None;
    }
    Some((label, f64::from(percent)))
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-09-09T12:00:00Z.
    const AT_MS: i64 = 1_788_955_200_000;

    #[test]
    fn session_and_weekly_lines_parse_with_reset_clauses() {
        let text = "Current session: 42% used, resets Sept 9, 5pm (UTC)\n\
            Current week (all models): 17% used, resets Sept 14, 5pm (UTC)\n\
            Current week (Fable): 63% used";
        let windows = parse_claude_usage_windows(text, AT_MS);
        assert_eq!(windows.len(), 3);
        assert_eq!(windows[0].id(), "five_hour");
        assert_eq!(windows[0].kind(), EngineUsageWindowKind::Session);
        assert_eq!(windows[0].percent_used(), 42.0);
        assert_eq!(windows[0].resets_at(), Some("2026-09-09T17:00:00Z"));
        assert_eq!(windows[1].id(), "seven_day");
        assert_eq!(windows[1].percent_used(), 17.0);
        assert_eq!(windows[1].resets_at(), Some("2026-09-14T17:00:00Z"));
        assert_eq!(windows[2].id(), "seven_day:fable");
        assert_eq!(windows[2].label(), Some("Fable"));
        assert_eq!(windows[2].resets_at(), None);
    }

    #[test]
    fn reset_clauses_require_utc_and_valid_fields() {
        assert_eq!(
            parse_reset_at(
                "Current session: 1% used, resets Dec 31, 11:30pm (UTC)",
                AT_MS
            ),
            Some("2026-12-31T23:30:00Z".to_owned())
        );
        assert_eq!(
            parse_reset_at(
                "Current session: 1% used, resets Jan 2, 1am (UTC)",
                1_767_225_600_000
            ),
            Some("2026-01-02T01:00:00Z".to_owned())
        );
        // Non-UTC zones resolve to no instant rather than a guessed one.
        assert_eq!(
            parse_reset_at(
                "Current session: 1% used, resets Sept 9, 5pm (Europe/Oslo)",
                AT_MS
            ),
            None
        );
        assert_eq!(
            parse_reset_at("Current session: 1% used, resets Sept 9, 5pm", AT_MS),
            None
        );
        assert_eq!(
            parse_reset_at("Current session: 1% used, resets Foo 9, 5pm (UTC)", AT_MS),
            None
        );
        assert_eq!(
            parse_reset_at("Current session: 1% used, resets Sept 9, 13pm (UTC)", AT_MS),
            None
        );
    }

    #[test]
    fn malformed_and_duplicate_lines_are_skipped() {
        let text = "Hello\nCurrent session: lots used\nCurrent week (): 5% used\n\
            Current session: 10% used\nCurrent session: 20% used\n";
        let windows = parse_claude_usage_windows(text, AT_MS);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].percent_used(), 10.0);
        assert!(parse_claude_usage_windows("", AT_MS).is_empty());
        assert_eq!(slugify_label("Claude  Opus 4.1!"), "claude-opus-4-1");
    }
}
