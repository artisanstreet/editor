use chrono::{Datelike as _, NaiveDate, NaiveDateTime, Timelike as _};
use icu_calendar::Gregorian;
use icu_datetime::FixedCalendarDateTimeFormatter;
use icu_datetime::fieldsets::{T, YMD};
use icu_datetime::input::{Date, DateTime, Time};
use icu_datetime::options::YearStyle;
use icu_decimal::DecimalFormatter;
use icu_decimal::input::{Decimal, SignDisplay};
use icu_decimal::options::DecimalFormatterOptions;
use icu_locale_core::{Locale, locale};
use std::fmt;
use std::sync::LazyLock;

mod system;

pub(crate) use system::SystemLocales;

pub(crate) const FALLBACK: Locale = locale!("en-US");
const NOT_A_NUMBER: &str = "–";

pub struct Formats {
    decimal: DecimalFormatter,
    clock: FixedCalendarDateTimeFormatter<Gregorian, T>,
    date: FixedCalendarDateTimeFormatter<Gregorian, YMD>,
}

impl fmt::Debug for Formats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Formats").finish_non_exhaustive()
    }
}

static SYSTEM_LOCALES: LazyLock<SystemLocales> = LazyLock::new(system::detect);

thread_local! {
    static SYSTEM: Formats = Formats::for_system(&SYSTEM_LOCALES);
}

pub(crate) fn with_system<R>(format: impl FnOnce(&Formats) -> R) -> R {
    SYSTEM.with(format)
}

pub fn count(value: impl Into<Decimal>, one: &str, other: &str) -> String {
    with_system(|formats| formats.count(value, one, other))
}

pub fn integer(value: impl Into<Decimal>) -> String {
    with_system(|formats| formats.integer(value))
}

pub fn decimal(value: f64, fraction_digits: usize) -> String {
    with_system(|formats| formats.decimal(value, fraction_digits))
}

impl Formats {
    #[cfg(test)]
    pub(crate) fn new(locale: &Locale) -> Self {
        Self::for_system(&SystemLocales::uniform(locale.clone()))
    }

    fn for_system(locales: &SystemLocales) -> Self {
        Self {
            decimal: decimal_formatter(&locales.numbers),
            clock: clock_formatter(&locales.time),
            date: date_formatter(&locales.time),
        }
    }

    fn integer(&self, value: impl Into<Decimal>) -> String {
        self.decimal.format(&value.into()).to_string()
    }

    fn count(&self, value: impl Into<Decimal>, one: &str, other: &str) -> String {
        let value = value.into();
        let noun = if value == Decimal::from(1_u8) {
            one
        } else {
            other
        };
        format!("{} {noun}", self.decimal.format(&value))
    }

    fn decimal(&self, value: f64, fraction_digits: usize) -> String {
        match fixed(value, fraction_digits) {
            Some(value) => self.decimal.format(&value).to_string(),
            None => NOT_A_NUMBER.to_owned(),
        }
    }

    pub(crate) fn trimmed(&self, value: f64, max_fraction_digits: usize) -> String {
        match fixed(value, max_fraction_digits) {
            Some(mut value) => {
                value.absolute.trim_end();
                self.decimal.format(&value).to_string()
            }
            None => NOT_A_NUMBER.to_owned(),
        }
    }

    pub(crate) fn clock(&self, at: NaiveDateTime) -> String {
        self.clock.format(&datetime(at)).to_string()
    }

    pub(crate) fn date(&self, on: NaiveDate) -> String {
        self.date.format(&date(on)).to_string()
    }
}

fn fixed(value: f64, fraction_digits: usize) -> Option<Decimal> {
    value.is_finite().then(|| {
        Decimal::try_from_str(&format!("{value:.fraction_digits$}"))
            .expect("invariant: a finite f64 prints as a plain decimal")
            .with_sign_display(SignDisplay::Negative)
    })
}

fn date(on: NaiveDate) -> Date<Gregorian> {
    let month = u8::try_from(on.month()).expect("invariant: chrono months run 1 to 12");
    let day = u8::try_from(on.day()).expect("invariant: chrono days run 1 to 31");
    Date::try_new_gregorian(on.year(), month, day)
        .expect("invariant: a chrono date is a valid Gregorian date")
}

fn datetime(at: NaiveDateTime) -> DateTime<Gregorian> {
    let hour = u8::try_from(at.hour()).expect("invariant: chrono hours run 0 to 23");
    let minute = u8::try_from(at.minute()).expect("invariant: chrono minutes run 0 to 59");
    DateTime {
        date: date(at.date()),
        time: Time::try_new(hour, minute, 0, 0)
            .expect("invariant: a chrono hour and minute form a valid time"),
    }
}

fn decimal_formatter(locale: &Locale) -> DecimalFormatter {
    DecimalFormatter::try_new(locale.into(), DecimalFormatterOptions::default()).unwrap_or_else(
        |error| {
            tracing::warn!(%locale, %error, "load number formats; formatting numbers for en-US");
            DecimalFormatter::try_new((&FALLBACK).into(), DecimalFormatterOptions::default())
                .expect("invariant: compiled ICU data formats en-US numbers")
        },
    )
}

fn clock_formatter(locale: &Locale) -> FixedCalendarDateTimeFormatter<Gregorian, T> {
    FixedCalendarDateTimeFormatter::try_new(locale.into(), T::hm()).unwrap_or_else(|error| {
        tracing::warn!(%locale, %error, "load clock formats; formatting times for en-US");
        FixedCalendarDateTimeFormatter::try_new((&FALLBACK).into(), T::hm())
            .expect("invariant: compiled ICU data formats en-US times")
    })
}

fn date_formatter(locale: &Locale) -> FixedCalendarDateTimeFormatter<Gregorian, YMD> {
    let fields = YMD::short().with_year_style(YearStyle::Full);
    FixedCalendarDateTimeFormatter::try_new(locale.into(), fields).unwrap_or_else(|error| {
        tracing::warn!(%locale, %error, "load date formats; formatting dates for en-US");
        FixedCalendarDateTimeFormatter::try_new((&FALLBACK).into(), fields)
            .expect("invariant: compiled ICU data formats en-US dates")
    })
}

#[cfg(test)]
#[path = "../../tests/ui/locale.rs"]
mod tests;
