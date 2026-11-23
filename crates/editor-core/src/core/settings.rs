use crate::core::{
    config_file::{ReadConfigError, UpdateConfigError},
    hosts::EditorSettings,
};
use artisan_api::Switch;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    sync::{Arc, LazyLock},
};

#[derive(Clone, Debug)]
pub enum Settings {
    Loaded {
        values: EditorSettings,
        rejected: BTreeMap<String, Rejected>,
        preferences: EditorPreferences,
    },
    Unreadable(Arc<ReadConfigError>),
}

#[derive(Clone, Debug)]
pub struct Rejected {
    value: Value,
    error: Arc<StoredSettingError>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Fonts {
    #[default]
    Spline,
    System,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Streaming {
    #[default]
    Smooth,
    Swift,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Browser {
    SystemDefault,
    Named(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemePreference {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemePreference {
    pub(crate) const fn from_dark(dark: bool) -> Self {
        if dark { Self::Dark } else { Self::Light }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditorPreferences {
    pub theme: ThemePreference,
    pub outline_text: Switch,
    pub notifications: Switch,
    pub lower_refresh_rate: Switch,
    pub graphics_vsync: Switch,
    pub hide_bad_model_options: Switch,
    pub show_all_models: Switch,
    pub reduced_motion: Option<bool>,
    pub high_contrast: Option<bool>,
    pub text_percent: Option<u32>,
    pub fonts: Fonts,
    pub streaming: Streaming,
    pub browser: Browser,
}

impl EditorPreferences {
    fn decode<'a>(stored: impl Fn(&str) -> Option<&'a Value>) -> Self {
        let switch = |key: &str| artisan_api::switch_decl(key).switch(stored(key));
        let declared = |key: &str| {
            let decl = artisan_api::setting_decl(key)
                .unwrap_or_else(|| unreachable!("invariant: {key} is a declared Editor setting"));
            stored(key)
                .filter(|value| decl.check(value).is_ok())
                .cloned()
                .unwrap_or_else(|| decl.default_value())
        };
        let system_switch = |key: &str| match declared(key) {
            Value::Bool(on) => Some(on),
            Value::Null => None,
            Value::Number(_) | Value::String(_) | Value::Array(_) | Value::Object(_) => {
                accepted(key)
            }
        };
        let text = |key: &str| match declared(key) {
            Value::String(text) => text,
            Value::Null
            | Value::Bool(_)
            | Value::Number(_)
            | Value::Array(_)
            | Value::Object(_) => accepted(key),
        };
        let curated = stored("curated_models").and_then(Value::as_bool);
        Self {
            theme: match stored("theme").and_then(Value::as_str) {
                Some("light") => ThemePreference::Light,
                Some("dark") => ThemePreference::Dark,
                Some("system") => ThemePreference::System,
                _ => stored("dark_mode")
                    .and_then(Value::as_bool)
                    .map_or(ThemePreference::System, ThemePreference::from_dark),
            },
            outline_text: switch("outline_text"),
            notifications: switch("notifications"),
            lower_refresh_rate: switch("lower_refresh_rate"),
            graphics_vsync: switch("graphics_vsync"),
            hide_bad_model_options: curated.map_or_else(
                || switch(artisan_api::HIDE_BAD_MODEL_OPTIONS_KEY),
                Switch::Stored,
            ),
            show_all_models: curated.map_or_else(
                || switch(artisan_api::SHOW_ALL_MODELS_KEY),
                |curated| Switch::Stored(!curated),
            ),
            reduced_motion: system_switch("reduced_motion"),
            high_contrast: system_switch("high_contrast"),
            text_percent: match declared("text_scale") {
                Value::Null => None,
                Value::Number(percent) => Some(
                    percent
                        .as_u64()
                        .and_then(|percent| u32::try_from(percent).ok())
                        .unwrap_or_else(|| accepted("text_scale")),
                ),
                Value::Bool(_) | Value::String(_) | Value::Array(_) | Value::Object(_) => {
                    accepted("text_scale")
                }
            },
            fonts: match text("font_set").as_str() {
                "spline" => Fonts::Spline,
                "system" => Fonts::System,
                _ => accepted("font_set"),
            },
            streaming: match text("streaming_mode").as_str() {
                "smooth" => Streaming::Smooth,
                "swift" => Streaming::Swift,
                _ => accepted("streaming_mode"),
            },
            browser: match text("browser").trim() {
                "" => Browser::SystemDefault,
                name => Browser::Named(name.to_owned()),
            },
        }
    }

    fn of(values: &EditorSettings, rejected: &BTreeMap<String, Rejected>) -> Self {
        Self::decode(|key| {
            values
                .get(key)
                .or_else(|| rejected.get(key).map(|rejected| &rejected.value))
        })
    }
}

fn accepted(key: &str) -> ! {
    unreachable!("invariant: {key} holds a value its declaration accepts")
}

static UNSET: LazyLock<EditorPreferences> = LazyLock::new(|| EditorPreferences::decode(|_| None));

#[derive(Debug, thiserror::Error)]
pub enum StoredSettingError {
    #[error("settings.json holds {key}, which is no Editor setting")]
    Undeclared { key: String },
    #[error("settings.json holds {key}, which is a Forge setting")]
    ForgeOwned { key: String },
    #[error("settings.json holds an invalid {key}")]
    Invalid {
        key: String,
        #[source]
        source: artisan_api::SettingValueError,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum SetSettingError {
    #[error("settings.json could not be read, so {key} was not saved until it is recovered")]
    Unreadable {
        key: String,
        #[source]
        source: Arc<ReadConfigError>,
    },
    #[error("refuse {key}")]
    Invalid {
        key: String,
        #[source]
        source: StoredSettingError,
    },
    #[error("save {key}")]
    Save {
        key: String,
        #[source]
        source: UpdateConfigError,
    },
}

impl Settings {
    #[must_use]
    pub(crate) fn read(result: Result<EditorSettings, ReadConfigError>) -> Self {
        match result {
            Ok(stored) => Self::validated(stored),
            Err(error) => Self::Unreadable(Arc::new(error)),
        }
    }

    #[must_use]
    pub(crate) fn validated(stored: EditorSettings) -> Self {
        let mut values = EditorSettings::new();
        let mut rejected = BTreeMap::new();
        for (key, value) in stored {
            match check(&key, &value) {
                Ok(()) => {
                    values.insert(key, value);
                }
                Err(error) => {
                    tracing::warn!(
                        key = %key,
                        error = %::diagnostics::chain(&error),
                        "settings.json holds a value the Editor ignores"
                    );
                    rejected.insert(
                        key,
                        Rejected {
                            value,
                            error: Arc::new(error),
                        },
                    );
                }
            }
        }
        let preferences = EditorPreferences::of(&values, &rejected);
        Self::Loaded {
            values,
            rejected,
            preferences,
        }
    }

    #[must_use]
    pub fn preferences(&self) -> &EditorPreferences {
        match self {
            Self::Loaded { preferences, .. } => preferences,
            Self::Unreadable(_) => &UNSET,
        }
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Self::Loaded { values, .. } => values.get(key),
            Self::Unreadable(_) => None,
        }
    }

    #[must_use]
    pub fn rejected(&self, key: &str) -> Option<&Arc<StoredSettingError>> {
        match self {
            Self::Loaded { rejected, .. } => rejected.get(key).map(|rejected| &rejected.error),
            Self::Unreadable(_) => None,
        }
    }

    pub(crate) fn values(&self) -> EditorSettings {
        match self {
            Self::Loaded { values, .. } => values.clone(),
            Self::Unreadable(_) => EditorSettings::new(),
        }
    }

    pub(crate) fn put(&mut self, key: &str, value: Option<Value>) {
        if let Self::Loaded {
            values,
            rejected,
            preferences,
        } = self
        {
            rejected.remove(key);
            match value {
                Some(value) => values.insert(key.to_owned(), value),
                None => values.remove(key),
            };
            *preferences = EditorPreferences::of(values, rejected);
        }
    }

    #[must_use]
    pub fn unreadable(&self) -> Option<&ReadConfigError> {
        match self {
            Self::Loaded { .. } => None,
            Self::Unreadable(error) => Some(error),
        }
    }
}

pub(crate) fn check(key: &str, value: &Value) -> Result<(), StoredSettingError> {
    match artisan_api::setting_decl(key) {
        None => Err(StoredSettingError::Undeclared {
            key: key.to_owned(),
        }),
        Some(decl) if decl.owner == artisan_api::SettingOwner::Forge => {
            Err(StoredSettingError::ForgeOwned {
                key: key.to_owned(),
            })
        }
        Some(decl) => decl
            .check(value)
            .map_err(|source| StoredSettingError::Invalid {
                key: key.to_owned(),
                source,
            }),
    }
}
