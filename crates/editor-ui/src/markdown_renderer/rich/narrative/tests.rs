//! Parse tests for the structure and narrative kinds.

use super::super::{Prepared, RichFence, prepare};

fn ready(kind: &str, json: &str) -> Prepared {
    match prepare(Some(&format!("artisan-{kind}")), json, true) {
        Some(RichFence::Ready(prepared)) => prepared,
        other => panic!("expected artisan-{kind} to parse, got {other:?}"),
    }
}

fn ready_example(kind: &str, json: &str) {
    let _ = ready(kind, json);
}

fn note(kind: &str, json: &str) -> String {
    match prepare(Some(&format!("artisan-{kind}")), json, true) {
        Some(RichFence::Invalid { note }) => note.to_string(),
        other => panic!("expected artisan-{kind} to be invalid, got {other:?}"),
    }
}

#[test]
fn compare_prepares_partial_rows_and_recommendation() {
    let _md = ready_example(
        "compare",
        r#"{"options": ["Redis", "Postgres"], "recommended": "postgres",
            "rows": [{"criterion": "Ops", "values": ["new service"]}, {"criterion": "Speed", "values": ["fast", "ok", "extra"]}]}"#,
    );
    assert!(note("compare", r#"{"options": ["a"]}"#).contains("rows"));
}
