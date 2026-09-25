//! Removing superseded versions of an installation.
//!
//! Every install leaves the previous version directory in place, which is
//! what makes re-activating it (rollback) instant. Pruning bounds that
//! history: the active version and the most recent others are kept, and a
//! version any running Editor or Forge was launched from is never touched.

use std::path::Path;

use crate::error::{InstallerError, Result, io};

use super::{
    authority::{InstallerLock, RootMode},
    state::{
        read_installed_state, recover_activation_pointer_swap, remove_path_in_root,
        validate_state_root,
    },
};

/// What one prune removed and kept.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PruneReport {
    /// Versions removed, most recent first.
    pub removed: Vec<String>,
    /// Inactive versions kept for rollback, most recent first.
    pub kept: Vec<String>,
    /// Versions that would be removed but are running.
    pub in_use: Vec<String>,
}

/// Removes inactive versions of the installation at `root`, keeping the
/// active version and the `keep` most recently installed others.
///
/// # Errors
///
/// Returns [`InstallerError`] when the root is busy or invalid, running
/// processes cannot be discovered, or a version cannot be removed safely.
pub fn prune(root: &Path, keep: usize) -> Result<PruneReport> {
    let root_lock = InstallerLock::acquire(root, RootMode::Existing)?;
    root_lock.fence()?;
    recover_activation_pointer_swap(&root_lock)?;
    let state = read_installed_state(root)?;
    validate_state_root(root, &state)?;
    let versions = root.join("versions");
    let running = crate::processes::running_versions(&versions)?;
    let mut candidates = Vec::new();
    for entry in std::fs::read_dir(&versions).map_err(io(&versions))? {
        let entry = entry.map_err(io(&versions))?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if name == state.active_version || name.starts_with('.') {
            continue;
        }
        if !entry.file_type().map_err(io(entry.path()))?.is_dir() {
            continue;
        }
        // The payload manifest is written once, when the version is staged.
        let installed = std::fs::metadata(entry.path().join(crate::payload::PAYLOAD_MANIFEST_NAME))
            .or_else(|_| entry.metadata())
            .and_then(|metadata| metadata.modified())
            .map_err(io(entry.path()))?;
        candidates.push((installed, name));
    }
    candidates.sort_by(|left, right| right.cmp(left));
    let mut report = PruneReport::default();
    for (index, (_, name)) in candidates.into_iter().enumerate() {
        if index < keep {
            report.kept.push(name);
        } else if running.contains(&name) {
            report.in_use.push(name);
        } else {
            root_lock.fence()?;
            remove_path_in_root(&versions, &versions.join(&name)).map_err(|error| {
                InstallerError::InvalidInstallation(format!(
                    "version {name} could not be removed: {error}"
                ))
            })?;
            report.removed.push(name);
        }
    }
    Ok(report)
}
