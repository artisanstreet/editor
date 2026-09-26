//! The Settings view of one Forge-managed engine install: the Forge-reported
//! status, the user's version selection, and the vendor version list.
//!
//! Everything shown here is Forge data; the page renders it and emits
//! selections, never deciding install state itself.

use std::fmt::Write as _;

use artisan_domain::{
    EngineInstallPhase, EngineInstallStatus, EngineIntegrity, EngineVersionEntry,
};

/// The vendor version list as the page knows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SettingsEngineVersions {
    /// Not requested yet.
    #[default]
    NotLoaded,
    /// A listing request is in flight.
    Loading,
    /// The Forge's listing, newest first.
    Loaded(Vec<EngineVersionEntry>),
    /// The listing failed; the copy says why.
    Failed(String),
}

/// One engine's managed install, projected for the Settings page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsEngineInstall {
    /// Display label of the engine.
    pub label: String,
    /// The Forge-reported status.
    pub status: EngineInstallStatus,
    /// The vendor version list.
    pub versions: SettingsEngineVersions,
    /// The last version request the Forge refused, when one did.
    pub request_failure: Option<String>,
}

impl SettingsEngineInstall {
    /// Returns the installation copy for the Forge-reported status.
    #[must_use]
    pub fn status_copy(&self) -> String {
        let status = &self.status;
        let label = &self.label;
        let mut copy = match status.phase {
            EngineInstallPhase::Ready => match &status.active_version {
                Some(version) => {
                    format!("{label} {version} is installed and managed by this Forge.")
                }
                None => format!("{label} is installed and managed by this Forge."),
            },
            EngineInstallPhase::Installing => {
                let target = status
                    .held_version
                    .as_ref()
                    .or(status.latest_version.as_ref())
                    .map_or_else(String::new, |version| format!(" {version}"));
                match status.progress_percent {
                    Some(percent) => format!("Installing {label}{target}… {percent}%"),
                    None => format!("Installing {label}{target}…"),
                }
            }
            EngineInstallPhase::NotInstalled => {
                format!("{label} is not installed yet. The Forge installs it in the background.")
            }
            EngineInstallPhase::Failed | EngineInstallPhase::Unsupported => status
                .reason
                .clone()
                .unwrap_or_else(|| format!("{label} could not be installed.")),
        };
        if let Some(pending) = &status.pending_version {
            let _ = write!(
                copy,
                " {pending} is ready and activates when no run is using {label}."
            );
        }
        if status.overridden {
            copy.push_str(" A developer override replaces the managed binary on this Forge.");
        }
        copy
    }

    /// Returns the version-selection copy: following `latest` or held.
    #[must_use]
    pub fn selection_copy(&self) -> String {
        match (&self.status.held_version, &self.status.latest_version) {
            (Some(held), _) => {
                format!(
                    "Held at {held}. Automatic updates are off until you return to the latest release."
                )
            }
            (None, Some(latest)) if self.status.update_available() => {
                format!("Follows the latest release; {latest} is being installed.")
            }
            (None, Some(latest)) => format!("Follows the latest release ({latest})."),
            (None, None) => "Follows the latest release.".to_owned(),
        }
    }

    /// Returns how this engine's downloads are verified, stating the weaker
    /// guarantee plainly for engines without vendor checksums.
    #[must_use]
    pub fn integrity_copy(&self) -> String {
        match self.status.integrity {
            EngineIntegrity::VendorChecksum => {
                "Verified by the vendor's published checksum.".to_owned()
            }
            EngineIntegrity::TrustOnFirstDownload => {
                let recorded = self
                    .status
                    .trusted_since
                    .as_deref()
                    .and_then(|since| since.get(..10))
                    .map_or_else(String::new, |date| format!(" (hash recorded {date})"));
                let listing = if self.status.vendor_version_list {
                    ""
                } else {
                    " The vendor publishes no version list, so Artisan offers the latest release and versions this Forge downloaded before."
                };
                format!(
                    "Trusted on first download{recorded}: the vendor publishes no checksum, so Artisan records the hash of the first HTTPS download of each version and rejects any later download that differs.{listing}"
                )
            }
        }
    }

    /// Whether version controls apply (the engine can be managed here).
    #[must_use]
    pub fn manageable(&self) -> bool {
        self.status.phase != EngineInstallPhase::Unsupported
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn install(phase: EngineInstallPhase) -> SettingsEngineInstall {
        SettingsEngineInstall {
            label: "Claude Code".to_owned(),
            status: EngineInstallStatus {
                engine_id: "claude".to_owned(),
                phase,
                active_version: Some("2.1.282".to_owned()),
                held_version: None,
                latest_version: Some("2.1.282".to_owned()),
                pending_version: None,
                rollback_version: None,
                progress_percent: None,
                reason: None,
                overridden: false,
                integrity: artisan_domain::EngineIntegrity::VendorChecksum,
                trusted_since: None,
                vendor_version_list: true,
            },
            versions: SettingsEngineVersions::NotLoaded,
            request_failure: None,
        }
    }

    #[test]
    fn status_copy_names_the_forge_state() {
        assert!(
            install(EngineInstallPhase::Ready)
                .status_copy()
                .contains("2.1.282 is installed")
        );
        let mut installing = install(EngineInstallPhase::Installing);
        installing.status.progress_percent = Some(40);
        installing.status.latest_version = Some("2.1.283".to_owned());
        assert_eq!(
            installing.status_copy(),
            "Installing Claude Code 2.1.283… 40%"
        );
        let mut failed = install(EngineInstallPhase::Failed);
        failed.status.reason = Some("Claude Code update failed.".to_owned());
        assert_eq!(failed.status_copy(), "Claude Code update failed.");
        let mut pending = install(EngineInstallPhase::Ready);
        pending.status.pending_version = Some("2.1.283".to_owned());
        pending.status.overridden = true;
        let copy = pending.status_copy();
        assert!(copy.contains("2.1.283 is ready and activates when no run is using"));
        assert!(copy.contains("developer override"));
        assert!(!install(EngineInstallPhase::Unsupported).manageable());
    }

    #[test]
    fn integrity_copy_states_the_trust_mode() {
        assert!(
            install(EngineInstallPhase::Ready)
                .integrity_copy()
                .starts_with("Verified by the vendor")
        );
        let mut trusted = install(EngineInstallPhase::Ready);
        trusted.status.integrity = EngineIntegrity::TrustOnFirstDownload;
        trusted.status.trusted_since = Some("2026-09-26T09:30:00.000Z".to_owned());
        trusted.status.vendor_version_list = false;
        let copy = trusted.integrity_copy();
        assert!(copy.starts_with("Trusted on first download (hash recorded 2026-09-26)"));
        assert!(copy.contains("no version list"));
    }

    #[test]
    fn selection_copy_distinguishes_latest_held_and_updates() {
        assert_eq!(
            install(EngineInstallPhase::Ready).selection_copy(),
            "Follows the latest release (2.1.282)."
        );
        let mut update = install(EngineInstallPhase::Ready);
        update.status.latest_version = Some("2.1.283".to_owned());
        assert!(
            update
                .selection_copy()
                .contains("2.1.283 is being installed")
        );
        let mut held = install(EngineInstallPhase::Ready);
        held.status.held_version = Some("2.1.282".to_owned());
        assert!(held.selection_copy().starts_with("Held at 2.1.282"));
    }
}
