//! Claude `/usage` quota diagnostics: window classification, reset parsing,
//! and CLI text parsing. Test-only until the diagnostics consumer lands;
//! quota windows never become usage reports and never gate a turn.

/// Kind of one Claude quota window, classified from its provider window.
///
/// Mirrors `parse_claude_cli_usage_windows` in
/// `modules/engines/src/claude/usage.ts`: 300 minutes is a session window,
/// 10,080 a weekly window; anything else is unknown rather than guessed.
/// Claude names no monthly bucket.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ClaudeQuotaWindowKind {
    Session,
    Weekly,
    Unknown,
}

/// Classifies one quota window duration.
pub(crate) fn classify_claude_quota_window_kind(
    window_minutes: Option<u64>,
) -> ClaudeQuotaWindowKind {
    match window_minutes {
        Some(300) => ClaudeQuotaWindowKind::Session,
        Some(10_080) => ClaudeQuotaWindowKind::Weekly,
        _ => ClaudeQuotaWindowKind::Unknown,
    }
}

/// Clamps one percent reading into `0..=100`.
///
/// Absent or non-finite readings become `0`: usage display never blocks on a
/// corrupt gauge and never invents quota from it.
pub(crate) fn clamp_claude_percent_used(used_percent: Option<f64>) -> f64 {
    match used_percent {
        Some(value) if value.is_finite() => value.clamp(0.0, 100.0),
        _ => 0.0,
    }
}

/// The exact non-billable CLI invocation that reads account usage.
///
/// Mirrors `claude_cli_usage_args` in `modules/engines/src/claude/usage.ts`
/// (`-p /usage` over JSON): the slash command travels in argv and stdin
/// closes immediately, so no prompt is ever billed.
pub(crate) fn claude_cli_usage_args() -> [&'static str; 4] {
    ["-p", "/usage", "--output-format", "json"]
}

/// One provider-neutral Claude quota window: diagnostics only, never quota.
///
/// Quota windows are read through the non-billable `/usage` surface and
/// classified here; they are never copied into [`RunUsageReport`] and never
/// gate a turn.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ClaudeQuotaWindow {
    pub id: String,
    pub kind: ClaudeQuotaWindowKind,
    pub label: Option<String>,
    pub percent_used: f64,
    pub resets_at: Option<String>,
    pub window_minutes: Option<u64>,
    /// `"shared"` for the session and all-models weekly buckets, `"model"`
    /// for per-model weekly buckets. Mirrors the TypeScript scope rule
    /// without inventing quota attribution.
    pub scope: &'static str,
}

/// Turns a provider-supplied weekly label into a stable id fragment.
///
/// Mirrors `slugify_claude_cli_label` in `modules/engines/src/claude/usage.ts`:
/// lowercase, non-alphanumeric runs become one dash, edge dashes trimmed.
pub(crate) fn slugify_claude_cli_label(label: &str) -> String {
    let mut slug = String::with_capacity(label.len());
    let mut pending_dash = false;
    for character in label.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            if pending_dash && !slug.is_empty() {
                slug.push('-');
            }
            pending_dash = false;
            slug.push(character);
        } else if !slug.is_empty() || pending_dash {
            pending_dash = true;
        }
    }
    slug
}

const CLAUDE_CLI_MONTHS: [(&str, i64); 12] = [
    ("jan", 1),
    ("feb", 2),
    ("mar", 3),
    ("apr", 4),
    ("may", 5),
    ("jun", 6),
    ("jul", 7),
    ("aug", 8),
    ("sep", 9),
    ("oct", 10),
    ("nov", 11),
    ("dec", 12),
];

fn claude_cli_month(name: &str) -> Option<i64> {
    let prefix: String = name.chars().take(3).flat_map(char::to_lowercase).collect();
    CLAUDE_CLI_MONTHS
        .iter()
        .find(|(month, _)| *month == prefix)
        .map(|(_, number)| *number)
}

/// Converts a civil date to days since the Unix epoch (Howard Hinnant's
/// algorithm), mirroring the epoch math behind the TypeScript reset parse
/// without a date dependency.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let adjusted_year = if month <= 2 { year - 1 } else { year };
    let era = adjusted_year.div_euclid(400);
    let year_of_era = adjusted_year.rem_euclid(400);
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Converts days since the Unix epoch back to a civil date.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_pair = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_pair + 2) / 5 + 1;
    let month = if month_pair < 10 {
        month_pair + 3
    } else {
        month_pair - 9
    };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// Parses the provider's English wall-clock reset clause to a UTC instant.
///
/// Mirrors `parse_claude_cli_reset_at` in `modules/engines/src/claude/usage.ts`
/// (`resets <Mon> <D>, <H>[:<MM>] <am|pm> (<Zone>)` at end of line). Only
/// `UTC`/`GMT`/`UT` zones resolve to a real instant: named IANA zones need a
/// zone database this owner does not carry, so they stay `None` rather than
/// becoming an invented instant. `at_ms` is the caller-observed now used for
/// year inference (December rolling into a January reset).
pub(crate) fn parse_claude_cli_reset_at(line: &str, at_ms: i64) -> Option<String> {
    // Scan left to right for the first `resets <clause>` that fully parses,
    // mirroring the unanchored regex scan in the TypeScript reader. The scan
    // is ASCII-boundary safe: `resets ` is pure ASCII and the cursor always
    // rests on a character boundary.
    let bytes = line.as_bytes();
    let mut index = 0;
    while index + 7 <= bytes.len() {
        if bytes[index..index + 7].eq_ignore_ascii_case(b"resets ")
            && (index == 0 || !bytes[index - 1].is_ascii_alphanumeric())
            && let Some(instant) = parse_reset_clause(&line[index + 7..], at_ms)
        {
            return Some(instant);
        }
        index += line[index..].chars().next()?.len_utf8();
    }
    None
}

fn parse_reset_clause(clause: &str, at_ms: i64) -> Option<String> {
    let clause = clause.trim_end();
    let (date_part, zone_part) = clause.rsplit_once('(')?;
    let zone = zone_part.strip_suffix(')')?;
    if !zone.eq_ignore_ascii_case("UTC")
        && !zone.eq_ignore_ascii_case("GMT")
        && !zone.eq_ignore_ascii_case("UT")
    {
        return None;
    }
    let mut tokens = date_part.split_whitespace();
    let month_token = tokens.next()?;
    if !month_token
        .chars()
        .all(|character| character.is_ascii_alphabetic())
    {
        return None;
    }
    let day_token = tokens.next()?.strip_suffix(',')?;
    let month = claude_cli_month(month_token)?;
    let day: i64 = day_token.parse().ok()?;
    let (hour_token, minute_token, meridiem_token) = match (tokens.next(), tokens.next()) {
        (Some(time), Some(meridiem)) => {
            let (hour, minute) = match time.split_once(':') {
                Some((hour, minute)) => {
                    if minute.len() != 2 {
                        return None;
                    }
                    (hour, Some(minute))
                }
                None => (time, None),
            };
            (hour, minute, meridiem)
        }
        _ => return None,
    };
    if tokens.next().is_some() {
        return None;
    }
    let hour12: i64 = hour_token.parse().ok()?;
    let minute: i64 = match minute_token {
        Some(text) => text.parse().ok()?,
        None => 0,
    };
    let meridiem = meridiem_token.to_ascii_lowercase();
    if !(1..=31).contains(&day) || !(1..=12).contains(&hour12) || !(0..=59).contains(&minute) {
        return None;
    }
    let hour = match meridiem.as_str() {
        "am" => hour12 % 12,
        "pm" => hour12 % 12 + 12,
        _ => return None,
    };
    let (current_year, current_month, _) = civil_from_days(at_ms.div_euclid(86_400_000));
    let year = current_year + i64::from(current_month == 12 && month == 1);
    if civil_from_days(days_from_civil(year, month, day)) != (year, month, day) {
        return None;
    }
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:00Z"
    ))
}

fn claude_usage_bucket_window(
    id: String,
    label: Option<String>,
    percent_text: &str,
    line: &str,
    at_ms: i64,
    scope: &'static str,
    window_minutes: u64,
) -> ClaudeQuotaWindow {
    ClaudeQuotaWindow {
        id,
        kind: classify_claude_quota_window_kind(Some(window_minutes)),
        label,
        percent_used: clamp_claude_percent_used(percent_text.parse::<f64>().ok()),
        resets_at: parse_claude_cli_reset_at(line, at_ms),
        window_minutes: Some(window_minutes),
        scope,
    }
}

fn match_percent_tail(text: &str) -> Option<&str> {
    let end = text
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(text.len());
    if end == 0 {
        return None;
    }
    let (digits, rest) = (&text[..end], &text[end..]);
    let rest = rest.strip_prefix('%')?;
    let rest = rest.trim_start();
    let after_used = rest.strip_prefix("used")?;
    if after_used
        .chars()
        .next()
        .is_some_and(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        return None;
    }
    Some(digits)
}

/// Parses provider-owned `/usage` CLI text into quota windows.
///
/// Mirrors `parse_claude_cli_usage_windows` in
/// `modules/engines/src/claude/usage.ts` without retaining credentials or raw
/// account data: one `five_hour` session window, one shared `seven_day`
/// weekly window, plus one per-model `seven_day:<slug>` weekly window.
/// Duplicate ids keep the first row; malformed lines yield nothing.
pub(crate) fn parse_claude_cli_usage_windows(
    result_text: &str,
    at_ms: i64,
) -> Vec<ClaudeQuotaWindow> {
    let mut windows: Vec<ClaudeQuotaWindow> = Vec::new();
    for raw_line in result_text.split('\n') {
        let line = raw_line.trim();
        if let Some(rest) = line.strip_prefix("Current session:") {
            let rest = rest.trim_start();
            if let Some(percent) = match_percent_tail(rest) {
                push_quota_window(
                    &mut windows,
                    claude_usage_bucket_window(
                        "five_hour".to_owned(),
                        None,
                        percent,
                        line,
                        at_ms,
                        "shared",
                        300,
                    ),
                );
            }
            continue;
        }
        // The all-models bucket is checked before the labeled pattern below.
        if let Some(rest) = line.strip_prefix("Current week (all models):") {
            let rest = rest.trim_start();
            if let Some(percent) = match_percent_tail(rest) {
                push_quota_window(
                    &mut windows,
                    claude_usage_bucket_window(
                        "seven_day".to_owned(),
                        None,
                        percent,
                        line,
                        at_ms,
                        "shared",
                        10_080,
                    ),
                );
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("Current week (") {
            let Some(close) = rest.find(')') else {
                continue;
            };
            // The labeled shape requires the exact `(<Label>): N% used`
            // form: the colon follows the parenthesis immediately.
            let label = &rest[..close];
            if label.is_empty() {
                continue;
            }
            let Some(after) = rest[close + 1..].strip_prefix(':') else {
                continue;
            };
            let after = after.trim_start();
            if let Some(percent) = match_percent_tail(after) {
                let slug = slugify_claude_cli_label(label);
                push_quota_window(
                    &mut windows,
                    claude_usage_bucket_window(
                        format!("seven_day:{slug}"),
                        Some(label.to_owned()),
                        percent,
                        line,
                        at_ms,
                        "model",
                        10_080,
                    ),
                );
            }
        }
    }
    windows
}

fn push_quota_window(windows: &mut Vec<ClaudeQuotaWindow>, window: ClaudeQuotaWindow) {
    if windows.iter().any(|known| known.id == window.id) {
        return;
    }
    windows.push(window);
}
