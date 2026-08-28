//! Focused, dependency-free coverage for notification preference resolution.
//!
//! The production module is included directly so this harness does not need
//! registration in the VP-owned frontend library or build files.

#[path = "../../modules/frontend/src/notification_preferences.rs"]
mod notification_preferences;

use notification_preferences::{
    DecodedNotificationPreferences, NOTIFICATION_PREFERENCES_STORAGE_KEY,
    NOTIFICATION_PREFERENCES_VERSION, NotificationPreferences,
    NotificationPreferencesLoadResolution, RuntimeSurface, StoredNotificationPreferences,
    default_notification_preferences, resolve_notification_preferences,
};

#[test]
fn surface_defaults_are_version_one_and_differ_by_host() {
    let desktop = default_notification_preferences(RuntimeSurface::Desktop);
    let browser = default_notification_preferences(RuntimeSurface::Browser);

    assert_eq!(desktop, NotificationPreferences::new(true));
    assert_eq!(browser, NotificationPreferences::new(false));
    assert_eq!(desktop.version, 1);
    assert_eq!(browser.version, NOTIFICATION_PREFERENCES_VERSION);
    assert!(desktop.enabled);
    assert!(!browser.enabled);
}

#[test]
fn explicit_true_and_false_values_survive_on_both_surfaces() {
    for surface in RuntimeSurface::ALL {
        for enabled in [true, false] {
            let state = NotificationPreferences::new(enabled);
            let resolution = resolve_notification_preferences(
                surface,
                StoredNotificationPreferences::Valid(state),
            );

            assert_eq!(
                resolution,
                NotificationPreferencesLoadResolution::Valid { state }
            );
            assert_eq!(resolution.state(), state);
            assert!(!resolution.should_repair());
            assert_eq!(resolution.replacement(), None);
        }
    }

    // In particular, an explicit opt-out on desktop and opt-in in a browser
    // must not be rewritten merely because each differs from its default.
    assert!(
        !resolve_notification_preferences(
            RuntimeSurface::Desktop,
            StoredNotificationPreferences::Valid(NotificationPreferences::new(false)),
        )
        .state()
        .enabled
    );
    assert!(
        resolve_notification_preferences(
            RuntimeSurface::Browser,
            StoredNotificationPreferences::Valid(NotificationPreferences::new(true)),
        )
        .state()
        .enabled
    );
}

#[test]
fn missing_values_use_the_surface_default_without_repair() {
    for (surface, expected_enabled) in [
        (RuntimeSurface::Desktop, true),
        (RuntimeSurface::Browser, false),
    ] {
        let expected = NotificationPreferences::new(expected_enabled);
        let resolution =
            resolve_notification_preferences(surface, StoredNotificationPreferences::Missing);

        assert_eq!(
            resolution,
            NotificationPreferencesLoadResolution::Missing { state: expected }
        );
        assert_eq!(resolution.state(), expected);
        assert!(!resolution.should_repair());
        assert_eq!(resolution.replacement(), None);
    }
}

#[test]
fn malformed_values_use_the_surface_default_and_request_replacement() {
    for (surface, expected_enabled) in [
        (RuntimeSurface::Desktop, true),
        (RuntimeSurface::Browser, false),
    ] {
        let expected = NotificationPreferences::new(expected_enabled);
        let resolution =
            resolve_notification_preferences(surface, StoredNotificationPreferences::Malformed);

        assert_eq!(
            resolution,
            NotificationPreferencesLoadResolution::Malformed {
                replacement: expected
            }
        );
        assert_eq!(resolution.state(), expected);
        assert!(resolution.should_repair());
        assert_eq!(resolution.replacement(), Some(expected));
    }
}

#[test]
fn only_version_one_decodes_as_valid_and_bad_versions_repair() {
    assert_eq!(
        StoredNotificationPreferences::from_decoded(DecodedNotificationPreferences::new(
            NOTIFICATION_PREFERENCES_VERSION,
            true,
        )),
        StoredNotificationPreferences::Valid(NotificationPreferences::new(true))
    );

    for version in [0, 2, u8::MAX] {
        assert_eq!(
            StoredNotificationPreferences::from_decoded(DecodedNotificationPreferences::new(
                version, true,
            )),
            StoredNotificationPreferences::Malformed,
            "version {version} must not enter the valid state"
        );
    }

    // The resolver remains defensive if a caller constructs the public enum
    // directly instead of using the decoded-outcome constructor.
    let manually_unsupported = StoredNotificationPreferences::Valid(NotificationPreferences {
        version: 2,
        enabled: true,
    });
    let resolution =
        resolve_notification_preferences(RuntimeSurface::Browser, manually_unsupported);
    assert_eq!(
        resolution,
        NotificationPreferencesLoadResolution::Malformed {
            replacement: NotificationPreferences::new(false)
        }
    );
    assert!(resolution.should_repair());
}

#[test]
fn storage_key_and_surface_vocabulary_are_exact() {
    assert_eq!(
        NOTIFICATION_PREFERENCES_STORAGE_KEY,
        "artisan.notifications"
    );
    assert_eq!(NOTIFICATION_PREFERENCES_VERSION, 1);
    assert_eq!(RuntimeSurface::Desktop.as_str(), "desktop");
    assert_eq!(RuntimeSurface::Browser.as_str(), "browser");
}
