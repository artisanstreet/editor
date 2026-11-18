use super::FALLBACK;
use icu_locale_core::Locale;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SystemLocales {
    pub numbers: Locale,
    pub time: Locale,
}

impl SystemLocales {
    pub fn from_environment(variable: impl Fn(&str) -> Option<String>) -> Self {
        let category = |name: &str| {
            ["LC_ALL", name, "LANG"]
                .into_iter()
                .find_map(|key| variable(key).filter(|value| !value.is_empty()))
                .and_then(|value| posix_locale(&value))
                .unwrap_or(FALLBACK)
        };
        Self {
            numbers: category("LC_NUMERIC"),
            time: category("LC_TIME"),
        }
    }

    pub fn uniform(locale: Locale) -> Self {
        Self {
            numbers: locale.clone(),
            time: locale,
        }
    }
}

pub fn posix_locale(value: &str) -> Option<Locale> {
    let name = value
        .split(['.', '@'])
        .next()
        .expect("invariant: split yields at least one piece");
    match name {
        "" | "C" | "POSIX" => None,
        name => match Locale::try_from_str(&name.replace('_', "-")) {
            Ok(locale) => Some(locale),
            Err(error) => {
                tracing::warn!(value, %error, "parse the locale setting; formatting for en-US");
                None
            }
        },
    }
}

#[cfg(windows)]
pub(super) fn detect() -> SystemLocales {
    let read = windows_registry::CURRENT_USER
        .open(r"Control Panel\International")
        .and_then(|key| key.get_string("LocaleName"));
    SystemLocales::uniform(match read {
        Ok(name) => match Locale::try_from_str(&name) {
            Ok(locale) => locale,
            Err(error) => {
                tracing::warn!(name, %error, "parse the Windows user locale; formatting for en-US");
                FALLBACK
            }
        },
        Err(error) => {
            tracing::warn!(%error, "read the Windows user locale; formatting for en-US");
            FALLBACK
        }
    })
}

#[cfg(not(windows))]
pub(super) fn detect() -> SystemLocales {
    SystemLocales::from_environment(|key| {
        std::env::var_os(key).map(|value| value.to_string_lossy().into_owned())
    })
}
