//! Deterministic relative-age formatting for settled conversation turns.
//!
//! This is the native counterpart of
//! `modules/frontend/src/lib/conversation/relative-time.ts`. It keeps date
//! parsing and display policy separate from the conversation footer renderer:
//! callers provide the observation instant, and this module returns only the
//! reader-facing age string.

const MILLIS_PER_SECOND: i128 = 1_000;
const MILLIS_PER_MINUTE: i128 = 60 * MILLIS_PER_SECOND;
const MILLIS_PER_HOUR: i128 = 60 * MILLIS_PER_MINUTE;
const MILLIS_PER_DAY: i128 = 24 * MILLIS_PER_HOUR;

/// Formats the age of `timestamp` at an explicit Unix-millisecond instant.
///
/// The thresholds, integer flooring, suffixes, and strings mirror the
/// TypeScript `format_relative_age` policy exactly:
///
/// - fewer than 60 seconds: `Ns ago`;
/// - fewer than 60 minutes: `Nm ago`;
/// - fewer than 24 hours: `Nh ago`;
/// - fewer than 7 days: `Nd ago`;
/// - fewer than 5 weeks: `Nw ago`;
/// - fewer than 12 thirty-day months: `Nmo ago`;
/// - otherwise: `Ny ago`, using 365 days per year.
///
/// `timestamp` accepts the ISO forms emitted by the protocol and fixtures:
/// canonical UTC date-times with an optional fractional second, date-only UTC
/// values, and ISO date-times with numeric offsets. A malformed timestamp
/// produces `"0s ago"`, matching the source policy's invalid-date fallback.
/// Timestamps after `now_millis` are clamped to the same zero-age result.
/// No clock or other process-global state is read.
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

    format!("{}y ago", elapsed_days / 365)
}

fn elapsed_seconds(now_millis: i64, timestamp: &str) -> u64 {
    let Some(timestamp_millis) = parse_timestamp_millis(timestamp) else {
        return 0;
    };

    if now_millis <= timestamp_millis {
        return 0;
    }

    let elapsed_millis = i128::from(now_millis) - i128::from(timestamp_millis);
    u64::try_from(elapsed_millis / MILLIS_PER_SECOND).unwrap_or(u64::MAX)
}

#[derive(Clone, Copy, Debug)]
struct CalendarDate {
    year: i32,
    month: u8,
    day: u8,
}

#[derive(Clone, Copy, Debug)]
struct ClockTime {
    hour: u8,
    minute: u8,
    second: u8,
    millisecond: u16,
    next_day: bool,
}

fn parse_timestamp_millis(input: &str) -> Option<i64> {
    if is_date_only(input) {
        let date = parse_date(input)?;
        return millis_from_parts(date, ClockTime::midnight(), 0);
    }

    let separator = input.find(['T', 't', ' '])?;
    let date = parse_date(&input[..separator])?;
    let time_and_zone = &input[separator + 1..];
    let (time, offset_minutes) = split_time_and_offset(time_and_zone)?;
    let clock = parse_time(time)?;

    millis_from_parts(date, clock, offset_minutes)
}

fn is_date_only(input: &str) -> bool {
    input.len() == 10
        && input.as_bytes().get(4) == Some(&b'-')
        && input.as_bytes().get(7) == Some(&b'-')
}

fn parse_date(input: &str) -> Option<CalendarDate> {
    if input.len() != 10
        || input.as_bytes().get(4) != Some(&b'-')
        || input.as_bytes().get(7) != Some(&b'-')
    {
        return None;
    }

    let bytes = input.as_bytes();
    let year = i32::try_from(parse_digits(&bytes[..4])?).ok()?;
    let month = u8::try_from(parse_digits(&bytes[5..7])?).ok()?;
    let day = u8::try_from(parse_digits(&bytes[8..10])?).ok()?;

    if month == 0 || month > 12 || day == 0 || day > days_in_month(year, month) {
        return None;
    }

    Some(CalendarDate { year, month, day })
}

fn split_time_and_offset(input: &str) -> Option<(&str, i32)> {
    if let Some(time) = input.strip_suffix(['Z', 'z']) {
        return (!time.is_empty()).then_some((time, 0));
    }

    let offset_start = input.rfind(['+', '-'])?;
    let (time, offset) = input.split_at(offset_start);
    if time.is_empty() {
        return None;
    }

    Some((time, parse_offset(offset)?))
}

fn parse_offset(input: &str) -> Option<i32> {
    let sign = match input.as_bytes().first()? {
        b'+' => 1_i32,
        b'-' => -1_i32,
        _ => return None,
    };
    let body = input.as_bytes().get(1..)?;

    let (hour_digits, minute_digits) = match body {
        [_, _, b':', _, _] => (&body[..2], &body[3..5]),
        [_, _, _, _] => (&body[..2], &body[2..4]),
        _ => return None,
    };

    let hours = parse_digits(hour_digits)?;
    let minutes = parse_digits(minute_digits)?;
    if hours > 23 || minutes > 59 {
        return None;
    }

    Some(sign * (i32::try_from(hours).ok()? * 60 + i32::try_from(minutes).ok()?))
}

fn parse_time(input: &str) -> Option<ClockTime> {
    let (main, fraction) = match input.split_once('.') {
        Some((main, fraction)) => (main, Some(fraction)),
        None => (input, None),
    };
    let mut components = main.split(':');
    let hour = parse_two_digits(components.next()?)?;
    let minute = parse_two_digits(components.next()?)?;
    let second = match components.next() {
        Some(value) => parse_two_digits(value)?,
        None if fraction.is_none() => 0,
        None => return None,
    };
    if components.next().is_some() {
        return None;
    }

    let millisecond = match fraction {
        Some(value) => parse_fraction(value)?,
        None => 0,
    };

    if minute > 59
        || second > 59
        || hour > 24
        || (hour == 24 && (minute != 0 || second != 0 || millisecond != 0))
    {
        return None;
    }

    Some(ClockTime {
        hour: if hour == 24 { 0 } else { hour },
        minute,
        second,
        millisecond,
        next_day: hour == 24,
    })
}

fn parse_two_digits(input: &str) -> Option<u8> {
    if input.len() != 2 {
        return None;
    }

    u8::try_from(parse_digits(input.as_bytes())?).ok()
}

fn parse_fraction(input: &str) -> Option<u16> {
    if input.is_empty() || !input.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }

    let prefix = &input.as_bytes()[..input.len().min(3)];
    let mut milliseconds = 0_u16;
    for &digit in prefix {
        milliseconds = milliseconds * 10 + u16::from(digit - b'0');
    }
    for _ in prefix.len()..3 {
        milliseconds *= 10;
    }
    Some(milliseconds)
}

fn parse_digits(input: &[u8]) -> Option<u32> {
    if input.is_empty() || !input.iter().all(u8::is_ascii_digit) {
        return None;
    }

    let mut value = 0_u32;
    for &digit in input {
        value = value
            .checked_mul(10)?
            .checked_add(u32::from(digit - b'0'))?;
    }
    Some(value)
}

impl ClockTime {
    const fn midnight() -> Self {
        Self {
            hour: 0,
            minute: 0,
            second: 0,
            millisecond: 0,
            next_day: false,
        }
    }
}

fn millis_from_parts(date: CalendarDate, time: ClockTime, offset_minutes: i32) -> Option<i64> {
    let day_count = i128::from(days_from_civil(
        date.year,
        u32::from(date.month),
        u32::from(date.day),
    ));
    let next_day = if time.next_day { MILLIS_PER_DAY } else { 0 };
    let local_millis = day_count * MILLIS_PER_DAY
        + next_day
        + i128::from(time.hour) * MILLIS_PER_HOUR
        + i128::from(time.minute) * MILLIS_PER_MINUTE
        + i128::from(time.second) * MILLIS_PER_SECOND
        + i128::from(time.millisecond);
    let utc_millis = local_millis - i128::from(offset_minutes) * MILLIS_PER_MINUTE;

    i64::try_from(utc_millis).ok()
}

const fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

const fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// Returns the days from 1970-01-01 using Howard Hinnant's civil-date method.
fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let month = i64::from(month);
    let year = i64::from(year) - i64::from(u32::from(month <= 2));
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month_from_march = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * month_from_march + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;

    era * 146_097 + day_of_era - 719_468
}
