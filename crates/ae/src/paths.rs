//! Where things live on this host.

use anyhow::{Context, Result};
use std::path::PathBuf;

/// Forge's data: `$XDG_DATA_HOME/artisan/forge` (`~/.local/share/…`) or
/// `%LOCALAPPDATA%\Artisan\forge`.
pub fn forge_data() -> Result<PathBuf> {
    Ok(if cfg!(windows) {
        local_app_data()?.join("Artisan").join("forge")
    } else {
        xdg_data()?.join("artisan").join("forge")
    })
}

pub fn home() -> Result<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .context("the home directory is not set")
}

pub fn xdg_data() -> Result<PathBuf> {
    match std::env::var_os("XDG_DATA_HOME").filter(|dir| !dir.is_empty()) {
        Some(dir) => Ok(PathBuf::from(dir)),
        None => Ok(home()?.join(".local").join("share")),
    }
}

pub fn xdg_config() -> Result<PathBuf> {
    match std::env::var_os("XDG_CONFIG_HOME").filter(|dir| !dir.is_empty()) {
        Some(dir) => Ok(PathBuf::from(dir)),
        None => Ok(home()?.join(".config")),
    }
}

pub fn local_app_data() -> Result<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .context("LOCALAPPDATA is not set")
}
