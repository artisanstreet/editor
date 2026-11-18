use crate::core::{
    config_file::{ReadConfigError, UpdateConfigError},
    hosts::EditorSettings,
};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub enum Settings {
    Loaded(EditorSettings),
    Unreadable(Arc<ReadConfigError>),
}

#[derive(Debug, thiserror::Error)]
pub enum SetSettingError {
    #[error("settings.json could not be read, so {key} was not saved until it is recovered")]
    Unreadable {
        key: String,
        #[source]
        source: Arc<ReadConfigError>,
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
    pub fn read(result: Result<EditorSettings, ReadConfigError>) -> Self {
        match result {
            Ok(settings) => Self::Loaded(settings),
            Err(error) => Self::Unreadable(Arc::new(error)),
        }
    }

    #[must_use]
    pub fn get(&self, key: &str) -> Option<&serde_json::Value> {
        match self {
            Self::Loaded(settings) => settings.get(key),
            Self::Unreadable(_) => None,
        }
    }

    #[must_use]
    pub fn unreadable(&self) -> Option<&ReadConfigError> {
        match self {
            Self::Loaded(_) => None,
            Self::Unreadable(error) => Some(error),
        }
    }
}
