//! ISO-8601 instants carried by the usage protocol: strict validation and
//! allocation-light formatting of signed Unix epoch milliseconds.

use super::EngineUsageError;

/// Validates one ISO-8601 timestamp of the strict UTC form the usage
/// protocol carries: `YYYY-MM-DDTHH:MM:SS[.fraction](Z|±HH:MM)`.
///
/// Day-of-month is checked against month length including leap years; a
/// four-or-more-digit year is required. This validation is syntactic: it
/// proves the shape a renderer can display, not that the instant is near the
/// current wall clock.
///
/// # Errors
///
/// Returns [`EngineUsageError::Timestamp`] with a stable reason for any
/// shape, range, or calendar violation.
pub fn validate_iso_timestamp(value: &str, field: &'static str) -> Result<(), EngineUsageError> {
    let invalid = |reason: &'static str| EngineUsageError::Timestamp { field, reason };
    let bytes = value.as_bytes();
    // Minimum: `YYYY-MM-DDTHH:MM:SSZ` (20 bytes).
    if bytes.len() < 20 {
        return Err(invalid("timestamp is shorter than YYYY-MM-DDTHH:MM:SSZ"));
    }
    if bytes.len() > 64 {
        return Err(invalid("timestamp exceeds its 64-byte ceiling"));
    }
    let digits = |start: usize, end: usize| -> Result<u32, EngineUsageError> {
        let mut number = 0_u32;
        for byte in &bytes[start..end] {
            if !byte.is_ascii_digit() {
                return Err(invalid("timestamp date/time fields must be ASCII digits"));
            }
            number = number * 10 + u32::from(byte - b'0');
        }
        Ok(number)
    };
    // Year accepts four or more digits; find the first `-` separator.
    let year_end = bytes
        .iter()
        .position(|byte| *byte == b'-')
        .ok_or(invalid("timestamp must separate the year with '-'"))?;
    if year_end < 4 {
        return Err(invalid("timestamp year needs at least four digits"));
    }
    for byte in &bytes[..year_end] {
        if !byte.is_ascii_digit() {
            return Err(invalid("timestamp year must be ASCII digits"));
        }
    }
    let rest = &bytes[year_end..];
    // `-MM-DDTHH:MM:SS` is 15 bytes after the year.
    if rest.len() < 15 {
        return Err(invalid("timestamp is shorter than YYYY-MM-DDTHH:MM:SSZ"));
    }
    if rest[0] != b'-' || rest[3] != b'-' || rest[6] != b'T' || rest[9] != b':' || rest[12] != b':'
    {
        return Err(invalid("timestamp separators must be -MM-DDTHH:MM:SS"));
    }
    let month = digits(year_end + 1, year_end + 3)?;
    let day = digits(year_end + 4, year_end + 6)?;
    let hour = digits(year_end + 7, year_end + 9)?;
    let minute = digits(year_end + 10, year_end + 12)?;
    let second = digits(year_end + 13, year_end + 15)?;
    if month == 0 || month > 12 {
        return Err(invalid("timestamp month is out of range"));
    }
    let leap = is_leap_year_mod(&bytes[..year_end]);
    let month_days = [
        31,
        28 + u32::from(leap),
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if day == 0 || day > month_days[(month - 1) as usize] {
        return Err(invalid("timestamp day is out of range for its month"));
    }
    if hour > 23 {
        return Err(invalid("timestamp hour is out of range"));
    }
    if minute > 59 || second > 59 {
        return Err(invalid("timestamp minute or second is out of range"));
    }
    let mut cursor = year_end + 15;
    if bytes.get(cursor) == Some(&b'.') {
        cursor += 1;
        let fraction_start = cursor;
        while cursor < bytes.len() && bytes[cursor].is_ascii_digit() {
            cursor += 1;
        }
        if cursor == fraction_start || cursor - fraction_start > 9 {
            return Err(invalid("timestamp fraction needs one to nine digits"));
        }
    }
    let zone = &bytes[cursor..];
    validate_iso_zone(zone, field)?;
    Ok(())
}

/// Validates the trailing `Z` or `±HH:MM` zone of an ISO-8601 timestamp.
///
/// Callers have already proven the cursor points one past the seconds field.
fn validate_iso_zone(zone: &[u8], field: &'static str) -> Result<(), EngineUsageError> {
    let invalid = |reason: &'static str| EngineUsageError::Timestamp { field, reason };
    if zone == *b"Z" {
        return Ok(());
    }
    if zone.len() == 6 && (zone[0] == b'+' || zone[0] == b'-') && zone[3] == b':' {
        if !zone[1].is_ascii_digit()
            || !zone[2].is_ascii_digit()
            || !zone[4].is_ascii_digit()
            || !zone[5].is_ascii_digit()
        {
            return Err(invalid("timestamp zone offset must be ASCII digits"));
        }
        let zone_hour = (u32::from(zone[1] - b'0')) * 10 + u32::from(zone[2] - b'0');
        let zone_minute = (u32::from(zone[4] - b'0')) * 10 + u32::from(zone[5] - b'0');
        if zone_hour > 23 || zone_minute > 59 {
            return Err(invalid("timestamp zone offset is out of range"));
        }
        return Ok(());
    }
    Err(invalid("timestamp zone must be Z or ±HH:MM"))
}

fn is_leap_year_mod(year_digits: &[u8]) -> bool {
    // The Gregorian rule evaluated with modular arithmetic so years beyond
    // four digits need no bignum: leap iff (divisible by 4 and not by 100)
    // or divisible by 400. Callers have already proven every byte is a digit.
    let mut mod4 = 0_u32;
    let mut mod100 = 0_u32;
    let mut mod400 = 0_u32;
    for byte in year_digits {
        let digit = u32::from(byte - b'0');
        mod4 = (mod4 * 10 + digit) % 4;
        mod100 = (mod100 * 10 + digit) % 100;
        mod400 = (mod400 * 10 + digit) % 400;
    }
    (mod4 == 0 && mod100 != 0) || mod400 == 0
}

/// Formats signed Unix epoch milliseconds as an ISO-8601 UTC timestamp.
///
/// Uses the proleptic Gregorian calendar (Howard Hinnant's days-from-civil
/// inversion) with Euclidean division so pre-1970 instants format correctly.
/// The year prints with at least four digits; millisecond precision is
/// preserved. This is total over the complete [`i64`] range.
#[must_use]
pub fn iso_millis(millis: i64) -> String {
    let seconds = millis.div_euclid(1_000);
    let millisecond = millis.rem_euclid(1_000);
    let days = seconds.div_euclid(86_400);
    let day_seconds = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = day_seconds / 3_600;
    let minute = (day_seconds % 3_600) / 60;
    let second = day_seconds % 60;
    if millisecond == 0 {
        format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
    } else {
        format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millisecond:03}Z")
    }
}

/// Splits signed Unix epoch milliseconds into a proleptic Gregorian
/// UTC calendar date.
///
/// Total over the complete [`i64`] range; used by readers that must infer a
/// reset year from wall-clock fields without a timezone database.
#[must_use]
pub fn utc_ymd(millis: i64) -> (i64, u32, u32) {
    let seconds = millis.div_euclid(1_000);
    civil_from_days(seconds.div_euclid(86_400))
}

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the civil-from-days inversion yields day 1..=31 and month 1..=12"
)]
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    // Inverts days-from-civil for the proleptic Gregorian calendar.
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_prime + 2) / 5 + 1) as u32;
    let month = if month_prime < 10 {
        (month_prime + 3) as u32
    } else {
        (month_prime - 9) as u32
    };
    (if month <= 2 { year + 1 } else { year }, month, day)
}
