pub mod composer_glide;
pub mod connection;
pub mod context_meter;
pub mod layout;
pub use crate::ui::menu_keys;
pub mod resource_preview;

pub mod action_failure;
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
pub mod settings_frame;
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

#[cfg(windows)]
fn open_url(url: &str) -> Result<(), OpenUrlError> {
    aqua::prelude::OpenTarget::url(url)
        .map_err(OpenUrlError::Target)?
        .execute()
        .map_err(OpenUrlError::Execute)
}

#[cfg(not(windows))]
fn open_url(url: &str) -> Result<(), OpenUrlError> {
    aqua::prelude::OpenTarget::url(url).map_err(OpenUrlError::Target)?;
    Err(OpenUrlError::Unsupported)
}
