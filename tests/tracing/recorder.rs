use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::json;

use super::*;

#[test]
fn saturation_drops_records_instead_of_waiting_for_the_consumer() {
    let (events, _receiver) = mpsc::sync_channel(1);
    let (controls, _) = mpsc::sync_channel(1);
    let recorder = Recorder {
        origin: Instant::now(),
        epoch_us: 0,
        events,
        controls,
        dropped: AtomicU64::new(0),
        running: AtomicBool::new(true),
    };
    for _ in 0..100 {
        recorder.record(Event {
            ts: 0,
            tid: 1,
            category: "test",
            name: "saturation",
            phase: "i",
            id: None,
            args: json!({}),
        });
    }
    assert_eq!(recorder.dropped.load(Ordering::Relaxed), 99);
}

#[test]
fn automatic_stall_manual_capture_and_shutdown_produce_readable_files() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "artisan-tracing-test-{}-{nonce}",
        std::process::id()
    ));
    let session = start_at("fixture", directory.clone()).expect("session");
    name_thread("fixture.ui");
    let queued = span("transport.queue", "fixture.queue", json!({}));
    std::thread::spawn(move || {
        name_thread("fixture.transport");
        queued.handoff();
        drop(queued);
        let _execution = span("transport.command", "fixture.command", json!({}));
        let began = Instant::now();
        std::thread::sleep(Duration::from_millis(10));
        completed("ui.task", "fixture.poll", began, Instant::now(), json!({}));
        counter("transport", "fixture.depth", json!({"queued":1}));
    })
    .join()
    .unwrap();
    let operation = span(
        "navigation",
        "fixture.wait",
        json!({"request_id":"fixture-id"}),
    );
    let heartbeat = crate::Heartbeat::new("fixture.ui");
    heartbeat.tick();
    std::thread::sleep(Duration::from_millis(750));
    heartbeat.tick();
    // Let the automatic capture keep its post-incident tail.
    std::thread::sleep(Duration::from_millis(2300));
    assert!(save());
    std::thread::sleep(Duration::from_millis(200));
    drop(heartbeat);
    drop(operation);
    drop(session);
    let captures: Vec<Value> = std::fs::read_dir(&directory)
        .unwrap()
        .map(|entry| {
            serde_json::from_slice(&std::fs::read(entry.unwrap().path()).unwrap()).unwrap()
        })
        .collect();
    assert!(
        captures
            .iter()
            .any(|c| c["metadata"]["reason"] == "heartbeat.stalled")
    );
    assert!(captures.iter().any(|c| c["metadata"]["reason"] == "manual"));
    assert!(
        captures
            .iter()
            .any(|c| c["metadata"]["reason"] == "shutdown")
    );
    assert!(captures.iter().any(|c| {
        c["traceEvents"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["name"] == "heartbeat.stalled")
    }));
    assert!(captures.iter().any(|c| {
        c["traceEvents"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["name"] == "fixture.wait" && e["args"]["unfinished"] == true)
    }));
    let events = captures
        .iter()
        .flat_map(|c| c["traceEvents"].as_array().unwrap());
    let events: Vec<_> = events.collect();
    assert!(
        events
            .iter()
            .any(|e| e["flow_out"] == true && e["ph"] == "X" && e["dur"] == 1)
    );
    assert!(
        events
            .iter()
            .any(|e| e["flow_in"] == true && e["bind_id"].is_string())
    );
    assert!(
        events
            .iter()
            .any(|e| e["name"] == "fixture.poll" && e["dur"].as_u64().unwrap_or(0) >= 10_000)
    );
    assert!(
        events
            .iter()
            .any(|e| e["name"] == "fixture.depth" && e["ph"] == "C")
    );
    if std::env::var_os("ARTISAN_TRACE_KEEP_FIXTURE").is_some() {
        println!("Fixture traces: {}", directory.display());
    } else {
        std::fs::remove_dir_all(directory).unwrap();
    }
}

#[test]
fn idle_history_expires_and_stalls_only_report_once_per_operation() {
    let mut history = History::new();
    history.push(Event {
        ts: 1,
        tid: 1,
        category: "navigation",
        name: "pending",
        phase: "b",
        id: Some(1),
        args: json!({}),
    });
    assert_eq!(history.slow_operation(6_000_000), Some("operation.stalled"));
    assert_eq!(history.slow_operation(7_000_000), None);
    history.expire(130_000_000);
    assert!(history.events.is_empty());
    assert_eq!(history.open.len(), 1);
}
