use super::system::posix_locale;
use super::{Formats, SystemLocales};
use chrono::NaiveDate;
use icu_locale_core::{Locale, locale};
use std::collections::HashMap;

fn formats(locale: &Locale) -> Formats {
    Formats::new(locale)
}

#[test]
fn counts_pick_the_singular_only_for_exactly_one() {
    let english = formats(&locale!("en-US"));
    assert_eq!(english.count(0_u64, "file", "files"), "0 files");
    assert_eq!(english.count(1_u64, "file", "files"), "1 file");
    assert_eq!(english.count(2_u64, "file", "files"), "2 files");
    assert_eq!(english.count(21_u64, "token", "tokens"), "21 tokens");
}

#[test]
fn counts_group_digits_the_way_the_locale_does() {
    let count = |locale: &Locale| formats(locale).count(1_234_567_u64, "file", "files");
    assert_eq!(count(&locale!("en-US")), "1,234,567 files");
    assert_eq!(count(&locale!("de-DE")), "1.234.567 files");
    assert_eq!(count(&locale!("nb-NO")), "1\u{a0}234\u{a0}567 files");
    assert_eq!(count(&locale!("hi-IN")), "12,34,567 files");
}

#[test]
fn decimals_use_the_locale_separator_and_trim_only_when_asked() {
    let english = formats(&locale!("en-US"));
    let german = formats(&locale!("de-DE"));
    assert_eq!(english.decimal(12.5, 2), "12.50");
    assert_eq!(german.decimal(12.5, 2), "12,50");
    assert_eq!(english.trimmed(12.5, 2), "12.5");
    assert_eq!(german.trimmed(1234.5, 2), "1.234,5");
    assert_eq!(german.trimmed(3.0, 2), "3");
    assert_eq!(english.decimal(f64::NAN, 1), "–");
}

#[test]
fn clocks_and_dates_follow_the_locale() {
    let at = NaiveDate::from_ymd_opt(2026, 11, 18)
        .and_then(|day| day.and_hms_opt(14, 5, 0))
        .unwrap();
    let english = formats(&locale!("en-US"));
    let german = formats(&locale!("de-DE"));
    let norwegian = formats(&locale!("nb-NO"));
    assert_eq!(english.clock(at), "2:05\u{202f}PM");
    assert_eq!(german.clock(at), "14:05");
    assert_eq!(english.date(at.date()), "11/18/2026");
    assert_eq!(german.date(at.date()), "18.11.2026");
    assert_eq!(norwegian.date(at.date()), "18.11.2026");
}

#[test]
fn posix_settings_become_locales_and_c_means_none() {
    assert_eq!(posix_locale("nb_NO.UTF-8"), Some(locale!("nb-NO")));
    assert_eq!(posix_locale("de_DE@euro"), Some(locale!("de-DE")));
    assert_eq!(posix_locale("C.UTF-8"), None);
    assert_eq!(posix_locale("POSIX"), None);
}

#[test]
fn the_environment_picks_each_category_with_lc_all_first() {
    let from = |pairs: &[(&str, &str)]| {
        let variables: HashMap<String, String> = pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect();
        SystemLocales::from_environment(|key| variables.get(key).cloned())
    };
    assert_eq!(
        from(&[
            ("LANG", "en_GB.UTF-8"),
            ("LC_NUMERIC", "de_DE.UTF-8"),
            ("LC_TIME", "nb_NO.UTF-8")
        ]),
        SystemLocales {
            numbers: locale!("de-DE"),
            time: locale!("nb-NO"),
        }
    );
    assert_eq!(
        from(&[("LC_ALL", "fr_FR.UTF-8"), ("LC_NUMERIC", "de_DE.UTF-8")]),
        SystemLocales::uniform(locale!("fr-FR"))
    );
    assert_eq!(
        from(&[("LANG", "C.UTF-8"), ("LC_TIME", "")]),
        SystemLocales::uniform(locale!("en-US"))
    );
}
