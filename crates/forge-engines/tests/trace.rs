use forge_engines::trace::{Kind, Started, Trace};
use serde_json::Value;

#[expect(clippy::unwrap_used, reason = "a writer that panicked fails the test")]
fn finish(started: Started) {
    let Started { trace, writer } = started;
    drop(trace);
    writer.join().unwrap();
}

#[expect(clippy::unwrap_used, reason = "an unreadable trace fails the test")]
fn records(dir: &std::path::Path) -> Vec<Value> {
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
    let started = Trace::start(dir.path(), "claude-t-1").unwrap();
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
    let started = Trace::start(dir.path(), "codex-t-2").unwrap();
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
