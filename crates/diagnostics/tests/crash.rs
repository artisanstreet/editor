use diagnostics::CrashReporter;
use std::path::Path;
use std::process::Command;

const CRASH_DATA: &str = "DIAGNOSTICS_CRASH_DATA";
const TEST: &str = "a_panic_writes_a_report_with_message_location_frames_spans_and_log";
const PANIC_LINE: u32 = line!() + 4;

#[expect(clippy::panic, reason = "the child process crashes on purpose")]
fn abandon_turn() -> ! {
    panic!("invariant: run r-7 of thread t-42 lost its turn")
}

fn crash(data: &Path, recent: diagnostics::RecentLines) -> ! {
    CrashReporter {
        app: artisan_paths::App::Forge,
        version: "9.9.9 (test)",
        data: data.to_owned(),
        recent,
    }
    .install();
    let span = tracing::info_span!("turn", thread = "t-42", run = "r-7");
    let _entered = span.enter();
    tracing::info!(host = "h-3", "turn started");
    abandon_turn()
}

#[test]
fn a_panic_writes_a_report_with_message_location_frames_spans_and_log() {
    if let Some(data) = std::env::var_os(CRASH_DATA) {
        let recent = diagnostics::subscribe("info", std::io::sink).unwrap();
        crash(Path::new(&data), recent);
    }
    let directory = tempfile::tempdir().unwrap();
    let output = Command::new(std::env::current_exe().unwrap())
        .args([TEST, "--exact", "--nocapture", "--test-threads=1"])
        .env(CRASH_DATA, directory.path())
        .env_remove("RUST_BACKTRACE")
        .output()
        .unwrap();
    assert!(!output.status.success(), "the child must crash");
    let reports: Vec<_> = std::fs::read_dir(CrashReporter::folder(directory.path()))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(reports.len(), 1, "one crash, one report: {reports:?}");
    let report = std::fs::read_to_string(&reports[0]).unwrap();
    let (head, rest) = report.split_once("span trace:").unwrap();
    let (spans, rest) = rest.split_once("backtrace:").unwrap();
    let (frames, log) = rest.split_once("recent log").unwrap();
    assert!(head.starts_with("Forge 9.9.9 (test) crashed"), "{head}");
    assert!(
        head.contains("message: invariant: run r-7 of thread t-42 lost its turn"),
        "{head}"
    );
    assert!(
        head.contains(&format!("tests/crash.rs:{PANIC_LINE}:5")),
        "{head}"
    );
    assert!(spans.contains("turn"), "{spans}");
    assert!(spans.contains("thread=\"t-42\""), "{spans}");
    assert!(spans.contains("run=\"r-7\""), "{spans}");
    assert!(frames.contains("crash::abandon_turn"), "{frames}");
    assert!(frames.contains("a_panic_writes_a_report"), "{frames}");
    assert!(log.contains("turn started"), "{log}");
    assert!(log.contains("host=\"h-3\""), "{log}");
}
