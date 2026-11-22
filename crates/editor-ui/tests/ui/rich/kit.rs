use super::compact_number;
use crate::ui::locale::{Formats, SystemLocales};
use icu_locale_core::locale;

#[test]
fn compact_numbers_keep_the_suffix_and_take_the_locale_separator() {
    let english = Formats::for_system(&SystemLocales::uniform(locale!("en-US")));
    let german = Formats::for_system(&SystemLocales::uniform(locale!("de-DE")));
    assert_eq!(compact_number(&english, 12_345.0), "12.3k");
    assert_eq!(compact_number(&german, 12_345.0), "12,3k");
    assert_eq!(compact_number(&german, 1_500_000.0), "1,5M");
    assert_eq!(compact_number(&german, 9_999.0), "9.999");
    assert_eq!(compact_number(&english, 2.0), "2");
    assert_eq!(compact_number(&english, 0.126), "0.13");
    assert_eq!(compact_number(&english, f64::INFINITY), "–");
}

#[test]
fn a_value_that_rounds_up_to_the_next_thousand_takes_the_next_suffix() {
    let english = Formats::for_system(&SystemLocales::uniform(locale!("en-US")));
    assert_eq!(compact_number(&english, 999_950.0), "1M");
    assert_eq!(compact_number(&english, -999_950.0), "-1M");
    assert_eq!(compact_number(&english, 999_499.0), "999k");
    assert_eq!(compact_number(&english, 9_999.6), "10k");
    assert_eq!(compact_number(&english, 999_999_999.0), "1B");
}

#[test]
fn a_negative_value_that_rounds_to_zero_shows_no_minus_sign() {
    let english = Formats::for_system(&SystemLocales::uniform(locale!("en-US")));
    assert_eq!(compact_number(&english, -0.001), "0");
    assert_eq!(compact_number(&english, -0.25), "-0.25");
}
