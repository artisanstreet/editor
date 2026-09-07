//! Conversation duration formatting policy.
//!
//! Native Rust port of `modules/frontend/src/lib/conversation/duration.ts`.
//! The TypeScript source formats every elapsed span so seconds are always
//! present, minutes appear when they are non-zero or when hours are present
//! (so `1h 0m 3s` never reads as an hour and three minutes), negative and
//! non-finite inputs floor at zero rather than rendering a backwards clock,
//! and ISO timestamp pairs are differenced before formatting.
//!
//! This module preserves those exact thresholds, rounding (`Math.floor` after
//! dividing by `1_000`), unknown/invalid handling, and output text semantics
//! with explicit typed inputs, deterministic integer arithmetic, and no
//! wall-clock, global, or DOM state. Adjacent conversation projection is
//! intentionally out of scope; this is a pure formatting policy.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

/// Formats an elapsed span as transcript duration text.
///
/// Mirrors `FormatElapsed` in `duration.ts`:
///
/// * `elapsed_ms` is divided by `1_000` and floored. Non-finite (`NaN`,
///   `Infinity`) values floor to `0` seconds, and negative values are clamped
///   to `0` before decomposition.
/// * Hours are `floor(total_seconds / 3_600)`, minutes are
///   `floor((total_seconds % 3_600) / 60)`, seconds are `total_seconds % 60`.
/// * Output always contains seconds (`"0s"` … `"59s"`). Minutes appear when
///   `minutes > 0` or `hours > 0`. Hours appear when `hours > 0`. Parts are
///   joined with a single space.
///
/// # Examples
///
/// ```rust
/// use artisan_frontend::conversation_duration::{format_duration, format_elapsed};
///
/// assert_eq!(format_elapsed(0.0), "0s");
/// assert_eq!(format_elapsed(1_000.0), "1s");
/// assert_eq!(format_elapsed(60_000.0), "1m 0s");
/// assert_eq!(format_elapsed(3_600_000.0), "1h 0m 0s");
/// assert_eq!(format_elapsed(3_663_000.0), "1h 1m 3s");
/// assert_eq!(format_elapsed(f64::NAN), "0s");
/// assert_eq!(format_duration("2024-01-01T00:00:00.000Z", "2024-01-01T00:01:01.000Z"), "1m 1s");
/// ```
///
/// # Errors
///
/// This function never returns an error. Invalid, unknown, or non-finite
/// inputs deterministically produce `"0s"` rather than failing.
#[must_use]
pub fn format_elapsed(elapsed_ms: f64) -> String {
    let total_seconds = elapsed_to_total_seconds(elapsed_ms);
    format_from_total_seconds(total_seconds)
}

/// Formats the span between two ISO-8601 timestamps as transcript duration text.
///
/// Mirrors `FormatDuration` in `duration.ts`, which computes
/// `FormatElapsed(Date.parse(ended_at) - Date.parse(started_at))`. Each
/// timestamp is parsed as an ISO-8601 / RFC-3339 instant; unparsable or
/// absent values behave like `NaN` in JavaScript and therefore floor to
/// `"0s"`. A negative difference (ended before started) also floors to
/// `"0s"`.
///
/// Accepted timestamp forms include `YYYY-MM-DD`, `YYYY-MM-DDTHH:MM:SS`,
/// `YYYY-MM-DDTHH:MM:SS.sss`, each optionally suffixed with `Z` or a
/// numeric offset `±HH:MM`, `±HHMM`, or `±HH`. Fractional seconds beyond
/// millisecond precision are truncated. When no timezone is present the
/// instant is treated as UTC, which matches the `toISOString()`-produced
/// inputs used by the call sites and keeps the function pure without
/// wall-clock or locale state.
///
/// # Examples
///
/// ```rust
/// use artisan_frontend::conversation_duration::format_duration;
///
/// assert_eq!(
///     format_duration("2024-01-01T00:00:00.000Z", "2024-01-01T01:00:03.000Z"),
///     "1h 0m 3s"
/// );
/// assert_eq!(format_duration("invalid", "2024-01-01T00:00:01.000Z"), "0s");
/// assert_eq!(
///     format_duration("2024-01-01T00:01:00.000Z", "2024-01-01T00:00:00.000Z"),
///     "0s"
/// );
/// ```
///
/// # Errors
///
/// This function never returns an error. Unparsable timestamps or a negative
/// span deterministically produce `"0s"`.
#[must_use]
pub fn format_duration(started_at: &str, ended_at: &str) -> String {
    let Some(start_ms) = parse_iso_to_millis(started_at) else {
        return format_from_total_seconds(0);
    };
    let Some(end_ms) = parse_iso_to_millis(ended_at) else {
        return format_from_total_seconds(0);
    };
    let diff = end_ms as f64 - start_ms as f64;
    format_elapsed(diff)
}

fn elapsed_to_total_seconds(elapsed_ms: f64) -> u64 {
    if !elapsed_ms.is_finite() || elapsed_ms <= 0.0 {
        return 0;
    }
    let secs_f = (elapsed_ms / 1_000.0).floor();
    if !secs_f.is_finite() || secs_f <= 0.0 {
        return 0;
    }
    if secs_f >= u64::MAX as f64 {
        return u64::MAX;
    }
    secs_f as u64
}

fn format_from_total_seconds(total_seconds: u64) -> String {
    let hours = total_seconds / 3_600;
    let minutes = (total_seconds % 3_600) / 60;
    let seconds = total_seconds % 60;
    if hours > 0 {
        format!("{hours}h {minutes}m {seconds}s")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{seconds}s")
    }
}

// ---------------------------------------------------------------------------
// Minimal ISO-8601 / RFC-3339 parser (dependency-light, pure, deterministic)
// ---------------------------------------------------------------------------

fn parse_iso_to_millis(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if s.len() < 10 {
        return None;
    }
    let date_part = &s[0..10];
    let bytes = date_part.as_bytes();
    if bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let year: i32 = date_part[0..4].parse().ok()?;
    let month: u8 = date_part[5..7].parse().ok()?;
    let day: u8 = date_part[8..10].parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let days = days_since_epoch(year, month, day)?;

    if s.len() == 10 {
        return millis_from_days_hms(days, 0, 0, 0, 0, 0);
    }

    let rest = &s[10..];
    let first = rest.as_bytes().first()?;
    if *first != b'T' && *first != b't' && *first != b' ' {
        return None;
    }
    let time_and_tz = &rest[1..];
    if time_and_tz.is_empty() {
        return None;
    }

    // Split time and timezone: find first Z/z/+/- after the time.
    let mut tz_start: Option<usize> = None;
    for (idx, ch) in time_and_tz.char_indices() {
        if ch == 'Z' || ch == 'z' || ch == '+' || ch == '-' {
            tz_start = Some(idx);
            break;
        }
    }

    let (time_str, tz_str) = match tz_start {
        Some(pos) => (&time_and_tz[..pos], &time_and_tz[pos..]),
        None => (time_and_tz, ""),
    };

    let (hour, minute, second, millis) = parse_time_part(time_str)?;

    let offset_minutes = parse_timezone_offset(tz_str)?;

    // Convert to UTC millis.
    let offset_millis: i64 = i64::from(offset_minutes) * 60 * 1_000;
    let local_millis = millis_from_days_hms(days, hour, minute, second, millis, 0)?;
    // UTC = local - offset
    local_millis.checked_sub(offset_millis)
}

fn parse_time_part(time_str: &str) -> Option<(u8, u8, u8, u16)> {
    if time_str.is_empty() {
        return None;
    }
    let colon_count = time_str.matches(':').count();
    match colon_count {
        1 => {
            let mut parts = time_str.split(':');
            let hour: u8 = parts.next()?.parse().ok()?;
            let minute: u8 = parts.next()?.parse().ok()?;
            if !(0..=23).contains(&hour) || !(0..=59).contains(&minute) {
                return None;
            }
            if parts.next().is_some() {
                return None;
            }
            Some((hour, minute, 0, 0))
        }
        2 => {
            let mut parts = time_str.split(':');
            let hour: u8 = parts.next()?.parse().ok()?;
            let minute: u8 = parts.next()?.parse().ok()?;
            let sec_frac = parts.next()?;
            if !(0..=23).contains(&hour) || !(0..=59).contains(&minute) {
                return None;
            }
            if parts.next().is_some() {
                return None;
            }
            let (second, millis) = parse_second_fraction(sec_frac)?;
            if second > 60 {
                return None;
            }
            // Clamp leap second 60 to 59 for millis calc; extra second is still 60s
            // but to keep arithmetic simple treat 60 as 59 + 1000ms? We just allow 60
            // and later compute millis; 60 seconds is valid for duration diff.
            // For epoch conversion, 60 maps to next minute start; we handle by
            // carrying over: if second == 60, set second=59 and millis+=1000 then
            // normalize? Simpler: reject 60 for parsing instant; conversation
            // timestamps never use leap seconds.
            if second == 60 {
                return None;
            }
            Some((hour, minute, second, millis))
        }
        _ => None,
    }
}

fn parse_second_fraction(sec_frac: &str) -> Option<(u8, u16)> {
    if let Some(dot) = sec_frac.find('.') {
        let sec_str = &sec_frac[..dot];
        let frac_str = &sec_frac[dot + 1..];
        if sec_str.is_empty() || frac_str.is_empty() {
            return None;
        }
        let second: u8 = sec_str.parse().ok()?;
        if frac_str.bytes().any(|b| !b.is_ascii_digit()) {
            return None;
        }
        let millis = fraction_to_millis(frac_str);
        Some((second, millis))
    } else {
        let second: u8 = sec_frac.parse().ok()?;
        Some((second, 0))
    }
}

fn fraction_to_millis(frac: &str) -> u16 {
    // Truncate or pad to 3 digits.
    let mut buf = [b'0'; 3];
    let copy_len = frac.len().min(3);
    buf[..copy_len].copy_from_slice(&frac.as_bytes()[..copy_len]);
    let s = std::str::from_utf8(&buf).unwrap_or("000");
    s.parse::<u16>().unwrap_or(0)
}

fn parse_timezone_offset(tz: &str) -> Option<i32> {
    if tz.is_empty() {
        return Some(0);
    }
    if tz.eq_ignore_ascii_case("z") {
        return Some(0);
    }
    let sign: i32 = match tz.as_bytes().first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let rest = &tz[1..];
    if rest.is_empty() {
        return None;
    }
    if let Some(colon) = rest.find(':') {
        let hour_str = &rest[..colon];
        let minute_str = &rest[colon + 1..];
        if hour_str.len() != 2 || minute_str.len() != 2 {
            return None;
        }
        let hour: i32 = hour_str.parse().ok()?;
        let minute: i32 = minute_str.parse().ok()?;
        if !(0..=23).contains(&hour) || !(0..=59).contains(&minute) {
            return None;
        }
        Some(sign * (hour * 60 + minute))
    } else if rest.len() == 2 {
        let hour: i32 = rest.parse().ok()?;
        if !(0..=23).contains(&hour) {
            return None;
        }
        Some(sign * hour * 60)
    } else if rest.len() == 4 {
        let hour: i32 = rest[0..2].parse().ok()?;
        let minute: i32 = rest[2..4].parse().ok()?;
        if !(0..=23).contains(&hour) || !(0..=59).contains(&minute) {
            return None;
        }
        Some(sign * (hour * 60 + minute))
    } else {
        None
    }
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0) && (year % 100 != 0 || year % 400 == 0)
}

fn days_in_month(year: i32, month: u8) -> u8 {
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

fn days_since_epoch(year: i32, month: u8, day: u8) -> Option<i64> {
    if !(1970..=2100).contains(&year) {
        return None;
    }
    let dim = days_in_month(year, month);
    if day == 0 || day > dim {
        return None;
    }
    let mut days: i64 = 0;
    for y in 1970..year {
        days = days.checked_add(if is_leap_year(y) { 366 } else { 365 })?;
    }
    for m in 1..month {
        days = days.checked_add(i64::from(days_in_month(year, m)))?;
    }
    days = days.checked_add(i64::from(day - 1))?;
    Some(days)
}

fn millis_from_days_hms(
    days: i64,
    hour: u8,
    minute: u8,
    second: u8,
    millis: u16,
    _extra: u8,
) -> Option<i64> {
    let day_millis = days.checked_mul(86_400_000)?;
    let hour_millis = i64::from(hour) * 3_600_000;
    let minute_millis = i64::from(minute) * 60_000;
    let second_millis = i64::from(second) * 1_000;
    let ms = i64::from(millis);
    day_millis
        .checked_add(hour_millis)?
        .checked_add(minute_millis)?
        .checked_add(second_millis)?
        .checked_add(ms)
}
