//! Port of `modules/frontend/src/lib/conversation/relative-time.ts`.
//!
//! The TypeScript source formats the elapsed time between a reference `now`
//! (milliseconds since the Unix epoch) and an ISO timestamp as a short
//! relative label such as `5m ago`. This module preserves the exact
//! thresholds, floor rounding, future clamping, and invalid handling, and
//! the exact labels `s`, `m`, `h`, `d`, `w`, `mo`, `y` suffixed with
//! ` ago`.
//!
//! Differences from the TypeScript implementation are intentionally limited
//! to the type boundary required by Rust:
//!
//! - `now` is an explicit `i64` Unix millisecond input; no global clock is
//!   read.
//! - `timestamp` remains a string; `Date.parse` failure maps to elapsed `0`
//!   and thus `"0s ago"`.
//! - Future timestamps (negative elapsed) clamp to `0` and also render as
//!   `"0s ago"`.
//!
//! The parser accepts the ISO 8601 subset produced by the application
//! (`YYYY-MM-DDTHH:MM:SS[.sss]Z` with an explicit `Z` or numeric offset, plus
//! date-only `YYYY-MM-DD` as UTC midnight). Strings with no timezone on a
//! date-time are treated as invalid to avoid host-local timezone dependence,
//! which matches the application's always-UTC output and keeps the function
//! pure.

/// Formats the elapsed time between `now_millis` and `timestamp` as a short
/// relative age string.
///
/// `now_millis` is milliseconds since the Unix epoch and `timestamp` is an
/// ISO 8601 string parsed with the same success/failure semantics as
/// `Date.parse` in the TypeScript source. Invalid or future timestamps
/// produce `"0s ago"`. Thresholds and suffixes match the TypeScript exactly:
///
/// - `< 60s` → `"{s}s ago"`
/// - `< 60m` → `"{m}m ago"`
/// - `< 24h` → `"{h}h ago"`
/// - `< 7d`  → `"{d}d ago"`
/// - `< 5w`  → `"{w}w ago"`
/// - `< 12mo`→ `"{mo}mo ago"` (`mo` uses `days / 30`)
/// - otherwise → `"{y}y ago"` (`y` uses `days / 365`)
#[must_use]
pub fn format_relative_age(now_millis: i64, timestamp: &str) -> String {
    let elapsed_seconds = elapsed_seconds(now_millis, timestamp);
    if elapsed_seconds < 60 {
        return format!("{elapsed_seconds}s ago");
    }
    let elapsed_minutes = elapsed_seconds / 60;
    if elapsed_minutes < 60 {
        return format!("{elapsed_minutes}m ago");
    }
    let elapsed_hours = elapsed_minutes / 60;
    if elapsed_hours < 24 {
        return format!("{elapsed_hours}h ago");
    }
    let elapsed_days = elapsed_hours / 24;
    if elapsed_days < 7 {
        return format!("{elapsed_days}d ago");
    }
    let elapsed_weeks = elapsed_days / 7;
    if elapsed_weeks < 5 {
        return format!("{elapsed_weeks}w ago");
    }
    let elapsed_months = elapsed_days / 30;
    if elapsed_months < 12 {
        return format!("{elapsed_months}mo ago");
    }
    let elapsed_years = elapsed_days / 365;
    format!("{elapsed_years}y ago")
}

fn elapsed_seconds(now_millis: i64, timestamp: &str) -> u64 {
    let Some(timestamp_millis) = parse_timestamp_millis(timestamp) else {
        return 0;
    };
    if now_millis <= timestamp_millis {
        return 0;
    }
    let diff = i128::from(now_millis) - i128::from(timestamp_millis);
    // `diff` is positive here; floor-divide by 1000 to mirror
    // `Math.floor((now - ts) / 1000)` in the TS source.
    let seconds = diff / 1000;
    if seconds <= i128::from(u64::MAX) {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let seconds = seconds as u64;
        seconds
    } else {
        u64::MAX
    }
}

/// Parses an ISO 8601 timestamp the way `Date.parse` does for the
/// application's UTC output. Returns `None` for invalid input.
fn parse_timestamp_millis(input: &str) -> Option<i64> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    // Date-only form `YYYY-MM-DD` – treated as UTC midnight per ES spec for
    // ISO date-only strings.
    if is_date_only(trimmed) {
        let (year, month, day) = parse_date(trimmed)?;
        return millis_from_civil(year, month, day, 0, 0, 0, 0, 0);
    }

    // Date-time form with `T`/`t` separator.
    let separator = trimmed.find(['T', 't'])?;
    let date_part = &trimmed[..separator];
    let time_and_zone = &trimmed[separator + 1..];
    if time_and_zone.is_empty() {
        return None;
    }

    let (year, month, day) = parse_date(date_part)?;

    // Split timezone from time.
    let (time_part, offset_minutes) = split_time_and_offset(time_and_zone)?;

    let (hour, minute, second, millis) = parse_time(time_part)?;

    millis_from_civil(
        year,
        month,
        day,
        hour,
        minute,
        second,
        millis,
        offset_minutes,
    )
}

fn is_date_only(value: &str) -> bool {
    // Cheap check: length 10, dashes at 4 and 7, no `T`/`t`, no `:`.
    value.len() == 10
        && value.as_bytes().get(4) == Some(&b'-')
        && value.as_bytes().get(7) == Some(&b'-')
        && !value.contains('T')
        && !value.contains('t')
        && !value.contains(':')
}

fn parse_date(value: &str) -> Option<(i32, u8, u8)> {
    let mut parts = value.split('-');
    let year_str = parts.next()?;
    let month_str = parts.next()?;
    let day_str = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let year: i32 = year_str.parse().ok()?;
    let month: u8 = month_str.parse().ok()?;
    let day: u8 = day_str.parse().ok()?;
    if month == 0 || month > 12 || day == 0 || day > 31 {
        return None;
    }
    if day > days_in_month(year, month) {
        return None;
    }
    Some((year, month, day))
}

fn split_time_and_offset(value: &str) -> Option<(&str, i32)> {
    if value.is_empty() {
        return None;
    }
    // Trailing `Z`/`z` → UTC.
    if value.ends_with(['Z', 'z']) {
        let time = &value[..value.len() - 1];
        if time.is_empty() {
            return None;
        }
        return Some((time, 0));
    }
    // Look for final `+`/`-` offset indicator. The `+`/`-` must occur after
    // the time component, so we search in the full zone suffix.
    // Time always contains `:`, so offset sign must be after at least 2 chars.
    let plus = value.rfind('+');
    let minus = value.rfind('-');
    let offset_start = match (plus, minus) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };
    let Some(start) = offset_start else {
        // No timezone – treat as invalid to avoid local-time dependence.
        return None;
    };
    let time = &value[..start];
    let zone = &value[start..];
    if time.is_empty() || zone.is_empty() {
        return None;
    }
    let offset = parse_offset(zone)?;
    Some((time, offset))
}

#[allow(clippy::manual_range_contains)]
fn parse_offset(value: &str) -> Option<i32> {
    // `+HH:MM`, `-HH:MM`, `+HHMM`, `-HHMM`, `+HH`, `-HH`
    let sign: i32 = match value.as_bytes().first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let body = &value[1..];
    if body.is_empty() {
        return None;
    }
    if body.contains(':') {
        let mut parts = body.split(':');
        let hour_str = parts.next()?;
        let minute_str = parts.next()?;
        if parts.next().is_some() {
            return None;
        }
        if hour_str.len() != 2 || minute_str.len() != 2 {
            return None;
        }
        let hours: i32 = hour_str.parse().ok()?;
        let minutes: i32 = minute_str.parse().ok()?;
        if hours < 0 || hours > 23 || minutes < 0 || minutes > 59 {
            return None;
        }
        Some(sign * (hours * 60 + minutes))
    } else if body.len() == 2 {
        let hours: i32 = body.parse().ok()?;
        if hours < 0 || hours > 23 {
            return None;
        }
        Some(sign * hours * 60)
    } else if body.len() == 4 {
        let hour_str = &body[..2];
        let minute_str = &body[2..];
        let hours: i32 = hour_str.parse().ok()?;
        let minutes: i32 = minute_str.parse().ok()?;
        if hours < 0 || hours > 23 || minutes < 0 || minutes > 59 {
            return None;
        }
        Some(sign * (hours * 60 + minutes))
    } else {
        None
    }
}

fn parse_time(value: &str) -> Option<(u8, u8, u8, u16)> {
    // `HH:MM:SS[.sss]` or `HH:MM` (seconds default 0, millis default 0)
    let (main, fraction) = match value.split_once('.') {
        Some((left, right)) => (left, Some(right)),
        None => (value, None),
    };

    let millis = if let Some(frac) = fraction {
        // `Date.parse` millisecond fraction is up to 3 digits; extra digits
        // are truncated. Non-digit fraction is invalid.
        if frac.is_empty() || !frac.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let truncated = if frac.len() > 3 { &frac[..3] } else { frac };
        let mut padded = String::from(truncated);
        while padded.len() < 3 {
            padded.push('0');
        }
        padded.parse::<u16>().ok()?
    } else {
        0
    };

    let mut parts = main.split(':');
    let hour_str = parts.next()?;
    let minute_str = parts.next()?;
    let second_str = parts.next();

    if parts.next().is_some() {
        return None;
    }

    let hour: u8 = hour_str.parse().ok()?;
    let minute: u8 = minute_str.parse().ok()?;
    if hour > 23 || minute > 59 {
        return None;
    }

    let second: u8 = if let Some(second_text) = second_str {
        let parsed: u8 = second_text.parse().ok()?;
        if parsed > 59 {
            return None;
        }
        parsed
    } else {
        0
    };

    Some((hour, minute, second, millis))
}

const fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if is_leap_year(year) {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

const fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

#[allow(clippy::too_many_arguments)]
fn millis_from_civil(
    year: i32,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
    millis: u16,
    offset_minutes: i32,
) -> Option<i64> {
    let days = days_from_civil(year, u32::from(month), u32::from(day));
    let days_millis = i128::from(days) * 86_400_000;
    let time_millis = i128::from(hour) * 3_600_000
        + i128::from(minute) * 60_000
        + i128::from(second) * 1_000
        + i128::from(millis);
    let offset_millis = i128::from(offset_minutes) * 60_000;
    let total = days_millis + time_millis - offset_millis;
    if total < i128::from(i64::MIN) || total > i128::from(i64::MAX) {
        return None;
    }
    #[allow(clippy::cast_possible_truncation)]
    let total = total as i64;
    Some(total)
}

/// Days since 1970-01-01 (Unix epoch) using Howard Hinnant's algorithm.
fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let month_i64 = i64::from(month);
    let year_adj = i64::from(year) - i64::from(u32::from(month <= 2));
    let era = if year_adj >= 0 {
        year_adj
    } else {
        year_adj - 399
    } / 400;
    let year_of_era = year_adj - era * 400;
    let month_adj = if month > 2 {
        month_i64 - 3
    } else {
        month_i64 + 9
    };
    let day_of_year = (153 * month_adj + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}
