use crate::ui::locale::{self, Formats};
use artisan_api::Timestamp;
use chrono::{DateTime, Local, NaiveDate, NaiveDateTime};

pub fn occasion(at: Timestamp) -> String {
    let Some(local) = DateTime::from_timestamp_millis(at).map(|at| at.with_timezone(&Local)) else {
        return String::new();
    };
    locale::with_system(|formats| {
        occasion_on(formats, local.naive_local(), Local::now().date_naive())
    })
}

pub fn occasion_on(formats: &Formats, at: NaiveDateTime, today: NaiveDate) -> String {
    let time = formats.clock(at);
    match (at.date() - today).num_days() {
        0 => format!("today at {time}"),
        1 => format!("tomorrow at {time}"),
        2..=6 => format!("on {} at {time}", at.format("%A")),
        _ => format!("on {} at {time}", formats.date(at.date())),
    }
}

#[cfg(test)]
#[path = "../../tests/ui/local_time.rs"]
mod tests;
