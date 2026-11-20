pub mod composer_glide;
pub mod connection;
pub mod context_meter;
pub mod layout;
pub mod link_failure;
pub use crate::ui::menu_keys;
pub mod resource_preview;

pub mod application;
pub use crate::ui::dialogs;
pub mod entry;
pub mod host;
pub mod live_batch;
pub mod live_throttle;
pub mod native_notifications;
pub mod notifications;
pub mod palette;
pub use crate::core::diagnostics::rebuild_stats;
pub use crate::ui::preferences;
pub mod browser;
pub mod refresh;
pub mod repository;
pub mod root;
pub mod selection;
pub mod source;
pub(crate) mod stopwatch;
pub mod thread_navigation;
pub mod titlebar;
pub mod view;
pub mod wheel;
pub mod workspace;

#[derive(Debug, thiserror::Error)]
pub enum OpenUrlError {
    #[error("the URL is not one the Editor opens")]
    Target(#[source] aqua::prelude::OpenError),
    #[cfg(windows)]
    #[error("Windows could not open the URL")]
    Execute(#[source] aqua::prelude::OpenError),
    #[cfg(not(windows))]
    #[error("the Aqua native URL service runs on Windows only")]
    Unsupported,
}

#[derive(Debug, thiserror::Error)]
pub enum OpenPathError {
    #[error("{} is not a file the Editor opens", path.display())]
    Target {
        path: std::path::PathBuf,
        #[source]
        source: aqua::prelude::OpenError,
    },
    #[cfg(windows)]
    #[error("Windows could not open {}", path.display())]
    Execute {
        path: std::path::PathBuf,
        #[source]
        source: aqua::prelude::OpenError,
    },
    #[cfg(not(windows))]
    #[error("the Aqua native shell opens files on Windows only, not {}", path.display())]
    Unsupported { path: std::path::PathBuf },
}

#[cfg(windows)]
pub(crate) fn open_url(url: &str) -> Result<(), OpenUrlError> {
    aqua::prelude::OpenTarget::url(url)
        .map_err(OpenUrlError::Target)?
        .execute()
        .map_err(OpenUrlError::Execute)
}

#[cfg(windows)]
pub(crate) fn open_path(path: &std::path::Path) -> Result<(), OpenPathError> {
    aqua::prelude::OpenTarget::path(path)
        .map_err(|source| OpenPathError::Target {
            path: path.to_owned(),
            source,
        })?
        .execute()
        .map_err(|source| OpenPathError::Execute {
            path: path.to_owned(),
            source,
        })
}

#[cfg(not(windows))]
pub(crate) fn open_path(path: &std::path::Path) -> Result<(), OpenPathError> {
    aqua::prelude::OpenTarget::path(path).map_err(|source| OpenPathError::Target {
        path: path.to_owned(),
        source,
    })?;
    Err(OpenPathError::Unsupported {
        path: path.to_owned(),
    })
}

#[cfg(not(windows))]
pub(crate) fn open_url(url: &str) -> Result<(), OpenUrlError> {
    aqua::prelude::OpenTarget::url(url).map_err(OpenUrlError::Target)?;
    Err(OpenUrlError::Unsupported)
}
