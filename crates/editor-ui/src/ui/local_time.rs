use artisan_api::Timestamp;
use chrono::{DateTime, Local, NaiveDateTime};

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
fn parts(at: NaiveDateTime) -> aqua::local_time::LocalDateParts {
    use chrono::{Datelike as _, Timelike as _};
    let word = |value: u32| u16::try_from(value).unwrap_or(0);
    aqua::local_time::LocalDateParts {
        year: u16::try_from(at.year()).unwrap_or(0),
        month: word(at.month()),
        weekday: word(at.weekday().num_days_from_sunday()),
        day: word(at.day()),
        hour: word(at.hour()),
        minute: word(at.minute()),
    }
}

#[cfg(windows)]
fn clock(at: NaiveDateTime) -> String {
    aqua::local_time::clock(parts(at)).unwrap_or_else(|| at.format("%H:%M").to_string())
}

#[cfg(windows)]
fn short_date(at: NaiveDateTime) -> String {
    aqua::local_time::short_date(parts(at)).unwrap_or_else(|| at.format("%Y-%m-%d").to_string())
}

#[cfg(not(windows))]
fn clock(at: NaiveDateTime) -> String {
    at.format("%H:%M").to_string()
}

#[cfg(not(windows))]
fn short_date(at: NaiveDateTime) -> String {
    at.format("%Y-%m-%d").to_string()
}
