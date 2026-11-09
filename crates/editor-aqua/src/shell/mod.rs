pub mod connection;
pub mod context_meter;
pub mod layout;
pub mod menu_keys;
pub mod resource_preview;

pub mod application;
pub mod dialogs;
pub mod entry;
pub mod host;
pub mod live_throttle;
pub mod native_notifications;
pub mod notifications;
pub mod palette;
pub mod preferences;
pub mod rebuild_stats;
pub mod refresh;
pub mod repository;
pub mod root;
pub mod selection;
pub mod source;
pub mod thread_navigation;
pub mod titlebar;
pub mod view;
pub mod wheel;
pub mod workspace;

#[cfg(windows)]
pub(crate) fn open_url(url: &str) -> anyhow::Result<()> {
    aqua::prelude::OpenTarget::url(url)?.execute()?;
    Ok(())
}

#[cfg(not(windows))]
pub(crate) fn open_url(url: &str) -> anyhow::Result<()> {
    aqua::prelude::OpenTarget::url(url)?;
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "the Aqua native URL service runs on Windows only",
    )
    .into())
}
