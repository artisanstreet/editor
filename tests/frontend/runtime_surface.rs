//! Focused, dependency-free coverage for the runtime-surface projection.

#[path = "../../modules/frontend/src/runtime_surface.rs"]
mod runtime_surface;

use runtime_surface::{
    EditorSessionStartedTelemetryIntent, RuntimeSurface, editor_session_started_telemetry,
    is_mac_desktop, runtime_surface_for, time_to_ready_ms,
};

#[test]
fn electron_detection_is_case_sensitive_and_matches_the_exact_substring() {
    let cases = [
        ("", RuntimeSurface::Browser),
        ("Mozilla/5.0 Electron/36.0", RuntimeSurface::Desktop),
        ("Electron/", RuntimeSurface::Desktop),
        ("prefix-Electron/-suffix", RuntimeSurface::Desktop),
        ("electron/36.0", RuntimeSurface::Browser),
        ("ELECTRON/36.0", RuntimeSurface::Browser),
        ("Electron", RuntimeSurface::Browser),
        ("ElectronX/36.0", RuntimeSurface::Browser),
        ("Electron /36.0", RuntimeSurface::Browser),
    ];

    for (user_agent, expected) in cases {
        assert_eq!(
            runtime_surface_for(user_agent),
            expected,
            "unexpected surface for user agent {user_agent:?}"
        );
    }
}

#[test]
fn mac_desktop_requires_both_desktop_marker_and_macintosh() {
    let cases = [
        ("Mozilla/5.0 Macintosh; Intel Mac OS X", false),
        ("Mozilla/5.0 Electron/36.0 Windows NT 10.0", false),
        ("Mozilla/5.0 Electron/36.0 Macintosh; Intel Mac OS X", true),
        ("Electron/36.0 Macintosh", true),
        ("electron/36.0 Macintosh", false),
        ("Electron/36.0 macintosh", false),
        ("Electron Macintosh", false),
    ];

    for (user_agent, expected) in cases {
        assert_eq!(
            is_mac_desktop(user_agent),
            expected,
            "unexpected Mac-desktop result for user agent {user_agent:?}"
        );
    }
}

#[test]
fn runtime_surface_labels_are_exact() {
    assert_eq!(RuntimeSurface::Desktop.as_str(), "desktop");
    assert_eq!(RuntimeSurface::Desktop.forge_connection(), "local");
    assert_eq!(
        RuntimeSurface::Desktop.telemetry_surface(),
        "desktop_renderer"
    );
    assert_eq!(RuntimeSurface::Browser.as_str(), "browser");
    assert_eq!(RuntimeSurface::Browser.forge_connection(), "remote");
    assert_eq!(
        RuntimeSurface::Browser.telemetry_surface(),
        "browser_renderer"
    );
}

#[test]
fn bootstrap_projection_has_the_exact_desktop_payload() {
    let event = editor_session_started_telemetry(
        "Mozilla/5.0 Electron/36.0 Macintosh; Intel Mac OS X",
        1_000,
        1_250,
    );

    assert_eq!(
        event,
        EditorSessionStartedTelemetryIntent {
            event: "editor_session_started",
            forge_connection: "local",
            surface: "desktop_renderer",
            time_to_ready_ms: 250,
        }
    );
}

#[test]
fn bootstrap_projection_has_the_exact_browser_payload() {
    let event = editor_session_started_telemetry(
        "Mozilla/5.0 Macintosh; Intel Mac OS X AppleWebKit/605.1.15",
        1_000,
        1_500,
    );

    assert_eq!(event.event, "editor_session_started");
    assert_eq!(event.forge_connection, "remote");
    assert_eq!(event.surface, "browser_renderer");
    assert_eq!(event.time_to_ready_ms, 500);
}

#[test]
fn ready_time_is_clamped_at_both_ends() {
    let cases = [
        (1_000, 999, 0),
        (1_000, 1_000, 0),
        (1_000, 1_250, 250),
        (1_000, 601_000, 600_000),
        (1_000, 601_001, 600_000),
    ];

    for (started_at_ms, ready_at_ms, expected) in cases {
        assert_eq!(
            time_to_ready_ms(started_at_ms, ready_at_ms),
            expected,
            "unexpected elapsed value for started={started_at_ms}, ready={ready_at_ms}"
        );
        assert_eq!(
            editor_session_started_telemetry("Electron/36.0", started_at_ms, ready_at_ms)
                .time_to_ready_ms,
            expected
        );
    }
}

#[test]
fn ready_time_handles_extreme_signed_inputs_without_overflow() {
    assert_eq!(time_to_ready_ms(i64::MIN, i64::MAX), 600_000);
    assert_eq!(time_to_ready_ms(i64::MAX, i64::MIN), 0);
}
