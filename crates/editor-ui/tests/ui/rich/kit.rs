use super::compact_number;
use crate::ui::locale::Formats;
use icu_locale_core::locale;

#[test]
fn compact_numbers_keep_the_suffix_and_take_the_locale_separator() {
    let english = Formats::new(&locale!("en-US"));
    let german = Formats::new(&locale!("de-DE"));
    assert_eq!(compact_number(&english, 12_345.0), "12.3k");
    assert_eq!(compact_number(&german, 12_345.0), "12,3k");
    assert_eq!(compact_number(&german, 1_500_000.0), "1,5M");
    assert_eq!(compact_number(&german, 9_999.0), "9.999");
    assert_eq!(compact_number(&english, 2.0), "2");
    assert_eq!(compact_number(&english, 0.126), "0.13");
    assert_eq!(compact_number(&english, f64::INFINITY), "–");
}
