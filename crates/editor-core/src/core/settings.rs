use crate::core::{
    config_file::{ReadConfigError, UpdateConfigError},
    hosts::EditorSettings,
};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Debug)]
pub enum Settings {
    Loaded {
        values: EditorSettings,
        rejected: BTreeMap<String, Arc<StoredSettingError>>,
    },
    Unreadable(Arc<ReadConfigError>),
}

#[derive(Debug, thiserror::Error)]
pub enum StoredSettingError {
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
                    rejected.insert(key, Arc::new(error));
                }
            }
        }
        Self::Loaded { values, rejected }
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<&serde_json::Value> {
        match self {
            Self::Loaded { values, .. } => values.get(key),
            Self::Unreadable(_) => None,
        }
    }

    #[must_use]
    pub fn rejected(&self, key: &str) -> Option<&Arc<StoredSettingError>> {
        match self {
            Self::Loaded { rejected, .. } => rejected.get(key),
            Self::Unreadable(_) => None,
        }
    }

    pub(crate) fn values(&self) -> crate::core::hosts::EditorSettings {
        match self {
            Self::Loaded { values, .. } => values.clone(),
            Self::Unreadable(_) => crate::core::hosts::EditorSettings::new(),
        }
    }

    pub(crate) fn put(&mut self, key: &str, value: Option<serde_json::Value>) {
        match (self, value) {
            (Self::Loaded { values, rejected }, Some(value)) => {
                rejected.remove(key);
                values.insert(key.to_owned(), value);
            }
            (Self::Loaded { values, rejected }, None) => {
                rejected.remove(key);
                values.remove(key);
            }
            (Self::Unreadable(_), _) => {}
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

pub(crate) fn check(key: &str, value: &serde_json::Value) -> Result<(), StoredSettingError> {
    match artisan_api::setting_decl(key) {
        None => Ok(()),
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
