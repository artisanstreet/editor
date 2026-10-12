//! The install root: `versions/<v>/`, `current` and `previous` (text files
//! naming a version, so Windows needs no symlinks), and `bin/ae`, a copy of
//! the current `ae` at a path that never changes (PATH, the service unit
//! and the URL handler point at it).

use super::release::usable_name;
use anyhow::{Context, Result};
use std::env::consts::EXE_SUFFIX;
use std::fs;
use std::path::{Path, PathBuf};

pub const EDITOR: &str = "artisan-editor";

pub struct Root {
    pub dir: PathBuf,
}

impl Root {
    /// `ARTISAN_HOME`, else `$XDG_DATA_HOME/artisan` or `%LOCALAPPDATA%\Artisan`.
    pub fn locate() -> Result<Self> {
        let dir = match std::env::var_os("ARTISAN_HOME").filter(|dir| !dir.is_empty()) {
            Some(dir) => PathBuf::from(dir),
            None if cfg!(windows) => crate::paths::local_app_data()?.join("Artisan"),
            None => crate::paths::xdg_data()?.join("artisan"),
        };
        Ok(Self { dir })
    }

    pub fn versions(&self) -> PathBuf {
        self.dir.join("versions")
    }

    pub fn version_dir(&self, version: &str) -> PathBuf {
        self.versions().join(version)
    }

    pub fn bin(&self) -> PathBuf {
        self.dir.join("bin")
    }

    /// The stable `ae`.
    pub fn ae(&self) -> PathBuf {
        self.bin().join(format!("ae{EXE_SUFFIX}"))
    }

    pub fn current(&self) -> Option<String> {
        read_pointer(&self.dir.join("current"))
    }

    pub fn previous(&self) -> Option<String> {
        read_pointer(&self.dir.join("previous"))
    }

    pub fn editor(&self, version: &str) -> PathBuf {
        self.version_dir(version)
            .join(format!("{EDITOR}{EXE_SUFFIX}"))
    }

    pub fn current_editor(&self) -> Option<PathBuf> {
        self.current()
            .map(|version| self.editor(&version))
            .filter(|path| path.is_file())
    }

    /// Unpacks a verified archive into `versions/<version>` (kept when it is
    /// already there).
    pub fn unpack(&self, version: &str, file_name: &str, archive: &[u8]) -> Result<PathBuf> {
        let dir = self.version_dir(version);
        if dir.join(format!("ae{EXE_SUFFIX}")).is_file() {
            return Ok(dir);
        }
        let partial = self.versions().join(format!(".{version}.partial"));
        let _ = fs::remove_dir_all(&partial);
        super::release::extract(file_name, archive, &partial)?;
        let _ = fs::remove_dir_all(&dir);
        fs::rename(&partial, &dir).with_context(|| format!("move {} into place", dir.display()))?;
        Ok(dir)
    }

    /// Makes `version` current, keeps the old current as previous, refreshes
    /// `bin/ae` and deletes every other version.
    pub fn activate(&self, version: &str) -> Result<()> {
        let source = self.version_dir(version).join(format!("ae{EXE_SUFFIX}"));
        fs::create_dir_all(self.bin())
            .with_context(|| format!("create {}", self.bin().display()))?;
        let next = self.bin().join(format!("ae.next{EXE_SUFFIX}"));
        fs::copy(&source, &next)
            .with_context(|| format!("copy {} to {}", source.display(), next.display()))?;
        let stable = self.ae();
        if cfg!(windows) && stable.exists() {
            // A running exe can't be replaced on Windows, but it can be renamed.
            let old = self.bin().join(format!("ae.old{EXE_SUFFIX}"));
            let _ = fs::remove_file(&old);
            fs::rename(&stable, &old)
                .with_context(|| format!("move the running {} aside", stable.display()))?;
        }
        fs::rename(&next, &stable).with_context(|| format!("replace {}", stable.display()))?;

        let current = self.current();
        if current.as_deref() != Some(version) {
            if let Some(old) = &current {
                write_pointer(&self.dir.join("previous"), old)?;
            }
            write_pointer(&self.dir.join("current"), version)?;
        }
        self.prune();
        Ok(())
    }

    /// Everything here but Forge's data and the user's settings.
    pub fn program_paths(&self) -> Vec<PathBuf> {
        ["versions", "bin", "current", "previous"]
            .iter()
            .map(|name| self.dir.join(name))
            .collect()
    }

    fn prune(&self) {
        let keep = [self.current(), self.previous()];
        let Ok(entries) = fs::read_dir(self.versions()) else {
            return;
        };
        for entry in entries.flatten() {
            if keep
                .iter()
                .flatten()
                .any(|version| entry.file_name() == version.as_str())
            {
                continue;
            }
            // A still-running old Editor pins its files on Windows; the next install retries.
            if let Err(error) = fs::remove_dir_all(entry.path()) {
                tracing::warn!(path = %entry.path().display(), %error, "could not remove an old version");
            }
        }
        let _ = fs::remove_file(self.bin().join(format!("ae.old{EXE_SUFFIX}")));
    }
}

fn read_pointer(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let version = text.trim();
    usable_name(version).then(|| version.to_owned())
}

fn write_pointer(path: &Path, version: &str) -> Result<()> {
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, format!("{version}\n"))
        .with_context(|| format!("write {}", temporary.display()))?;
    fs::rename(&temporary, path).with_context(|| format!("replace {}", path.display()))
}
