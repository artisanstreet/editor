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
fn inline_image_payloads_are_traced_as_their_size() {
    let dir = tempfile::tempdir().unwrap();
    let started = Trace::start(dir.path(), "codex-t-1", Secrets::default()).unwrap();
    let payload = "iVBORw0KGgo+/=".repeat(64 * 1024);
    let line = format!(
        r#"{{"content":[{{"type":"image","url":"data:image/webp;base64,{payload}"}},{{"type":"text","text":"look"}}]}}"#
    );
    started.trace.record(Kind::Out, &line);
    finish(started);
    let records = records(dir.path());
    let out = of_kind(&records, "out");
    assert_eq!(out.len(), 1, "{records:?}");
    assert_eq!(
        out[0]["text"],
        format!(
            r#"{{"content":[{{"type":"image","url":"data:image/webp;base64,<{} bytes>"}},{{"type":"text","text":"look"}}]}}"#,
            payload.len()
        )
    );
    assert!(out[0].get("truncated_from").is_none(), "{:?}", out[0]);
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
    let started = Trace::start(dir.path(), "codex-t-3", Secrets::of(&launch, &[])).unwrap();
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

fn password_launch() -> forge_engines::Launch {
    let mut launch = forge_engines::Launch::default();
    launch.env.push((
        "OPENCODE_SERVER_PASSWORD".to_owned(),
        "server-pässword-42".to_owned(),
    ));
    launch
}

#[expect(clippy::unwrap_used, reason = "a record without text fails the test")]
fn traced_stderr(dir: &Path) -> String {
    of_kind(&records(dir), "stderr")
        .iter()
        .map(|record| record["text"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn a_credential_split_across_stderr_reads_is_redacted_at_every_split() {
    let line = "error: login with server-pässword-42 failed\n".as_bytes();
    for split in 0..=line.len() {
        let dir = tempfile::tempdir().unwrap();
        let started = Trace::start(
            dir.path(),
            "opencode-t-4",
            Secrets::of(&password_launch(), &[]),
        )
        .unwrap();
        let mut stderr = started.trace.stream(Kind::Stderr);
        stderr.write(&line[..split]);
        stderr.write(&line[split..]);
        drop(stderr);
        finish(started);
        assert_eq!(
            traced_stderr(dir.path()),
            "error: login with [redacted] failed\n",
            "split after byte {split}"
        );
    }
}

#[test]
fn credentials_in_long_unbroken_stderr_are_redacted_across_reads() {
    let dir = tempfile::tempdir().unwrap();
    let started = Trace::start(
        dir.path(),
        "opencode-t-5",
        Secrets::of(&password_launch(), &[]),
    )
    .unwrap();
    let filler = "é".repeat(487);
    let written = format!("{filler}server-pässword-42").repeat(64);
    let mut stderr = started.trace.stream(Kind::Stderr);
    for read in written.as_bytes().chunks(4093) {
        stderr.write(read);
    }
    drop(stderr);
    finish(started);
    let traced = traced_stderr(dir.path());
    assert!(
        !traced.contains("pässword"),
        "a split credential was traced"
    );
    assert_eq!(traced, format!("{filler}[redacted]").repeat(64));
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

#[test]
fn a_short_credential_is_redacted_wherever_it_stands_alone_and_switches_are_kept() {
    let dir = tempfile::tempdir().unwrap();
    let mut launch = forge_engines::Launch::default();
    launch.mcp_servers.push(forge_engines::McpServer {
        name: "db".to_owned(),
        command: "db-server".to_owned(),
        env: [("DB_PASSWORD".to_owned(), "abc123".to_owned())].into(),
        ..forge_engines::McpServer::default()
    });
    let environment = [("FEATURE_AUTH_ENABLED".to_owned(), "1".to_owned())];
    let started = Trace::start(
        dir.path(),
        "claude-short",
        Secrets::of(&launch, &environment),
    )
    .unwrap();
    started.trace.record(
        Kind::Stderr,
        "login abc123 failed; {\"password\":\"abc123\"}; abc1234 and xabc123 are other words; step 1 of 2",
    );
    finish(started);
    assert_eq!(
        of_kind(&records(dir.path()), "stderr")[0]["text"],
        "login [redacted] failed; {\"password\":\"[redacted]\"}; abc1234 and xabc123 are other words; step 1 of 2"
    );
}

#[expect(clippy::unwrap_used, reason = "an unreadable folder fails the test")]
fn trace_named(dir: &Path, prefix: &str) -> String {
    let mut named: Vec<String> = remaining(dir)
        .into_iter()
        .filter(|name| name.starts_with(prefix))
        .collect();
    assert_eq!(named.len(), 1, "one trace starts with {prefix}: {named:?}");
    named.pop().unwrap()
}

#[expect(clippy::unwrap_used, reason = "an unreadable folder fails the test")]
fn bytes_on_disk(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().metadata().unwrap().len())
        .sum()
}

#[test]
fn a_trace_still_being_written_is_never_pruned_however_old_or_large() {
    let dir = tempfile::tempdir().unwrap();
    let now = SystemTime::now();
    let first = Trace::start(dir.path(), "claude-a", Secrets::default()).unwrap();
    let live = trace_named(dir.path(), "claude-a-");
    std::fs::File::options()
        .write(true)
        .open(dir.path().join(&live))
        .unwrap()
        .set_modified(now - 8 * DAY)
        .unwrap();
    trace_file(dir.path(), "codex-b-1.jsonl", 64 * MIB, now - 3 * HOUR);
    trace_file(dir.path(), "codex-c-1.jsonl", 64 * MIB, now - 2 * HOUR);
    trace_file(dir.path(), "codex-d-1.jsonl", 64 * MIB, now - HOUR);

    let second = Trace::start(dir.path(), "claude-e", Secrets::default()).unwrap();
    first.trace.record(Kind::Note, "still running");
    finish(first);
    finish(second);

    let left = remaining(dir.path());
    assert!(left.contains(&live), "the live trace was deleted: {left:?}");
    assert!(
        !left.contains("codex-b-1.jsonl"),
        "the oldest finished trace makes room instead: {left:?}"
    );
    let kept = std::fs::read_to_string(dir.path().join(&live)).unwrap();
    assert!(kept.contains("still running"), "{kept}");
}

#[test]
fn traces_written_at_once_stay_within_256_mib_together() {
    let dir = tempfile::tempdir().unwrap();
    let now = SystemTime::now();
    for (name, age) in [("a", 4), ("b", 3), ("c", 2), ("d", 1)] {
        trace_file(
            dir.path(),
            &format!("codex-{name}-1.jsonl"),
            47 * MIB,
            now - age * HOUR,
        );
    }
    let first = Trace::start(dir.path(), "claude-x", Secrets::default()).unwrap();
    let second = Trace::start(dir.path(), "claude-y", Secrets::default()).unwrap();
    let chunk = "\u{1}".repeat(256 * 1024);

    for _ in 0..24 {
        first.trace.record(Kind::Out, &chunk);
        second.trace.record(Kind::Out, &chunk);
    }
    finish(first);
    finish(second);

    let total = bytes_on_disk(dir.path());
    assert!(
        total <= 256 * MIB,
        "the traces take {} MiB together",
        total.div_ceil(MIB)
    );
    assert_eq!(
        remaining(dir.path())
            .iter()
            .filter(|name| name.starts_with("codex-"))
            .count(),
        4,
        "the finished traces fitted beside the new ones when they started"
    );
    let dropped: u64 = ["claude-x-", "claude-y-"]
        .into_iter()
        .flat_map(|prefix| {
            std::fs::read_to_string(dir.path().join(trace_named(dir.path(), prefix)))
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str::<Value>(line).unwrap())
                .filter_map(|record| match record["kind"].as_str() {
                    Some("end") => record["dropped"].as_u64(),
                    Some("dropped") => record["records"].as_u64(),
                    _ => None,
                })
                .collect::<Vec<_>>()
        })
        .sum();
    assert!(
        dropped > 0,
        "the records past the shared budget are counted"
    );
}

#[test]
fn a_credential_that_reads_like_a_switch_is_still_redacted() {
    let dir = tempfile::tempdir().unwrap();
    let environment = [
        ("DB_PASSWORD".to_owned(), "yes".to_owned()),
        ("API_TOKEN".to_owned(), "ON".to_owned()),
        ("TOKENIZERS_PARALLELISM".to_owned(), "false".to_owned()),
        ("FEATURE_AUTH_ENABLED".to_owned(), "1".to_owned()),
    ];
    let started = Trace::start(
        dir.path(),
        "claude-switch",
        Secrets::of(&forge_engines::Launch::default(), &environment),
    )
    .unwrap();
    started.trace.record(
        Kind::Stderr,
        "password yes, token ON; {\"stream\":false,\"step\":1}",
    );
    finish(started);
    assert_eq!(
        of_kind(&records(dir.path()), "stderr")[0]["text"],
        "password [redacted], token [redacted]; {\"stream\":false,\"step\":1}"
    );
}
