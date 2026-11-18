use forge_engines::trace::{Kind, Secrets, Started, Trace, prune};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::Path;
use std::time::{Duration, SystemTime};

#[expect(clippy::unwrap_used, reason = "a writer that panicked fails the test")]
fn finish(started: Started) {
    let Started { trace, writer } = started;
    drop(trace);
    writer.join().unwrap();
}

#[expect(clippy::unwrap_used, reason = "an unreadable trace fails the test")]
fn records(dir: &Path) -> Vec<Value> {
    let entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(entries.len(), 1, "one trace file");
    std::fs::read_to_string(entries[0].path())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn of_kind<'a>(records: &'a [Value], kind: &str) -> Vec<&'a Value> {
    records
        .iter()
        .filter(|record| record["kind"] == kind)
        .collect()
}

#[test]
fn an_oversized_record_is_kept_truncated_and_says_how_long_it_was() {
    let dir = tempfile::tempdir().unwrap();
    let started = Trace::start(dir.path(), "claude-t-1", Secrets::default()).unwrap();
    let long = "é".repeat(200 * 1024);
    started.trace.record(Kind::Out, &long);
    finish(started);
    let records = records(dir.path());
    let out = of_kind(&records, "out");
    assert_eq!(out.len(), 1, "{records:?}");
    assert_eq!(out[0]["truncated_from"], long.len());
    let kept = out[0]["text"].as_str().unwrap();
    assert!(
        long.starts_with(kept) && kept.len() < long.len(),
        "a prefix of the text is kept"
    );
    let end = of_kind(&records, "end");
    assert_eq!(end.len(), 1, "the trace says it is complete");
    assert_eq!(end[0]["dropped"], 0);
}

#[test]
fn every_record_past_the_budget_is_counted_as_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let started = Trace::start(dir.path(), "codex-t-2", Secrets::default()).unwrap();
    let chunk = "x".repeat(200 * 1024);
    let sent = 120;
    for _ in 0..sent {
        started.trace.record(Kind::Out, &chunk);
    }
    started.trace.record(Kind::Note, "exited");
    finish(started);
    let records = records(dir.path());
    let written = of_kind(&records, "out").len() + of_kind(&records, "note").len();
    let marked: u64 = of_kind(&records, "dropped")
        .iter()
        .map(|record| record["records"].as_u64().unwrap())
        .sum();
    let end = of_kind(&records, "end");
    assert_eq!(end.len(), 1, "the trace says it is complete");
    let at_end = end[0]["dropped"].as_u64().unwrap();
    assert!(marked + at_end > 0, "the budget cannot hold every record");
    assert_eq!(
        written as u64 + marked + at_end,
        sent + 1,
        "every record is either written or counted as dropped"
    );
}

#[test]
fn launch_credentials_never_reach_the_trace() {
    let dir = tempfile::tempdir().unwrap();
    let mut launch = forge_engines::Launch::default();
    forge_engines::with_gateway_mcp(
        artisan_api::EngineId::Codex,
        &mut launch,
        "artisan",
        "https://127.0.0.1:4100/mcp/artisan/gateway-token-0123456789",
    );
    launch.mcp_servers.push(forge_engines::McpServer {
        name: "docs".to_owned(),
        url: Some("https://docs.example/mcp".to_owned()),
        headers: [(
            "Authorization".to_owned(),
            "Bearer \"header-secret\"".to_owned(),
        )]
        .into(),
        ..forge_engines::McpServer::default()
    });
    launch.env.push((
        "OPENCODE_SERVER_PASSWORD".to_owned(),
        "server-password-42".to_owned(),
    ));
    let started = Trace::start(dir.path(), "codex-t-3", Secrets::of(&launch)).unwrap();
    let frame = serde_json::json!({
        "method": "thread/start",
        "params": {
            "config": {
                "mcp_servers": {
                    "artisan": { "url": "https://127.0.0.1:4100/mcp/artisan/gateway-token-0123456789" },
                    "docs": { "http_headers": { "Authorization": "Bearer \"header-secret\"" } },
                },
            },
            "text": "the token was gateway-token-0123456789 and the password server-password-42",
        },
    });
    started.trace.record(Kind::In, &frame.to_string());
    finish(started);
    let records = records(dir.path());
    let traced = of_kind(&records, "in")[0]["text"]
        .as_str()
        .unwrap()
        .to_owned();
    for secret in [
        "gateway-token-0123456789",
        "header-secret",
        "server-password-42",
    ] {
        assert!(
            !traced.contains(secret),
            "the trace kept {secret}: {traced}"
        );
    }
    assert!(
        traced.contains("thread/start") && traced.contains("[redacted]"),
        "the frame is kept with its credentials redacted: {traced}"
    );
}

const MIB: u64 = 1024 * 1024;
const HOUR: Duration = Duration::from_hours(1);
const DAY: Duration = Duration::from_hours(24);

#[expect(
    clippy::unwrap_used,
    reason = "a fixture that cannot be written fails the test"
)]
fn trace_file(dir: &Path, name: &str, bytes: u64, modified: SystemTime) {
    let file = std::fs::File::create(dir.join(name)).unwrap();
    file.set_len(bytes).unwrap();
    file.set_modified(modified).unwrap();
}

#[expect(clippy::unwrap_used, reason = "an unreadable folder fails the test")]
fn remaining(dir: &Path) -> BTreeSet<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

#[test]
fn traces_older_than_a_week_are_deleted_and_other_files_are_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let now = SystemTime::now();
    trace_file(dir.path(), "claude-a-1.jsonl", 1024, now - 8 * DAY);
    trace_file(dir.path(), "codex-b-2.jsonl", 1024, now - 6 * DAY);
    trace_file(dir.path(), "notes.txt", 1024, now - 30 * DAY);
    assert_eq!(prune(dir.path(), now).unwrap(), 1);
    assert_eq!(
        remaining(dir.path()),
        BTreeSet::from(["codex-b-2.jsonl".to_owned(), "notes.txt".to_owned()])
    );
}

#[test]
fn only_the_newest_traces_that_fit_in_256_mib_are_kept() {
    let dir = tempfile::tempdir().unwrap();
    let now = SystemTime::now();
    trace_file(dir.path(), "claude-c-3.jsonl", 100 * MIB, now - HOUR);
    trace_file(dir.path(), "claude-b-2.jsonl", 100 * MIB, now - 2 * HOUR);
    trace_file(dir.path(), "claude-a-1.jsonl", 100 * MIB, now - 3 * HOUR);
    trace_file(dir.path(), "codex-z-0.jsonl", 1024, now - 4 * HOUR);
    assert_eq!(prune(dir.path(), now).unwrap(), 2);
    assert_eq!(
        remaining(dir.path()),
        BTreeSet::from(["claude-b-2.jsonl".to_owned(), "claude-c-3.jsonl".to_owned()]),
        "once the budget is spent every older trace goes, however small"
    );
}

#[test]
fn starting_a_trace_makes_room_for_it_and_drops_stale_ones() {
    let dir = tempfile::tempdir().unwrap();
    let now = SystemTime::now();
    trace_file(dir.path(), "claude-b-2.jsonl", 100 * MIB, now - HOUR);
    trace_file(dir.path(), "claude-a-1.jsonl", 100 * MIB, now - 2 * HOUR);
    trace_file(dir.path(), "codex-z-0.jsonl", 1024, now - 8 * DAY);
    let started = Trace::start(dir.path(), "claude-c-3", Secrets::default()).unwrap();
    finish(started);
    let left = remaining(dir.path());
    assert_eq!(left.len(), 2, "{left:?}");
    assert!(left.contains("claude-b-2.jsonl"), "{left:?}");
    assert!(
        left.iter().any(|name| name.starts_with("claude-c-3-")),
        "{left:?}"
    );
}
