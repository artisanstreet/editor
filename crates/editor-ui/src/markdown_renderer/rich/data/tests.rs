//! Data kinds: production preparation and numerical styling.

use super::super::kit::Tone;
use super::super::{Prepared, RichFence, prepare};
use super::stats::Tile;

fn ready(kind: &str, json: &str) -> Prepared {
    match prepare(Some(&format!("artisan-{kind}")), json, true) {
        Some(RichFence::Ready(prepared)) => prepared,
        other => panic!("expected artisan-{kind} to parse, got {other:?}"),
    }
}

fn invalid(kind: &str, json: &str) -> String {
    match prepare(Some(&format!("artisan-{kind}")), json, true) {
        Some(RichFence::Invalid { note }) => note.to_string(),
        other => panic!("expected artisan-{kind} to be invalid, got {other:?}"),
    }
}

fn tile(json: &str) -> Tile {
    serde_json::from_str(json).expect("tile")
}

#[test]
fn stats_prepares_tiles_and_requires_values() {
    let _prepared = ready(
        "stats",
        r#"{"tiles": [
            {"label": "Requests", "value": 85000, "delta": 0.12, "sparkline": [1, 3, 2]},
            {"label": "p95", "value": "41 ms", "delta": "-8%", "good": "down", "hint": "Last 24 h"}
        ]}"#,
    );
    assert!(invalid("stats", r#"{"tiles": [{"label": "x"}]}"#).contains("value"));
    assert!(invalid("stats", r#"{"tiles": [{"value": 1}]}"#).contains("label"));
}

#[test]
fn stat_deltas_are_colored_by_whether_their_trend_is_good() {
    assert_eq!(
        tile(r#"{"label": "a", "value": 1, "delta": 5}"#).tone(),
        Tone::Success
    );
    assert_eq!(
        tile(r#"{"label": "a", "value": 1, "delta": "+5%", "good": "down"}"#).tone(),
        Tone::Danger
    );
    assert_eq!(
        tile(r#"{"label": "a", "value": 1, "delta": "−3", "good": "down"}"#).tone(),
        Tone::Success
    );
    assert_eq!(
        tile(r#"{"label": "a", "value": 1, "delta": "3", "trend": "down"}"#).tone(),
        Tone::Danger
    );
    assert_eq!(
        tile(r#"{"label": "a", "value": 1, "delta": 0}"#).tone(),
        Tone::Neutral
    );
    assert_eq!(
        tile(r#"{"label": "a", "value": 1, "trend": "flat"}"#).tone(),
        Tone::Neutral
    );
    assert_eq!(
        tile(r#"{"label": "a", "value": 1, "delta": "n/a"}"#).tone(),
        Tone::Neutral
    );
}
