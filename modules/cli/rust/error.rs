use std::{io, path::PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("Artisan is not installed correctly: {0}")]
    Installation(String),
    #[error("Forge is not configured in this Artisan home; run `ae setup`")]
    MissingInstance,
    #[error("Forge is not running")]
    NotRunning,
    #[error("Forge control request failed: {0}")]
    Control(String),
    #[error("unsupported operation: {0}")]
    Unsupported(String),
    #[error("development build guard: {0}")]
    DebugBuildGuard(String),
    #[error("refusing unsafe filesystem operation on {0}")]
    UnsafePath(PathBuf),
    #[error("{context}: {source}")]
    Io {
        context: &'static str,
        #[source]
        source: io::Error,
    },
    #[error("invalid JSON in {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

impl CliError {
    pub const fn exit_code(&self) -> i32 {
        match self {
            Self::MissingInstance | Self::NotRunning => 3,
            Self::Installation(_) => 4,
            _ => 1,
        }
    }
}

pub type Result<T> = std::result::Result<T, CliError>;

pub(crate) fn io(context: &'static str) -> impl FnOnce(io::Error) -> CliError {
    move |source| CliError::Io { context, source }
}
