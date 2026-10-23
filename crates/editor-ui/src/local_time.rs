//! Moments as this computer's settings write them: its time zone, and on
//! Windows the user's locale (12- or 24-hour clock, date order).

use artisan_api::Timestamp;
use chrono::{DateTime, Local, NaiveDateTime};

/// `at` as a clock time when it falls today, else with its short date
/// first: "15:00", "3:00 PM", "22.10.2026 09:00".
#[must_use]
pub fn moment(at: Timestamp) -> String {
    let Some(local) = DateTime::from_timestamp_millis(at).map(|at| at.with_timezone(&Local)) else {
        return String::new();
    };
    let local = local.naive_local();
    let time = clock(local);
    if local.date() == Local::now().date_naive() {
        time
    } else {
        format!("{} {time}", short_date(local))
    }
}

#[cfg(windows)]
fn system_time(at: NaiveDateTime) -> windows::Win32::Foundation::SYSTEMTIME {
    use chrono::{Datelike as _, Timelike as _};
    let word = |value: u32| u16::try_from(value).unwrap_or(0);
    windows::Win32::Foundation::SYSTEMTIME {
        wYear: u16::try_from(at.year()).unwrap_or(0),
        wMonth: word(at.month()),
        wDayOfWeek: word(at.weekday().num_days_from_sunday()),
        wDay: word(at.day()),
        wHour: word(at.hour()),
        wMinute: word(at.minute()),
        wSecond: 0,
        wMilliseconds: 0,
    }
}

#[cfg(windows)]
fn clock(at: NaiveDateTime) -> String {
    use windows::Win32::Globalization::{GetTimeFormatEx, TIME_NOSECONDS};
    use windows::core::PCWSTR;
    let time = system_time(at);
    let mut text = [0_u16; 64];
    // SAFETY: the time and buffer outlive the call; a null locale and
    // format mean the user's own.
    let written = unsafe {
        GetTimeFormatEx(
            PCWSTR::null(),
            TIME_NOSECONDS,
            Some(&raw const time),
            PCWSTR::null(),
            Some(&mut text),
        )
    };
    utf16(&text, written).unwrap_or_else(|| at.format("%H:%M").to_string())
}

#[cfg(windows)]
fn short_date(at: NaiveDateTime) -> String {
    use windows::Win32::Globalization::{DATE_SHORTDATE, GetDateFormatEx};
    use windows::core::PCWSTR;
    let date = system_time(at);
    let mut text = [0_u16; 64];
    // SAFETY: as in `clock`; no calendar override.
    let written = unsafe {
        GetDateFormatEx(
            PCWSTR::null(),
            DATE_SHORTDATE,
            Some(&raw const date),
            PCWSTR::null(),
            Some(&mut text),
            PCWSTR::null(),
        )
    };
    utf16(&text, written).unwrap_or_else(|| at.format("%Y-%m-%d").to_string())
}

/// The text a Win32 formatter wrote, without its terminating nul.
#[cfg(windows)]
fn utf16(text: &[u16], written: i32) -> Option<String> {
    let written = usize::try_from(written).ok()?.checked_sub(1)?;
    String::from_utf16(text.get(..written)?).ok()
}

#[cfg(not(windows))]
fn clock(at: NaiveDateTime) -> String {
    at.format("%H:%M").to_string()
}

#[cfg(not(windows))]
fn short_date(at: NaiveDateTime) -> String {
    at.format("%Y-%m-%d").to_string()
}
