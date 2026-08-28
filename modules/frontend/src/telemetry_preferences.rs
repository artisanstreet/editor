//! Pure telemetry-preference state and transport-fallback policy.
//!
//! This is the dependency-free native counterpart of
//! `modules/frontend/src/lib/settings/telemetry-controller.ts`. It stops at
//! deterministic values and transitions: an adapter supplies an optional
//! already-decoded remote result and performs any transport or publication
//! work around the returned value. There are no Effect layers, streams,
//! locks, or transport calls here.

#![allow(clippy::module_name_repetitions)]

/// The only telemetry-preference schema version understood by this leaf.
pub const TELEMETRY_PREFERENCES_VERSION: u8 = 1;

/// The closed set of choices for one telemetry category.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum TelemetryPreference {
    /// The reader has not made a choice yet.
    #[default]
    Unset,
    /// The reader has opted in to this category.
    Enabled,
    /// The reader has opted out of this category.
    Disabled,
}

impl TelemetryPreference {
    /// Every supported choice in the canonical wire order.
    pub const ALL: [Self; 3] = [Self::Unset, Self::Enabled, Self::Disabled];

    /// Returns the exact lowercase literal used by the TypeScript schema.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unset => "unset",
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
        }
    }
}

/// Classification of a raw telemetry-preference schema version.
///
/// Version `1` is the only version this module defines. Any other number is
/// retained as [`Self::Unsupported`] instead of being silently interpreted as
/// version 1. An adapter that decodes an untrusted remote value should make
/// its own explicit policy decision before handing that value to a state
/// transition.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TelemetryPreferencesVersion {
    /// The current version-1 schema.
    V1,
    /// A schema version this leaf does not define.
    Unsupported(u8),
}

impl TelemetryPreferencesVersion {
    /// Classifies a raw schema version without coercion.
    #[must_use]
    pub const fn from_raw(version: u8) -> Self {
        if version == TELEMETRY_PREFERENCES_VERSION {
            Self::V1
        } else {
            Self::Unsupported(version)
        }
    }

    /// Returns the raw version represented by this classification.
    #[must_use]
    pub const fn raw(self) -> u8 {
        match self {
            Self::V1 => TELEMETRY_PREFERENCES_VERSION,
            Self::Unsupported(version) => version,
        }
    }

    /// Whether this is the supported version-1 schema.
    #[must_use]
    pub const fn is_supported(self) -> bool {
        matches!(self, Self::V1)
    }
}

/// Versioned telemetry choices used by the frontend state model.
///
/// The public `version` field mirrors the protocol value and lets an adapter
/// preserve a remote result exactly. Values made by [`Self::new`] and
/// [`Self::initial`] are always version 1. Use [`Self::version_status`] when a
/// decoded value may have come from a future schema; no transition below
/// silently downgrades such a value.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TelemetryPreferences {
    /// Choice for crash reports.
    pub crash_reports: TelemetryPreference,
    /// Choice for product-usage analytics.
    pub usage_analytics: TelemetryPreference,
    /// Persisted schema version.
    pub version: u8,
}

impl TelemetryPreferences {
    /// The current schema version carried by newly-created values.
    pub const VERSION: u8 = TELEMETRY_PREFERENCES_VERSION;

    /// Creates version-1 preferences with the supplied independent choices.
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

    /// Creates the exact initial state used by the TypeScript controller.
    #[must_use]
    pub const fn initial() -> Self {
        Self::new(TelemetryPreference::Unset, TelemetryPreference::Unset)
    }

    /// Creates an already-decoded value with an explicitly supplied version.
    ///
    /// This constructor is useful at an adapter seam that must classify a
    /// future value with [`Self::version_status`]. It does not claim that the
    /// supplied version is supported.
    #[must_use]
    pub const fn with_version(
        version: u8,
        crash_reports: TelemetryPreference,
        usage_analytics: TelemetryPreference,
    ) -> Self {
        Self {
            crash_reports,
            usage_analytics,
            version,
        }
    }

    /// Classifies this value's schema version without changing the value.
    #[must_use]
    pub const fn version_status(self) -> TelemetryPreferencesVersion {
        TelemetryPreferencesVersion::from_raw(self.version)
    }

    /// Whether this value belongs to the supported version-1 schema.
    #[must_use]
    pub const fn has_supported_version(self) -> bool {
        self.version_status().is_supported()
    }

    /// Applies a local partial update while preserving omitted fields and the
    /// current schema version.
    #[must_use]
    pub const fn apply_update(self, update: TelemetryPreferencesUpdate) -> Self {
        update.apply_to(self)
    }
}

impl Default for TelemetryPreferences {
    fn default() -> Self {
        Self::initial()
    }
}

/// An independent, optionally populated update for the two choices.
///
/// `None` means that the corresponding field was omitted. Applying an update
/// therefore never resets an omitted field to `unset`. The TypeScript schema
/// validates that at least one field is present before an update reaches this
/// policy; an empty value remains representable here so a defensive adapter
/// can treat it as a deterministic no-op.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct TelemetryPreferencesUpdate {
    /// Replacement for crash-report consent, when supplied.
    pub crash_reports: Option<TelemetryPreference>,
    /// Replacement for product-usage consent, when supplied.
    pub usage_analytics: Option<TelemetryPreference>,
}

impl TelemetryPreferencesUpdate {
    /// Creates an update with independently optional fields.
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

    /// Creates an update for crash reports only.
    #[must_use]
    pub const fn for_crash_reports(choice: TelemetryPreference) -> Self {
        Self::new(Some(choice), None)
    }

    /// Creates an update for usage analytics only.
    #[must_use]
    pub const fn for_usage_analytics(choice: TelemetryPreference) -> Self {
        Self::new(None, Some(choice))
    }

    /// Creates an update for both independent choices.
    #[must_use]
    pub const fn for_both(
        crash_reports: TelemetryPreference,
        usage_analytics: TelemetryPreference,
    ) -> Self {
        Self::new(Some(crash_reports), Some(usage_analytics))
    }

    /// Whether this update omits both fields.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.crash_reports.is_none() && self.usage_analytics.is_none()
    }

    /// Applies each supplied field independently to the current state.
    #[must_use]
    pub const fn apply_to(self, current: TelemetryPreferences) -> TelemetryPreferences {
        TelemetryPreferences {
            crash_reports: match self.crash_reports {
                Some(choice) => choice,
                None => current.crash_reports,
            },
            usage_analytics: match self.usage_analytics {
                Some(choice) => choice,
                None => current.usage_analytics,
            },
            version: current.version,
        }
    }
}

/// Applies a local partial update without involving a remote operation.
#[must_use]
pub const fn apply_telemetry_preferences_update(
    current: TelemetryPreferences,
    update: TelemetryPreferencesUpdate,
) -> TelemetryPreferences {
    current.apply_update(update)
}

/// Resolves a get/refresh result from an optional remote value.
///
/// An absent get follows the controller's fallback to the exact initial state.
/// A present value is returned exactly, rather than merged with or completed
/// from local state. The input is already decoded; callers handling untrusted
/// data should inspect [`TelemetryPreferences::version_status`] first.
#[must_use]
pub const fn resolve_get_telemetry_preferences(
    remote: Option<TelemetryPreferences>,
) -> TelemetryPreferences {
    match remote {
        Some(preferences) => preferences,
        None => TelemetryPreferences::initial(),
    }
}

/// Resolves an update result from an optional remote value.
///
/// A present remote result replaces local state exactly, matching the
/// controller's `tap` of the transport response. When the update operation is
/// absent, the partial update is applied to the current state and omitted
/// fields are preserved.
#[must_use]
pub const fn resolve_update_telemetry_preferences(
    current: TelemetryPreferences,
    update: TelemetryPreferencesUpdate,
    remote: Option<TelemetryPreferences>,
) -> TelemetryPreferences {
    match remote {
        Some(preferences) => preferences,
        None => current.apply_update(update),
    }
}

/// The result type for the absent-capture fallback.
pub type CaptureTelemetryFallbackResult = Result<(), std::convert::Infallible>;

/// Models the controller's absent capture operation as a successful no-op.
///
/// A real adapter may invoke its capture transport when present; this helper
/// only represents the `client.CaptureTelemetryIntent ?? (() => Effect.void)`
/// branch and therefore cannot fail or emit an event.
#[must_use]
pub const fn capture_telemetry_intent_fallback() -> CaptureTelemetryFallbackResult {
    Ok(())
}
