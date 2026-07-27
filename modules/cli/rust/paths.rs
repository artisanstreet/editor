use std::{env, path::PathBuf};

use crate::{CliError, Result};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Layout {
    pub root: PathBuf,
    pub profiles: PathBuf,
    pub manifest: PathBuf,
}

impl Layout {
    pub fn discover() -> Result<Self> {
        let root = match env::var_os("ARTISAN_HOME") {
            Some(value) => PathBuf::from(value),
            None => platform_root()?,
        };
        Ok(Self {
            profiles: root.join("profiles"),
            manifest: root.join("installation.json"),
            root,
        })
    }
}

fn platform_root() -> Result<PathBuf> {
    #[cfg(target_os = "windows")]
    let value = env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|path| path.join("Artisan"));
    #[cfg(target_os = "macos")]
    let value = env::var_os("HOME")
        .map(PathBuf::from)
        .map(|path| path.join("Library/Application Support/Artisan"));
    #[cfg(all(unix, not(target_os = "macos")))]
    let value = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .map(|path| path.join("artisan"));
    value.ok_or_else(|| CliError::Installation("user data directory is unavailable".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_keeps_manifest_and_profiles_under_root() {
        let root = PathBuf::from("example");
        let layout = Layout {
            profiles: root.join("profiles"),
            manifest: root.join("installation.json"),
            root: root.clone(),
        };
        assert!(layout.manifest.starts_with(&root));
        assert!(layout.profiles.starts_with(&root));
    }
}
