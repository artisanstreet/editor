use super::occasion_on;
use crate::ui::locale::{Formats, SystemLocales};
use chrono::NaiveDate;
use icu_locale_core::locale;

#[test]
fn occasions_read_as_english_with_locale_clocks_and_dates() {
    let today = NaiveDate::from_ymd_opt(2026, 11, 18).unwrap();
    let at = |days: u64| {
        today
            .checked_add_days(chrono::Days::new(days))
            .and_then(|day| day.and_hms_opt(9, 30, 0))
            .unwrap()
    };
    let german = Formats::for_system(&SystemLocales::uniform(locale!("de-DE")));
    let english = Formats::for_system(&SystemLocales::uniform(locale!("en-US")));
    assert_eq!(occasion_on(&german, at(0), today), "today at 09:30");
    assert_eq!(
        occasion_on(&english, at(1), today),
        "tomorrow at 9:30\u{202f}AM"
    );
    assert_eq!(occasion_on(&german, at(2), today), "on Friday at 09:30");
    assert_eq!(
        occasion_on(&german, at(30), today),
        "on 18.12.2026 at 09:30"
    );
}
