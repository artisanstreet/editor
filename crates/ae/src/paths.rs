use artisan_paths::{App, RootError};
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
#[error("find where {} lives", app.display_name())]
pub struct LocateError {
    pub app: App,
    #[source]
    pub source: RootError,
}

pub fn binary(app: App) -> Result<PathBuf, LocateError> {
    app.binary().map_err(|source| LocateError { app, source })
}

pub fn dir(app: App) -> Result<PathBuf, LocateError> {
    app.dir().map_err(|source| LocateError { app, source })
}

pub fn data(app: App) -> Result<PathBuf, LocateError> {
    app.data().map_err(|source| LocateError { app, source })
}
