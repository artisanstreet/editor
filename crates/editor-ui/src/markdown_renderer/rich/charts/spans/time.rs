//! Span times: plain numbers, or ISO calendar dates counted in days since
//! 1970-01-01 so they share one numeric axis.

use std::fmt;

use serde_json::Value;

/// A time as written in a spec.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in super::super) enum Time {
    Number(f64),
    /// Days since 1970-01-01.
    Date(f64),
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

impl Time {
    /// A finite number, or a `YYYY-MM-DD` string (anything after the date,
    /// such as a time of day, is ignored).
    pub(in super::super) fn parse(value: &Value) -> Option<Self> {
        match value {
            Value::Number(number) => number.as_f64().filter(|n| n.is_finite()).map(Self::Number),
            Value::String(text) => {
                let text = text.trim();
                if let Ok(number) = text.parse::<f64>() {
                    return number.is_finite().then_some(Self::Number(number));
                }
                let date = text.get(..10)?;
                let mut parts = date.split('-');
                let year: i64 = parts.next()?.parse().ok()?;
                let month: u32 = parts.next()?.parse().ok()?;
                let day: u32 = parts.next()?.parse().ok()?;
                if !(1..=12).contains(&month) || !(1..=days_in_month(year, month)).contains(&day) {
                    return None;
                }
                #[expect(clippy::cast_precision_loss, reason = "day counts are small")]
                Some(Self::Date(days_from_civil(year, month, day) as f64))
            }
            _ => None,
        }
    }

    pub(in super::super) fn is_date(self) -> bool {
        matches!(self, Self::Date(_))
    }

    pub(in super::super) fn value(self) -> f64 {
        match self {
            Self::Number(value) | Self::Date(value) => value,
        }
    }
}

impl fmt::Display for Time {
    /// Dates read "Oct 7, 2026"; numbers as plain numbers.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Number(value) => write!(f, "{value}"),
            Self::Date(days) => {
                #[expect(clippy::cast_possible_truncation, reason = "rounded day count")]
                let (year, month, day) = civil_from_days(days.round() as i64);
                write!(f, "{} {day}, {year}", MONTHS[(month - 1) as usize])
            }
        }
    }
}

fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01 (Howard Hinnant's `days_from_civil`).
pub(in super::super) fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let day_of_year =
        (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The civil date of a day count (Howard Hinnant's `civil_from_days`).
pub(in super::super) fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = days.div_euclid(146_097);
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "1..=12 and 1..=31"
    )]
    (year, month as u32, day as u32)
}
