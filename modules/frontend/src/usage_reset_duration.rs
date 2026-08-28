//! Dependency-free presentation policy for engine usage reset durations.
//!
//! This is the native counterpart of
//! `modules/frontend/src/lib/identity/usage-reset.ts`. The protocol adapter
//! supplies only the reset timestamp from each validated engine-usage window;
//! this module does not duplicate the window's identity, cadence, percentage,
//! or provider-specific fields.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

const MILLIS_PER_MINUTE: i128 = 60_000;
const MILLIS_PER_HOUR: i128 = 60 * MILLIS_PER_MINUTE;
const MILLIS_PER_DAY: i128 = 24 * MILLIS_PER_HOUR;
const JAVASCRIPT_TIME_CLIP_LIMIT_MS: i128 = 8_640_000_000_000_000;

/// The reset timestamp slice of one protocol engine-usage window.
///
/// The generated protocol window also contains provider identity, cadence,
/// percentage, and window-length fields. None of those facts participate in
/// the reset-duration policy, so callers adapt only `resets_at` here. A
/// missing timestamp is represented by `None` and intentionally makes the
/// whole group silent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UsageResetWindow<'a> {
    /// The provider's optional ISO-8601 reset timestamp.
    pub resets_at: Option<&'a str>,
}

impl<'a> UsageResetWindow<'a> {
    /// Creates an adapter for one protocol window's reset timestamp.
    #[must_use]
    pub const fn new(resets_at: Option<&'a str>) -> Self {
        Self { resets_at }
    }
}

/// Alias matching the protocol's full window name for adapter call sites.
pub type EngineUsageWindow<'a> = UsageResetWindow<'a>;

/// Formats the remaining time until the latest reset in a complete window
/// group.
///
/// The group is fail-closed: an empty group, a missing or unparseable reset,
/// or any reset at or before `at_ms` returns `None`. For a complete future
/// group, the latest reset controls the duration. Remaining milliseconds are
/// rounded upward to whole minutes with a one-minute floor, then displayed in
/// minutes below one hour, hours below one day, or days otherwise. The
/// returned labels use the exact singular/plural words from the TypeScript
/// caller.
#[must_use]
pub fn usage_reset_duration(windows: &[UsageResetWindow<'_>], at_ms: i64) -> Option<String> {
    if windows.is_empty() {
        return None;
    }

    let mut latest_reset_ms: Option<i64> = None;
    for window in windows {
        let reset_at_ms = parse_iso_timestamp_ms(window.resets_at?)?;
        if reset_at_ms <= at_ms {
            return None;
        }
        latest_reset_ms =
            Some(latest_reset_ms.map_or(reset_at_ms, |latest| latest.max(reset_at_ms)));
    }

    let latest_reset_ms = latest_reset_ms?;
    let remaining_ms = i128::from(latest_reset_ms) - i128::from(at_ms);
    let minutes = ceil_positive_division(remaining_ms, MILLIS_PER_MINUTE).max(1);
    let (amount, unit) = if minutes < 60 {
        (minutes, "minute")
    } else if minutes < 24 * 60 {
        (ceil_positive_division(minutes, 60), "hour")
    } else {
        (ceil_positive_division(minutes, 24 * 60), "day")
    };
    let unit_label = if amount == 1 {
        unit
    } else {
        match unit {
            "minute" => "minutes",
            "hour" => "hours",
            "day" => "days",
            _ => unreachable!("the policy has only known display units"),
        }
    };

    Some(format!("{amount} {unit_label}"))
}

/// Parses the ISO timestamp forms admitted by the protocol boundary and its
/// explicit-offset test seam into JavaScript-compatible epoch milliseconds.
///
/// Protocol timestamps are canonical UTC strings. The offset forms are also
/// accepted because the browser policy calls `Date.parse`, which converts an
/// explicit ISO-8601 offset before doing the duration arithmetic. Fractional
/// seconds beyond milliseconds are ignored, matching `Date.parse`'s
/// millisecond time clip. Values outside JavaScript's finite date range are
/// rejected rather than leaking an invalid duration.
fn parse_iso_timestamp_ms(value: &str) -> Option<i64> {
    let bytes = value.as_bytes();
    let mut cursor = 0;
    let (negative_year, year_digits) = match bytes.get(cursor) {
        Some(b'+') => {
            cursor += 1;
            (false, 6)
        }
        Some(b'-') => {
            cursor += 1;
            (true, 6)
        }
        _ => (false, 4),
    };
    let year_magnitude = parse_digits(bytes, &mut cursor, year_digits)?;
    if negative_year && year_magnitude == 0 {
        return None;
    }
    let year = if negative_year {
        -year_magnitude
    } else {
        year_magnitude
    };

    expect_byte(bytes, &mut cursor, b'-')?;
    let month = parse_digits(bytes, &mut cursor, 2)?;
    expect_byte(bytes, &mut cursor, b'-')?;
    let day = parse_digits(bytes, &mut cursor, 2)?;
    expect_byte(bytes, &mut cursor, b'T')?;
    let hour = parse_digits(bytes, &mut cursor, 2)?;
    expect_byte(bytes, &mut cursor, b':')?;
    let minute = parse_digits(bytes, &mut cursor, 2)?;
    expect_byte(bytes, &mut cursor, b':')?;
    let second = parse_digits(bytes, &mut cursor, 2)?;
    let millisecond = parse_fractional_milliseconds(bytes, &mut cursor)?;

    if minute > 59 || second > 59 {
        return None;
    }
    let day_carry = if hour == 24 {
        if minute != 0 || second != 0 || millisecond != 0 {
            return None;
        }
        1
    } else if hour <= 23 {
        0
    } else {
        return None;
    };

    let days = days_from_civil(year, month, day)?.checked_add(day_carry)?;
    let utc_offset_minutes = parse_timezone_offset_minutes(bytes, &mut cursor)?;
    if cursor != bytes.len() {
        return None;
    }

    let local_ms = days
        .checked_mul(MILLIS_PER_DAY)?
        .checked_add(hour.checked_mul(MILLIS_PER_HOUR)?)?
        .checked_add(minute.checked_mul(MILLIS_PER_MINUTE)?)?
        .checked_add(second.checked_mul(1_000)?)?
        .checked_add(millisecond)?;
    let timestamp_ms = local_ms.checked_sub(utc_offset_minutes.checked_mul(MILLIS_PER_MINUTE)?)?;
    if !(-JAVASCRIPT_TIME_CLIP_LIMIT_MS..=JAVASCRIPT_TIME_CLIP_LIMIT_MS).contains(&timestamp_ms) {
        return None;
    }

    i64::try_from(timestamp_ms).ok()
}

fn parse_digits(bytes: &[u8], cursor: &mut usize, count: usize) -> Option<i128> {
    let end = cursor.checked_add(count)?;
    let digits = bytes.get(*cursor..end)?;
    if !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }

    let mut value: i128 = 0;
    for digit in digits {
        value = value
            .checked_mul(10)?
            .checked_add(i128::from(*digit - b'0'))?;
    }
    *cursor = end;
    Some(value)
}

fn expect_byte(bytes: &[u8], cursor: &mut usize, expected: u8) -> Option<()> {
    if bytes.get(*cursor) != Some(&expected) {
        return None;
    }
    *cursor += 1;
    Some(())
}

fn parse_fractional_milliseconds(bytes: &[u8], cursor: &mut usize) -> Option<i128> {
    if bytes.get(*cursor) != Some(&b'.') {
        return Some(0);
    }
    *cursor += 1;
    let start = *cursor;
    while bytes.get(*cursor).is_some_and(u8::is_ascii_digit) {
        *cursor += 1;
    }
    let digits = bytes.get(start..*cursor)?;
    if digits.is_empty() {
        return None;
    }

    let significant_digits = digits.len().min(3);
    let mut milliseconds: i128 = 0;
    for digit in digits.iter().take(significant_digits) {
        milliseconds = milliseconds
            .checked_mul(10)?
            .checked_add(i128::from(*digit - b'0'))?;
    }
    Some(match significant_digits {
        1 => milliseconds * 100,
        2 => milliseconds * 10,
        _ => milliseconds,
    })
}

fn parse_timezone_offset_minutes(bytes: &[u8], cursor: &mut usize) -> Option<i128> {
    match bytes.get(*cursor) {
        Some(b'Z') => {
            *cursor += 1;
            Some(0)
        }
        Some(b'+' | b'-') => {
            let sign = *bytes.get(*cursor)?;
            *cursor += 1;
            let hours = parse_digits(bytes, cursor, 2)?;
            let has_colon = bytes.get(*cursor) == Some(&b':');
            if has_colon {
                *cursor += 1;
            }
            let minutes = parse_digits(bytes, cursor, 2)?;
            if hours > 23 || minutes > 59 {
                return None;
            }
            let magnitude = hours.checked_mul(60)?.checked_add(minutes)?;
            Some(if sign == b'+' { magnitude } else { -magnitude })
        }
        _ => None,
    }
}

fn days_from_civil(year: i128, month: i128, day: i128) -> Option<i128> {
    if !(1..=12).contains(&month) {
        return None;
    }
    let maximum_day = match month {
        2 if is_leap_year(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if !(1..=maximum_day).contains(&day) {
        return None;
    }

    // Howard Hinnant's proleptic-Gregorian civil-date conversion, with the
    // epoch offset used by JavaScript Date values.
    let adjusted_year = year - i128::from(month <= 2);
    let era = if adjusted_year >= 0 {
        adjusted_year / 400
    } else {
        (adjusted_year - 399) / 400
    };
    let year_of_era = adjusted_year - era * 400;
    let month_prime = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month_prime + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some(era * 146_097 + day_of_era - 719_468)
}

fn is_leap_year(year: i128) -> bool {
    year.rem_euclid(4) == 0 && (year.rem_euclid(100) != 0 || year.rem_euclid(400) == 0)
}

fn ceil_positive_division(dividend: i128, divisor: i128) -> i128 {
    let quotient = dividend / divisor;
    if dividend % divisor == 0 {
        quotient
    } else {
        quotient + 1
    }
}
