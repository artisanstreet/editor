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
        head.contains(&format!("{}:{PANIC_LINE}:5", file!())),
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

const PRUNE_TEST: &str = "a_crash_keeps_only_the_newest_reports_of_its_app";

#[test]
fn a_crash_keeps_only_the_newest_reports_of_its_app() {
    if let Some(data) = std::env::var_os(CRASH_DATA) {
        let recent = diagnostics::subscribe("info", std::io::sink).unwrap();
        crash(Path::new(&data), recent);
    }
    let directory = tempfile::tempdir().unwrap();
    let folder = CrashReporter::folder(directory.path());
    std::fs::create_dir_all(&folder).unwrap();
    let hour = std::time::Duration::from_secs(3600);
    let old = |name: &str, hours: u32| {
        let path = folder.join(name);
        std::fs::write(&path, "an old report").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(std::time::UNIX_EPOCH + hour * hours)
            .unwrap();
    };
    for report in 0..CrashReporter::KEPT_REPORTS {
        old(
            &format!("forge-{report}-1.txt"),
            u32::try_from(report).unwrap() + 1,
        );
    }
    old("editor-0-1.txt", 0);

    let output = Command::new(std::env::current_exe().unwrap())
        .args([PRUNE_TEST, "--exact", "--nocapture", "--test-threads=1"])
        .env(CRASH_DATA, directory.path())
        .output()
        .unwrap();
    assert!(!output.status.success(), "the child must crash");

    let mut names: Vec<String> = std::fs::read_dir(&folder)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names.len(), CrashReporter::KEPT_REPORTS + 1, "{names:?}");
    assert!(
        !names.contains(&"forge-0-1.txt".to_owned()),
        "the oldest Forge report went: {names:?}"
    );
    assert!(names.contains(&"forge-1-1.txt".to_owned()), "{names:?}");
    assert!(
        names.contains(&"editor-0-1.txt".to_owned()),
        "another app's report stays: {names:?}"
    );
    assert_eq!(
        names
            .iter()
            .filter(|name| std::fs::read_to_string(folder.join(name))
                .unwrap()
                .contains("crashed"))
            .count(),
        1,
        "the new report is kept: {names:?}"
    );
}
