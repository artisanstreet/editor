use super::*;
use serde_json::json;

#[test]
fn the_windows_are_read_in_forge_s_shape() {
    let usage = json!({
        "five_hour": {
            "utilization": 3.0,
            "resets_at": "2026-10-21T17:20:00.759594+00:00"
        },
        "seven_day": { "utilization": 12.5, "resets_at": null }
    });
    let windows = windows_of(&usage);
    assert_eq!(windows.len(), 2);
    assert_eq!(windows[0].label, "Session");
    assert_eq!(windows[0].cadence, UsageCadence::Session);
    assert!((windows[0].used_percent - 3.0).abs() < f64::EPSILON);
    assert!(windows[0].resets_at.is_some());
    assert_eq!(windows[1].label, "Week");
    assert_eq!(windows[1].cadence, UsageCadence::Week);
    assert!((windows[1].used_percent - 12.5).abs() < f64::EPSILON);
    assert!(windows[1].resets_at.is_none());
}
