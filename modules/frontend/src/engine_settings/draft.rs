//! The manual settings form's raw document and its visible lifecycle.
//!
//! The form keeps every field as text (the domain's
//! [`ManualEngineConfiguration`](artisan_domain::ManualEngineConfiguration));
//! the Forge builds and validates the configuration it describes. Nothing
//! here builds or validates an engine configuration.

#[allow(clippy::wildcard_imports)]
use super::*;

pub use artisan_domain::{
    MANUAL_CONFIGURATION_KEYS, MAX_MANUAL_CONFIGURATION_BYTES, MAX_MANUAL_CONFIGURATION_LINES,
};

/// Raw text of every field of the manual settings form. Every field starts
/// empty; no default is ever synthesized.
pub type EngineSettingsDraft = artisan_domain::ManualEngineConfiguration;

/// Returns the stable empty-value clipboard document used by the settings
/// surface: one exact `key=` line per field, never copying a value.
#[must_use]
pub fn manual_configuration_template() -> String {
    EngineSettingsDraft::template()
}

/// Parses one complete manual settings document without retaining the
/// source text.
///
/// # Errors
///
/// Returns a bounded [`EngineConfigError`] for an oversized document, a
/// malformed/unknown/duplicate line, or a missing field.
pub fn parse_manual_configuration(
    document: &str,
) -> Result<EngineSettingsDraft, EngineConfigError> {
    EngineSettingsDraft::parse(document)
}

/// Operation whose redacted failure is currently visible in the settings UI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EngineSettingsFailureOperation {
    /// The certified profile catalogue could not be admitted or read.
    Registry,
    /// The selected thread's authoritative settings could not be read.
    SettingsRead,
    /// The selected thread's durable save failed.
    Save,
    /// A clipboard document or local draft value was rejected.
    Input,
}

/// Visible lifecycle for the engine-settings section.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EngineSettingsStatus {
    /// No thread selected.
    Unselected,
    /// Awaiting authoritative settings and possibly registry.
    Loading,
    /// Registry is absent on this host.
    RegistryMissing,
    /// Registry is present but contains no certified profile.
    RegistryPresentEmpty,
    /// Authoritatively unconfigured.
    Unconfigured,
    /// Authoritatively configured and no local edits.
    Ready,
    /// Local draft differs from authoritative.
    Dirty,
    /// Save in flight.
    Saving,
    /// Conflict detected; one authoritative reload is required.
    ConflictRefreshing,
    /// Redacted failure.
    Failure,
}

/// Registry view exposed to the UI.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RegistryView {
    /// Not yet loaded.
    Loading,
    /// No registry file exists.
    Missing,
    /// Registry exists but is empty.
    PresentEmpty,
    /// Registry exists with ordered profile ids.
    Present(Vec<EngineProfileId>),
}
