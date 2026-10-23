//! Where Artisan's apps live. Each app has one folder under the Artisan
//! Street root, holding its binary and its `data` folder:
//!
//! - Windows: `%LOCALAPPDATA%\Artisan Street\{ae,Forge,Editor}`
//! - macOS: `~/Library/Application Support/Artisan Street/{ae,Forge,Editor}`
//! - Linux: `$XDG_DATA_HOME/artisan-street/{ae,forge,editor}`
//!
//! One version of each app is installed; installing replaces its binary.

use std::io::{Error, ErrorKind, Result};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum App {
    Ae,
    Forge,
    Editor,
}

impl App {
    pub const ALL: [Self; 3] = [Self::Ae, Self::Forge, Self::Editor];

    /// The name on the command line and in release artifacts.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Ae => "ae",
            Self::Forge => "forge",
            Self::Editor => "editor",
        }
    }

    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Ae => "ae",
            Self::Forge => "Forge",
            Self::Editor => "Artisan Editor",
        }
    }

    /// The executable's file name on this OS.
    #[must_use]
    pub fn binary_name(self) -> String {
        self.binary_name_on(std::env::consts::OS)
    }

    /// The executable's file name on `os` (`std::env::consts::OS` naming).
    /// On Windows the Editor is `Artisan Editor.exe`, the name Task Manager
    /// and Alt+Tab show; elsewhere it keeps the command-line name.
    #[must_use]
    pub fn binary_name_on(self, os: &str) -> String {
        let windows = os == "windows";
        let stem = match self {
            Self::Ae => "ae",
            Self::Forge => "forge",
            Self::Editor if windows => "Artisan Editor",
            Self::Editor => "artisan-editor",
        };
        if windows {
            format!("{stem}.exe")
        } else {
            stem.to_owned()
        }
    }

    const fn folder(self) -> &'static str {
        match (self, cfg!(any(windows, target_os = "macos"))) {
            (Self::Ae, _) => "ae",
            (Self::Forge, true) => "Forge",
            (Self::Editor, true) => "Editor",
            (Self::Forge, false) => "forge",
            (Self::Editor, false) => "editor",
        }
    }

    /// The app's folder.
    pub fn dir(self) -> Result<PathBuf> {
        Ok(root()?.join(self.folder()))
    }

    /// The installed executable.
    pub fn binary(self) -> Result<PathBuf> {
        Ok(self.dir()?.join(self.binary_name()))
    }

    /// The app's own data (settings, database, logs).
    pub fn data(self) -> Result<PathBuf> {
        Ok(self.dir()?.join("data"))
    }
}

/// The Artisan Street folder every app lives in.
pub fn root() -> Result<PathBuf> {
    if cfg!(windows) {
        Ok(var("LOCALAPPDATA")?.join("Artisan Street"))
    } else if cfg!(target_os = "macos") {
        Ok(home()?.join("Library/Application Support/Artisan Street"))
    } else {
        Ok(xdg_data()?.join("artisan-street"))
    }
}

/// The user's home folder.
pub fn home() -> Result<PathBuf> {
    var(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
}

/// `$XDG_DATA_HOME`, else `~/.local/share`.
pub fn xdg_data() -> Result<PathBuf> {
    var("XDG_DATA_HOME").or_else(|_| Ok(home()?.join(".local/share")))
}

/// `$XDG_CONFIG_HOME`, else `~/.config`.
pub fn xdg_config() -> Result<PathBuf> {
    var("XDG_CONFIG_HOME").or_else(|_| Ok(home()?.join(".config")))
}

fn var(name: &str) -> Result<PathBuf> {
    std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| Error::new(ErrorKind::NotFound, format!("{name} is not set")))
}
