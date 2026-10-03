use serde_json::json;

use crate::event::{Event, bounded_args};
use crate::recorder::{HISTORY_CAPACITY, History};

use super::Snapshot;

fn event(ts: u64, phase: &'static str, id: Option<u64>) -> Event {
    Event {
        ts,
        tid: 7,
        category: "navigation",
        name: "thread_switch",
        phase,
        id,
        args: json!({}),
    }
}

#[test]
fn concurrent_async_spans_keep_distinct_ids_and_unfinished_work_is_visible() {
    let mut history = History::new();
    history.push(event(10, "b", Some(1)));
    history.push(event(11, "b", Some(2)));
    history.push(event(20, "e", Some(1)));
    let export = Snapshot::new(&history, 30, 4, "manual").json("editor", 123);
    let events = export["traceEvents"].as_array().unwrap();
    let unfinished = events
        .iter()
        .find(|e| e["id"] == "2" && e["ph"] == "e")
        .unwrap();
    assert_eq!(unfinished["args"]["unfinished"], true);
    assert_eq!(unfinished["ts"], 30);
    assert_eq!(export["metadata"]["dropped_records"], 4);
    assert!(events.iter().all(|e| e["pid"] == 123));
}

#[test]
fn time_window_and_count_are_bounded_without_losing_unfinished_operations() {
    let mut history = History::new();
    history.push(event(1, "b", Some(1)));
    for i in 0..HISTORY_CAPACITY + 10 {
        history.push(event(130_000_000 + i as u64, "i", None));
    }
    assert_eq!(history.events.len(), HISTORY_CAPACITY);
    assert!(history.evicted > 0);
    let export = Snapshot::new(&history, 140_000_000, 0, "test").json("editor", 1);
    let begin = export["traceEvents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["ph"] == "b")
        .unwrap();
    assert_eq!(begin["args"]["begin_truncated"], true);
}

#[test]
fn an_end_whose_begin_was_dropped_is_not_exported_as_an_orphan() {
    let mut history = History::new();
    history.push(event(20, "e", Some(9)));
    let export = Snapshot::new(&history, 30, 1, "test").json("editor", 1);
    assert!(
        !export["traceEvents"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["ph"] == "e")
    );
}

#[test]
fn arguments_reject_nested_payloads_and_bound_unicode_strings() {
    let bounded = bounded_args(
        json!({"text":"ø".repeat(200), "payload":{"secret":"no"}, "items":[1,2], "count":2}),
    );
    assert_eq!(bounded["text"].as_str().unwrap().chars().count(), 128);
    assert!(bounded.get("payload").is_none());
    assert!(bounded.get("items").is_none());
    assert_eq!(bounded["count"], 2);
}

#[test]
fn synchronous_measurements_have_microsecond_durations() {
    let mut measured = event(10, "X", None);
    measured.args = json!({"duration_us":1234});
    assert_eq!(measured.json(1)["dur"], 1234);
}

#[test]
fn an_idle_stalled_operation_keeps_its_duration_and_thread_name() {
    let mut history = History::new();
    history.push(event(1, "b", Some(1)));
    let mut metadata = event(2, "M", None);
    metadata.name = "thread_name";
    metadata.args = json!({"name":"editor.ui"});
    history.push(metadata);
    history.events.clear();
    let export = Snapshot::new(&history, 150_000_000, 0, "test").json("editor", 1);
    let events = export["traceEvents"].as_array().unwrap();
    let begin = events.iter().find(|e| e["ph"] == "b").unwrap();
    assert_eq!(begin["ts"], 30_000_000);
    assert!(
        events
            .iter()
            .any(|e| e["name"] == "thread_name" && e["args"]["name"] == "editor.ui")
    );
}

#[test]
fn disk_retention_is_bounded_and_no_partial_file_survives_success() {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "artisan-export-test-{}-{nonce}",
        std::process::id()
    ));
    let history = History::new();
    for sequence in 0..20 {
        super::write_snapshot(
            &directory,
            "fixture",
            1,
            sequence,
            &Snapshot::new(&history, sequence as u64, 0, "test"),
        )
        .unwrap();
    }
    let files: Vec<_> = std::fs::read_dir(&directory)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(files.len(), 16);
    assert!(files.iter().all(|file| file.extension().unwrap() == "json"));
    for file in files {
        let _: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(file).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
    std::fs::remove_dir_all(directory).unwrap();
}
