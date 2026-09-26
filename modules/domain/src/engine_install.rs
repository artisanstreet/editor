//! Forge-managed engine installs, as the Forge reports them.
//!
//! The Forge installs every engine CLI itself, follows the vendor's latest
//! release unless the user holds a version, and keeps previous generations
//! for rollback. It pushes one status per engine whenever anything changes;
//! the Editor renders the status and sends version selections and rollbacks,
//! never deciding install state itself.

use thiserror::Error;

/// Maximum engines in one snapshot.
pub const ENGINE_INSTALL_MAX_ENGINES: usize = 16;
/// Maximum versions in one vendor version list.
pub const ENGINE_VERSION_LIST_MAX: usize = 64;
/// Maximum bytes of one engine identifier.
pub const ENGINE_INSTALL_ID_MAX_BYTES: usize = 64;
/// Maximum bytes of one version string.
pub const ENGINE_VERSION_MAX_BYTES: usize = 128;
/// Maximum bytes of one status reason.
pub const ENGINE_INSTALL_REASON_MAX_BYTES: usize = 512;

/// Where one engine's managed install stands.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EngineInstallPhase {
    /// Not installed yet; the Forge installs it in the background.
    NotInstalled,
    /// Downloading, verifying, or activating a version.
    Installing,
    /// A verified version is active.
    Ready,
    /// The last install or update failed; `reason` says why.
    Failed,
    /// The engine cannot be managed on this Forge's platform.
    Unsupported,
}

impl EngineInstallPhase {
    /// Returns the stable wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotInstalled => "not_installed",
            Self::Installing => "installing",
            Self::Ready => "ready",
            Self::Failed => "failed",
            Self::Unsupported => "unsupported",
        }
    }
}

/// One engine's install status.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct EngineInstallStatus {
    /// Engine identifier (`claude`, `codex`, …).
    pub engine_id: String,
    /// Install phase.
    pub phase: EngineInstallPhase,
    /// Active version, when one is installed (also while updating).
    pub active_version: Option<String>,
    /// The held version; `None` means the engine follows `latest`.
    pub held_version: Option<String>,
    /// The vendor's current release as last observed.
    pub latest_version: Option<String>,
    /// A verified version waiting for the engine to become idle.
    pub pending_version: Option<String>,
    /// The version a rollback would switch to.
    pub rollback_version: Option<String>,
    /// Download progress while installing, 0–100.
    pub progress_percent: Option<u8>,
    /// Presentation-ready reason for `Failed` and `Unsupported`.
    pub reason: Option<String>,
    /// A developer override replaces the managed executable.
    pub overridden: bool,
}

impl EngineInstallStatus {
    /// A newer release exists than the active version while following
    /// `latest`.
    #[must_use]
    pub fn update_available(&self) -> bool {
        self.held_version.is_none()
            && matches!(
                (&self.active_version, &self.latest_version),
                (Some(active), Some(latest)) if active != latest
            )
    }

    /// Validates bounds and field consistency.
    ///
    /// # Errors
    ///
    /// Returns [`EngineInstallError`] for an invalid identifier, version,
    /// reason, or progress value.
    pub fn validate(&self) -> Result<(), EngineInstallError> {
        validate_token(&self.engine_id, ENGINE_INSTALL_ID_MAX_BYTES)
            .ok_or(EngineInstallError::EngineId)?;
        for version in [
            &self.active_version,
            &self.held_version,
            &self.latest_version,
            &self.pending_version,
            &self.rollback_version,
        ]
        .into_iter()
        .flatten()
        {
            validate_token(version, ENGINE_VERSION_MAX_BYTES).ok_or(EngineInstallError::Version)?;
        }
        if self.progress_percent.is_some_and(|percent| percent > 100) {
            return Err(EngineInstallError::Progress);
        }
        if let Some(reason) = &self.reason
            && (reason.trim().is_empty()
                || reason.len() > ENGINE_INSTALL_REASON_MAX_BYTES
                || reason.chars().any(char::is_control))
        {
            return Err(EngineInstallError::Reason);
        }
        Ok(())
    }
}

/// Every managed engine's status.
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct EngineInstallSnapshot {
    engines: Vec<EngineInstallStatus>,
}

impl EngineInstallSnapshot {
    /// Creates a validated snapshot.
    ///
    /// # Errors
    ///
    /// Returns [`EngineInstallError`] for too many or repeated engines, or an
    /// invalid status.
    pub fn new(engines: Vec<EngineInstallStatus>) -> Result<Self, EngineInstallError> {
        if engines.len() > ENGINE_INSTALL_MAX_ENGINES {
            return Err(EngineInstallError::TooMany);
        }
        let mut seen = std::collections::HashSet::with_capacity(engines.len());
        for status in &engines {
            status.validate()?;
            if !seen.insert(status.engine_id.as_str()) {
                return Err(EngineInstallError::Duplicate);
            }
        }
        Ok(Self { engines })
    }

    /// Returns the statuses in Forge order.
    #[must_use]
    pub fn engines(&self) -> &[EngineInstallStatus] {
        &self.engines
    }

    /// Returns one engine's status.
    #[must_use]
    pub fn engine(&self, engine_id: &str) -> Option<&EngineInstallStatus> {
        self.engines
            .iter()
            .find(|status| status.engine_id == engine_id)
    }
}

/// One published vendor version and its local state.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct EngineVersionEntry {
    pub version: String,
    /// A generation of this version is on disk.
    pub installed: bool,
    /// This version is active.
    pub active: bool,
    /// Older than the engine's compatibility floor; cannot be selected.
    pub below_floor: bool,
}

/// The vendor's versions of one engine, newest first.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct EngineVersionList {
    engine_id: String,
    versions: Vec<EngineVersionEntry>,
}

impl EngineVersionList {
    /// Creates a validated list.
    ///
    /// # Errors
    ///
    /// Returns [`EngineInstallError`] for an invalid identifier or version,
    /// or too many versions.
    pub fn new(
        engine_id: String,
        versions: Vec<EngineVersionEntry>,
    ) -> Result<Self, EngineInstallError> {
        validate_token(&engine_id, ENGINE_INSTALL_ID_MAX_BYTES)
            .ok_or(EngineInstallError::EngineId)?;
        if versions.len() > ENGINE_VERSION_LIST_MAX {
            return Err(EngineInstallError::TooMany);
        }
        for entry in &versions {
            validate_token(&entry.version, ENGINE_VERSION_MAX_BYTES)
                .ok_or(EngineInstallError::Version)?;
        }
        Ok(Self {
            engine_id,
            versions,
        })
    }

    /// Returns the engine identifier.
    #[must_use]
    pub fn engine_id(&self) -> &str {
        &self.engine_id
    }

    /// Returns the versions, newest first.
    #[must_use]
    pub fn versions(&self) -> &[EngineVersionEntry] {
        &self.versions
    }
}

/// What the user selects for an engine.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum EngineVersionSelection {
    /// Follow the vendor's latest release.
    Latest,
    /// Hold one exact version.
    Version(String),
}

impl EngineVersionSelection {
    /// Validates a held version.
    ///
    /// # Errors
    ///
    /// Returns [`EngineInstallError::Version`] for an invalid version.
    pub fn validate(&self) -> Result<(), EngineInstallError> {
        match self {
            Self::Latest => Ok(()),
            Self::Version(version) => {
                validate_token(version, ENGINE_VERSION_MAX_BYTES).ok_or(EngineInstallError::Version)
            }
        }
    }
}

/// Reads every managed engine's status. The connection that read them
/// receives later changes pushed as
/// [`Event::EngineInstalls`](crate::Event::EngineInstalls).
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct ReadEngineInstalls;

/// Lists the vendor's published versions of one engine.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ListEngineVersions {
    pub engine_id: String,
}

/// What to do with one engine's version.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum EngineVersionChange {
    /// Follow `latest` or hold a version.
    Select(EngineVersionSelection),
    /// Switch back to the previous generation and hold it.
    Rollback,
}

/// Changes one engine's version. The Forge answers with the snapshot after
/// queuing the change; progress and the outcome arrive as pushed statuses.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ChangeEngineVersion {
    pub engine_id: String,
    pub change: EngineVersionChange,
}

impl ChangeEngineVersion {
    /// Validates the engine identifier and a held version.
    ///
    /// # Errors
    ///
    /// Returns [`EngineInstallError`] for an invalid identifier or version.
    pub fn validate(&self) -> Result<(), EngineInstallError> {
        validate_token(&self.engine_id, ENGINE_INSTALL_ID_MAX_BYTES)
            .ok_or(EngineInstallError::EngineId)?;
        match &self.change {
            EngineVersionChange::Select(selection) => selection.validate(),
            EngineVersionChange::Rollback => Ok(()),
        }
    }
}

/// Validates an engine identifier from the wire.
///
/// # Errors
///
/// Returns [`EngineInstallError::EngineId`] for an invalid identifier.
pub fn validate_engine_id(engine_id: &str) -> Result<(), EngineInstallError> {
    validate_token(engine_id, ENGINE_INSTALL_ID_MAX_BYTES).ok_or(EngineInstallError::EngineId)
}

/// Invalid engine install data.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum EngineInstallError {
    #[error("engine identifier is invalid")]
    EngineId,
    #[error("engine version is invalid")]
    Version,
    #[error("engine install progress is out of range")]
    Progress,
    #[error("engine install reason is invalid")]
    Reason,
    #[error("too many engine install entries")]
    TooMany,
    #[error("an engine appears twice")]
    Duplicate,
}

fn validate_token(value: &str, maximum_bytes: usize) -> Option<()> {
    (!value.is_empty()
        && value.len() <= maximum_bytes
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'+')))
    .then_some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(engine_id: &str) -> EngineInstallStatus {
        EngineInstallStatus {
            engine_id: engine_id.to_owned(),
            phase: EngineInstallPhase::Ready,
            active_version: Some("2.1.282".to_owned()),
            held_version: None,
            latest_version: Some("2.1.283".to_owned()),
            pending_version: None,
            rollback_version: Some("2.1.281".to_owned()),
            progress_percent: None,
            reason: None,
            overridden: false,
        }
    }

    #[test]
    fn snapshots_validate_bounds_and_reject_duplicates() {
        let snapshot = EngineInstallSnapshot::new(vec![status("claude"), status("codex")]).unwrap();
        assert!(snapshot.engine("claude").unwrap().update_available());
        assert_eq!(
            EngineInstallSnapshot::new(vec![status("claude"), status("claude")]),
            Err(EngineInstallError::Duplicate)
        );
        let mut bad = status("claude");
        bad.progress_percent = Some(101);
        assert_eq!(bad.validate(), Err(EngineInstallError::Progress));
        let mut bad = status("claude");
        bad.active_version = Some("2.1 .282".to_owned());
        assert_eq!(bad.validate(), Err(EngineInstallError::Version));
        let mut held = status("claude");
        held.held_version = Some("2.1.282".to_owned());
        assert!(!held.update_available());
    }

    #[test]
    fn version_lists_and_selections_are_bounded() {
        let entry = EngineVersionEntry {
            version: "0.157.1".to_owned(),
            installed: true,
            active: true,
            below_floor: false,
        };
        assert!(EngineVersionList::new("codex".to_owned(), vec![entry.clone()]).is_ok());
        assert_eq!(
            EngineVersionList::new("codex".to_owned(), vec![entry; ENGINE_VERSION_LIST_MAX + 1]),
            Err(EngineInstallError::TooMany)
        );
        assert!(EngineVersionSelection::Latest.validate().is_ok());
        assert_eq!(
            EngineVersionSelection::Version(String::new()).validate(),
            Err(EngineInstallError::Version)
        );
    }
}
