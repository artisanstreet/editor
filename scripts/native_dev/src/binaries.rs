//! Locate Cargo-built payload binaries beside the launcher or in --bin-dir.
use crate::{error::DevError, paths::exe_name};
use std::path::{Path, PathBuf};

/// Located build outputs for the four payload binaries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BinarySet {
    /// Staged `ae` build output.
    pub ae: PathBuf,
    /// Staged `editor` build output.
    pub editor: PathBuf,
    /// Staged `forge` build output.
    pub forge: PathBuf,
    /// Staged `installer` build output.
    pub installer: PathBuf,
}

impl BinarySet {
    /// Iterates `(payload-relative-name, source-path)` pairs.
    #[must_use]
    pub fn entries(&self) -> [(String, PathBuf); 4] {
        [
            (format!("bin/{}", exe_name("ae")), self.ae.clone()),
            (
                format!("bin/{}", exe_name("installer")),
                self.installer.clone(),
            ),
            (format!("bin/{}", exe_name("editor")), self.editor.clone()),
            (format!("bin/{}", exe_name("forge")), self.forge.clone()),
        ]
    }
}

/// Locates payload binaries in an explicit directory or beside this executable.
///
/// # Errors
/// Returns an error if the executable directory or any payload is missing.
pub fn locate_binaries(explicit: Option<&Path>) -> Result<BinarySet, DevError> {
    if let Some(directory) = explicit {
        return locate_in_dir(directory);
    }
    let executable = std::env::current_exe().map_err(|error| DevError::BinaryMissing {
        name: "dev".to_owned(),
        hint: error.to_string(),
    })?;
    let directory = executable.parent().ok_or_else(|| DevError::BinaryMissing {
        name: "dev".to_owned(),
        hint: "pass --bin-dir with the Cargo output directory".to_owned(),
    })?;
    locate_in_dir(directory)
}

/// Locates the four binaries inside one explicit directory.
///
/// # Errors
///
/// Returns [`DevError::BinaryMissing`] when any binary is absent.
pub fn locate_in_dir(directory: &Path) -> Result<BinarySet, DevError> {
    let mut missing = Vec::new();
    let mut get = |stem: &str| {
        let path = directory.join(exe_name(stem));
        if path.is_file() {
            Some(path)
        } else {
            missing.push(exe_name(stem));
            None
        }
    };
    let set = BinarySet {
        ae: get("ae").unwrap_or_default(),
        editor: get("editor").unwrap_or_default(),
        forge: get("forge").unwrap_or_default(),
        installer: get("installer").unwrap_or_default(),
    };
    if missing.is_empty() {
        Ok(set)
    } else {
        Err(DevError::BinaryMissing {
            name: missing.join(", "),
            hint: format!("--bin-dir {}", directory.display()),
        })
    }
}
