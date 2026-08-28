//! Pure notification-preference state and storage-resolution policy.
//!
//! This is the native counterpart of
//! `modules/frontend/src/lib/notifications/preferences.ts`. The storage
//! adapter supplies an already-decoded outcome and carries out the returned
//! replacement, if any. This module deliberately does not know about Effect,
//! browser permission APIs, serialization, or key-value-store I/O.

#![allow(clippy::module_name_repetitions)]

/// The only notification-preference schema version currently understood.
pub const NOTIFICATION_PREFERENCES_VERSION: u8 = 1;

/// The durable key shared by the browser and desktop storage adapters.
pub const NOTIFICATION_PREFERENCES_STORAGE_KEY: &str = "artisan.notifications";

/// The host surface whose notification default is being resolved.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RuntimeSurface {
    /// The installed desktop editor shell.
    Desktop,
    /// A browser tab connected to the editor.
    Browser,
}

impl RuntimeSurface {
    /// Every supported surface, in the canonical desktop-then-browser order.
    pub const ALL: [Self; 2] = [Self::Desktop, Self::Browser];

    /// Returns the canonical surface label used by the host boundary.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::Browser => "browser",
        }
    }

    /// Whether an unset preference is enabled on this surface.
    #[must_use]
    pub const fn default_notifications_enabled(self) -> bool {
        matches!(self, Self::Desktop)
    }
}

/// Version-1 notification preferences.
///
/// `version` remains public because it is part of the value an adapter
/// persists. Values normally come from [`Self::new`], a surface default, or
/// [`StoredNotificationPreferences::from_decoded`]. The resolver also checks
/// the field defensively so a manually restored value with an unsupported
/// version cannot bypass repair.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct NotificationPreferences {
    /// The persisted schema version. Supported values are exactly `1`.
    pub version: u8,
    /// Whether the reader has enabled host notifications.
    pub enabled: bool,
}

impl NotificationPreferences {
    /// The schema version carried by every native preference value.
    pub const VERSION: u8 = NOTIFICATION_PREFERENCES_VERSION;

    /// Creates a version-1 value with the reader's explicit choice.
    #[must_use]
    pub const fn new(enabled: bool) -> Self {
        Self {
            version: Self::VERSION,
            enabled,
        }
    }

    /// Creates the unset default for a browser or desktop surface.
    #[must_use]
    pub const fn default_for(surface: RuntimeSurface) -> Self {
        Self::new(surface.default_notifications_enabled())
    }

    /// Whether this value belongs to the supported version-1 schema.
    #[must_use]
    pub const fn has_supported_version(self) -> bool {
        self.version == Self::VERSION
    }
}

/// Returns the unset notification state for a host surface.
#[must_use]
pub const fn default_notification_preferences(surface: RuntimeSurface) -> NotificationPreferences {
    NotificationPreferences::default_for(surface)
}

/// The already-decoded fields supplied by a storage adapter.
///
/// A decoder that cannot produce these fields (for example, because the
/// stored value has the wrong shape or a non-boolean `enabled`) should pass
/// [`StoredNotificationPreferences::Malformed`] instead. Version validation
/// is kept here because the version is representable without a serializer.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct DecodedNotificationPreferences {
    /// The decoded persisted schema version.
    pub version: u8,
    /// The decoded explicit notification choice.
    pub enabled: bool,
}

impl DecodedNotificationPreferences {
    /// Creates decoded fields before version classification.
    #[must_use]
    pub const fn new(version: u8, enabled: bool) -> Self {
        Self { version, enabled }
    }
}

/// What the storage adapter found under [`NOTIFICATION_PREFERENCES_STORAGE_KEY`].
///
/// This is an explicit value-level replacement for an effectful store result:
/// missing is not an error, valid carries the decoded choice, and malformed
/// covers decode failures as well as unsupported versions.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum StoredNotificationPreferences {
    /// No value exists for the storage key.
    Missing,
    /// A decoded value whose version is expected to be supported.
    Valid(NotificationPreferences),
    /// The stored value could not be used, or its version is unsupported.
    Malformed,
}

impl StoredNotificationPreferences {
    /// Classifies decoded fields without performing serialization or I/O.
    #[must_use]
    pub const fn from_decoded(value: DecodedNotificationPreferences) -> Self {
        if value.version == NOTIFICATION_PREFERENCES_VERSION {
            Self::Valid(NotificationPreferences {
                version: value.version,
                enabled: value.enabled,
            })
        } else {
            Self::Malformed
        }
    }
}

/// The pure result of resolving one stored preference outcome for a surface.
///
/// The `Malformed` variant carries the value the adapter should write back to
/// the exact storage key. Missing and valid values never request a write.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum NotificationPreferencesLoadResolution {
    /// No preference was stored; use the surface default without repair.
    Missing {
        /// The surface default to expose to the caller.
        state: NotificationPreferences,
    },
    /// A version-1 preference was stored; preserve its explicit choice.
    Valid {
        /// The decoded value to expose to the caller.
        state: NotificationPreferences,
    },
    /// The stored value was malformed; replace it with the surface default.
    Malformed {
        /// The value the adapter should persist as repair.
        replacement: NotificationPreferences,
    },
}

impl NotificationPreferencesLoadResolution {
    /// Returns the state the caller should use for this load.
    #[must_use]
    pub const fn state(self) -> NotificationPreferences {
        match self {
            Self::Missing { state } | Self::Valid { state } => state,
            Self::Malformed { replacement } => replacement,
        }
    }

    /// Whether the storage adapter should replace the malformed value.
    #[must_use]
    pub const fn should_repair(self) -> bool {
        matches!(self, Self::Malformed { .. })
    }

    /// Returns the replacement to persist, when repair is required.
    #[must_use]
    pub const fn replacement(self) -> Option<NotificationPreferences> {
        match self {
            Self::Malformed { replacement } => Some(replacement),
            Self::Missing { .. } | Self::Valid { .. } => None,
        }
    }
}

/// Resolves one decoded storage outcome without performing storage I/O.
///
/// Missing values use the surface default and are left absent. Valid version-1
/// values preserve `enabled`, including an explicit choice that differs from
/// the current surface default. Malformed values use the surface default and
/// ask the adapter to replace the stored value with that default.
#[must_use]
pub const fn resolve_notification_preferences(
    surface: RuntimeSurface,
    stored: StoredNotificationPreferences,
) -> NotificationPreferencesLoadResolution {
    let default = default_notification_preferences(surface);

    match stored {
        StoredNotificationPreferences::Missing => {
            NotificationPreferencesLoadResolution::Missing { state: default }
        }
        StoredNotificationPreferences::Valid(state) if state.has_supported_version() => {
            NotificationPreferencesLoadResolution::Valid { state }
        }
        StoredNotificationPreferences::Valid(_) | StoredNotificationPreferences::Malformed => {
            NotificationPreferencesLoadResolution::Malformed {
                replacement: default,
            }
        }
    }
}
