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

impl Time {
    /// A finite number, or exactly a `YYYY-MM-DD` calendar date.
    /// Timestamps are rejected rather than silently losing their time.
    pub(in super::super) fn parse(value: &Value) -> Option<Self> {
        match value {
            Value::Number(number) => number.as_f64().filter(|n| n.is_finite()).map(Self::Number),
            Value::String(text) => {
                let text = text.trim();
                if let Ok(number) = text.parse::<f64>() {
                    return number.is_finite().then_some(Self::Number(number));
                }
                if text.len() != 10 {
                    return None;
                }
                let date = chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d").ok()?;
                let epoch = chrono::NaiveDate::from_ymd_opt(1970, 1, 1)?;
                let days = i32::try_from(date.signed_duration_since(epoch).num_days()).ok()?;
                Some(Self::Date(f64::from(days)))
            }
            Value::Null | Value::Bool(_) | Value::Array(_) | Value::Object(_) => None,
        }
    }

    pub(in super::super) const fn is_date(self) -> bool {
        matches!(self, Self::Date(_))
    }

    pub(in super::super) const fn value(self) -> f64 {
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
                let epoch = chrono::NaiveDate::from_ymd_opt(1970, 1, 1).ok_or(fmt::Error)?;
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "dates hold whole day counts parsed from an i32; the cast saturates"
                )]
                let days = days.round() as i64;
                let date = epoch
                    .checked_add_signed(chrono::Duration::days(days))
                    .ok_or(fmt::Error)?;
                write!(f, "{}", date.format("%b %-d, %Y"))
            }
        }
    }
}
