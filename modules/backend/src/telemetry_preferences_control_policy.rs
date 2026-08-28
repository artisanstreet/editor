//! Dependency-free policy for the backend telemetry-preference control seam.
//!
//! The TypeScript control layer has two deliberately small boundaries: its
//! no-op implementation returns a fresh version-1 value for every operation,
//! while an injected port can supply a value or fail.  This module keeps both
//! boundaries deterministic.  It does not retain state, await promises,
//! execute RPC or I/O, or emit telemetry.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use std::fmt;

/// The only telemetry-preference schema version understood by this policy.
pub const TELEMETRY_PREFERENCES_VERSION: u8 = 1;

/// The closed set of choices for one telemetry category.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum TelemetryPreference {
    /// No explicit consent choice has been made.
    #[default]
    Unset,
    /// Consent was granted for this category.
    Enabled,
    /// Consent was declined for this category.
    Disabled,
}

impl TelemetryPreference {
    /// Every supported choice in the exact protocol order.
    pub const ALL: [Self; 3] = [Self::Unset, Self::Enabled, Self::Disabled];

    /// Returns the exact lowercase protocol spelling of this choice.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unset => "unset",
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
        }
    }
}

/// The two independent operations exposed by the preference control.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TelemetryPreferencesControlOperation {
    /// Read the current preferences.
    Read,
    /// Update the preferences from a patch.
    Update,
}

impl TelemetryPreferencesControlOperation {
    /// Returns the exact operation spelling carried by the TypeScript error.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Update => "update",
        }
    }
}

/// Payload-free classification of an injected control-port failure.
///
/// The source layer catches the port's rejection and exposes only its
/// operation.  The original port error is therefore intentionally not stored
/// here.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TelemetryPreferencesControlError {
    /// Operation that failed at the control boundary.
    pub operation: TelemetryPreferencesControlOperation,
}

impl TelemetryPreferencesControlError {
    /// Creates an operation-specific control error.
    #[must_use]
    pub const fn new(operation: TelemetryPreferencesControlOperation) -> Self {
        Self { operation }
    }

    /// Creates the error used when a read port operation fails.
    #[must_use]
    pub const fn read() -> Self {
        Self::new(TelemetryPreferencesControlOperation::Read)
    }

    /// Creates the error used when an update port operation fails.
    #[must_use]
    pub const fn update() -> Self {
        Self::new(TelemetryPreferencesControlOperation::Update)
    }
}

impl fmt::Display for TelemetryPreferencesControlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "telemetry preferences {} operation failed",
            self.operation.as_str()
        )
    }
}

impl std::error::Error for TelemetryPreferencesControlError {}

/// Versioned independent crash-report and usage-analytics choices.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TelemetryPreferences {
    /// Choice for crash reports.
    pub crash_reports: TelemetryPreference,
    /// Choice for product-usage analytics.
    pub usage_analytics: TelemetryPreference,
    /// Protocol schema version; values made by this policy are always `1`.
    pub version: u8,
}

impl TelemetryPreferences {
    /// The schema version carried by all values made by this policy.
    pub const VERSION: u8 = TELEMETRY_PREFERENCES_VERSION;

    /// Creates version-1 preferences with independent category choices.
    #[must_use]
    pub const fn new(
        crash_reports: TelemetryPreference,
        usage_analytics: TelemetryPreference,
    ) -> Self {
        Self {
            crash_reports,
            usage_analytics,
            version: Self::VERSION,
        }
    }

    /// Creates the exact default returned by the no-op read operation.
    #[must_use]
    pub const fn initial() -> Self {
        Self::new(TelemetryPreference::Unset, TelemetryPreference::Unset)
    }
}

impl Default for TelemetryPreferences {
    fn default() -> Self {
        Self::initial()
    }
}

/// An independently optional patch for the two telemetry categories.
///
/// `None` means that the property was omitted from the protocol patch.  The
/// backend no-op update does not merge with an earlier result: each omitted
/// property becomes [`TelemetryPreference::Unset`] in the fresh result.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct TelemetryPreferencesUpdate {
    /// Replacement for the crash-report choice, when supplied.
    pub crash_reports: Option<TelemetryPreference>,
    /// Replacement for the usage-analytics choice, when supplied.
    pub usage_analytics: Option<TelemetryPreference>,
}

impl TelemetryPreferencesUpdate {
    /// Creates a patch with independently optional category choices.
    #[must_use]
    pub const fn new(
        crash_reports: Option<TelemetryPreference>,
        usage_analytics: Option<TelemetryPreference>,
    ) -> Self {
        Self {
            crash_reports,
            usage_analytics,
        }
    }

    /// Creates a patch for crash reports only.
    #[must_use]
    pub const fn for_crash_reports(choice: TelemetryPreference) -> Self {
        Self::new(Some(choice), None)
    }

    /// Creates a patch for usage analytics only.
    #[must_use]
    pub const fn for_usage_analytics(choice: TelemetryPreference) -> Self {
        Self::new(None, Some(choice))
    }

    /// Creates a patch for both independent categories.
    #[must_use]
    pub const fn for_both(
        crash_reports: TelemetryPreference,
        usage_analytics: TelemetryPreference,
    ) -> Self {
        Self::new(Some(crash_reports), Some(usage_analytics))
    }

    /// Whether both optional properties were omitted.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.crash_reports.is_none() && self.usage_analytics.is_none()
    }

    /// Converts this patch to the fresh result produced by the no-op update.
    ///
    /// Every omitted property is independently replaced by `unset`; no prior
    /// result is accepted or retained.
    #[must_use]
    pub const fn into_preferences(self) -> TelemetryPreferences {
        TelemetryPreferences::new(
            match self.crash_reports {
                Some(choice) => choice,
                None => TelemetryPreference::Unset,
            },
            match self.usage_analytics {
                Some(choice) => choice,
                None => TelemetryPreference::Unset,
            },
        )
    }
}

/// The typed result returned by a synchronous control operation.
pub type TelemetryPreferencesControlResult =
    Result<TelemetryPreferences, TelemetryPreferencesControlError>;

/// Synchronous port shape corresponding to the source control interface.
///
/// Implementations supply already-decoded values and may use any local error
/// type.  The policy discards that error value and maps it to the operation
/// that failed, matching the source layer's `tryPromise` catches.
pub trait TelemetryPreferencesControlPort {
    /// Opaque error type supplied by the injected port.
    type Error;

    /// Reads a decoded preference result from the injected port.
    ///
    /// # Errors
    ///
    /// Returns the port's opaque error when the read cannot produce a value;
    /// the policy maps it to [`TelemetryPreferencesControlError::read`].
    fn read(&self) -> Result<TelemetryPreferences, Self::Error>;

    /// Updates the injected port with one decoded patch.
    ///
    /// # Errors
    ///
    /// Returns the port's opaque error when the update cannot produce a
    /// value; the policy maps it to
    /// [`TelemetryPreferencesControlError::update`].
    fn update(
        &self,
        patch: TelemetryPreferencesUpdate,
    ) -> Result<TelemetryPreferences, Self::Error>;
}

/// Maps a synchronous read outcome to the operation-specific control result.
///
/// The successful value is forwarded exactly.  An injected error is not
/// retained, because the TypeScript boundary exposes only `operation: "read"`.
///
/// # Errors
///
/// Returns [`TelemetryPreferencesControlError::read`] when `outcome` is an
/// error.
#[must_use = "handle the telemetry preference read result"]
pub fn resolve_read_result<E>(
    outcome: Result<TelemetryPreferences, E>,
) -> TelemetryPreferencesControlResult {
    outcome.map_err(|_| TelemetryPreferencesControlError::read())
}

/// Maps a synchronous update outcome to the operation-specific control result.
///
/// The successful value is forwarded exactly.  An injected error is not
/// retained, because the TypeScript boundary exposes only `operation: "update"`.
///
/// # Errors
///
/// Returns [`TelemetryPreferencesControlError::update`] when `outcome` is an
/// error.
#[must_use = "handle the telemetry preference update result"]
pub fn resolve_update_result<E>(
    outcome: Result<TelemetryPreferences, E>,
) -> TelemetryPreferencesControlResult {
    outcome.map_err(|_| TelemetryPreferencesControlError::update())
}

/// Stateless entry point for the backend telemetry-preference control.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TelemetryPreferencesControlPolicy;

impl TelemetryPreferencesControlPolicy {
    /// Creates the stateless control policy.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Returns the no-op read default: both categories are `unset`.
    #[must_use]
    pub const fn read() -> TelemetryPreferences {
        TelemetryPreferences::initial()
    }

    /// Applies a patch using the no-op update semantics.
    ///
    /// The result is newly constructed for this call.  Omitted fields become
    /// `unset`, and no earlier update can affect this result.
    #[must_use]
    pub const fn update(patch: TelemetryPreferencesUpdate) -> TelemetryPreferences {
        patch.into_preferences()
    }

    /// Resolves a synchronous read through an injected port.
    ///
    /// # Errors
    ///
    /// Returns [`TelemetryPreferencesControlError::read`] when the port
    /// rejects the read.
    #[must_use = "handle the telemetry preference read result"]
    pub fn read_from<P>(port: &P) -> TelemetryPreferencesControlResult
    where
        P: TelemetryPreferencesControlPort + ?Sized,
    {
        resolve_read_result(port.read())
    }

    /// Resolves a synchronous update through an injected port.
    ///
    /// # Errors
    ///
    /// Returns [`TelemetryPreferencesControlError::update`] when the port
    /// rejects the update.
    #[must_use = "handle the telemetry preference update result"]
    pub fn update_from<P>(
        port: &P,
        patch: TelemetryPreferencesUpdate,
    ) -> TelemetryPreferencesControlResult
    where
        P: TelemetryPreferencesControlPort + ?Sized,
    {
        resolve_update_result(port.update(patch))
    }
}

/// Returns the no-op read default: both categories are `unset`.
#[must_use]
pub const fn read() -> TelemetryPreferences {
    TelemetryPreferencesControlPolicy::read()
}

/// Applies a patch using the stateless no-op update semantics.
///
/// Each omitted category becomes `unset`; no previous result is consulted.
#[must_use]
pub const fn update(patch: TelemetryPreferencesUpdate) -> TelemetryPreferences {
    TelemetryPreferencesControlPolicy::update(patch)
}

/// Resolves a synchronous read through an injected port.
///
/// # Errors
///
/// Returns [`TelemetryPreferencesControlError::read`] when the port rejects
/// the read.
#[must_use = "handle the telemetry preference read result"]
pub fn read_from<P>(port: &P) -> TelemetryPreferencesControlResult
where
    P: TelemetryPreferencesControlPort + ?Sized,
{
    TelemetryPreferencesControlPolicy::read_from(port)
}

/// Resolves a synchronous update through an injected port.
///
/// # Errors
///
/// Returns [`TelemetryPreferencesControlError::update`] when the port rejects
/// the update.
#[must_use = "handle the telemetry preference update result"]
pub fn update_from<P>(
    port: &P,
    patch: TelemetryPreferencesUpdate,
) -> TelemetryPreferencesControlResult
where
    P: TelemetryPreferencesControlPort + ?Sized,
{
    TelemetryPreferencesControlPolicy::update_from(port, patch)
}
