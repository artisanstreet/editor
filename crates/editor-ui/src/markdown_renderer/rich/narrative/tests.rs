//! Parse and Markdown tests for the structure and narrative kinds.

use super::super::{Prepared, RichFence, prepare};

fn ready(kind: &str, json: &str) -> Prepared {
    match prepare(Some(&format!("artisan-{kind}")), json, true) {
        Some(RichFence::Ready(prepared)) => prepared,
        other => panic!("expected artisan-{kind} to parse, got {other:?}"),
    }
}

fn markdown(kind: &str, json: &str) -> String {
    ready(kind, json).markdown().to_string()
}

fn note(kind: &str, json: &str) -> String {
    match prepare(Some(&format!("artisan-{kind}")), json, true) {
        Some(RichFence::Invalid { note }) => note.to_string(),
        other => panic!("expected artisan-{kind} to be invalid, got {other:?}"),
    }
}

#[test]
fn compare_is_a_table_marking_the_recommendation() {
    let md = markdown(
        "compare",
        r#"{"options": ["Redis", "Postgres"], "recommended": "postgres",
            "rows": [{"criterion": "Ops", "values": ["new service"]}, {"criterion": "Speed", "values": ["fast", "ok", "extra"]}]}"#,
    );
    assert!(md.contains("|  | Redis | Postgres (recommended) |"), "{md}");
    assert!(md.contains("| **Ops** | new service |  |"), "{md}");
    assert!(md.contains("| **Speed** | fast | ok |"), "{md}");
    assert!(!md.contains("extra"), "{md}");
    assert!(note("compare", r#"{"options": ["a"]}"#).contains("rows"));
}
