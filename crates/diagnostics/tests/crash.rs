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

fn has_frame_at(frames: &str, function: &str, line: u32) -> bool {
    let lines: Vec<&str> = frames.lines().map(str::trim).collect();
    lines.windows(2).any(|pair| {
        let named = pair[0]
            .split_once(": ")
            .is_some_and(|(_, name)| name.rsplit("::").next() == Some(function));
        let located = pair[1]
            .strip_prefix("at ")
            .and_then(|location| location.split_once("crash.rs:"))
            .is_some_and(|(_, position)| {
                position.split(':').next() == Some(line.to_string().as_str())
            });
        named && located
    })
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
    let (frames, rest) = rest.split_once(diagnostics::frames::HEADING).unwrap();
    let (recorded, log) = rest.split_once("recent log").unwrap();
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
    assert!(has_frame_at(frames, "abandon_turn", PANIC_LINE), "{frames}");
    assert!(frames.contains("a_panic_writes_a_report"), "{frames}");
    let resolved = symbolized(recorded);
    assert!(
        resolved
            .iter()
            .any(|(function, line)| function.contains("abandon_turn") && *line == Some(PANIC_LINE)),
        "the recorded frames don't resolve offline to the panic: {resolved:?}\n{recorded}"
    );
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

fn field<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    line.split_whitespace()
        .find_map(|part| part.strip_prefix(name)?.strip_prefix('='))
}

#[expect(clippy::unwrap_used, reason = "a malformed report fails the test")]
fn hex(text: &str) -> u64 {
    u64::from_str_radix(text.trim_start_matches("0x"), 16).unwrap()
}

#[expect(clippy::unwrap_used, reason = "a malformed report fails the test")]
fn symbolized(recorded: &str) -> Vec<(String, Option<u32>)> {
    let this = std::env::current_exe().unwrap();
    let name = this.file_name().unwrap().to_owned();
    let offsets: Vec<u64> = recorded
        .lines()
        .filter(|line| {
            line.split_once("module=").is_some_and(|(_, module)| {
                Path::new(module.trim()).file_name() == Some(name.as_os_str())
            })
        })
        .map(|line| hex(field(line, "offset").unwrap()))
        .collect();
    assert!(
        !offsets.is_empty(),
        "no recorded frame is in {}: {recorded}",
        this.display()
    );
    resolve(&this, &offsets)
}

#[cfg(not(target_env = "msvc"))]
#[expect(
    clippy::unwrap_used,
    reason = "an executable without readable DWARF fails the test"
)]
fn resolve(module: &Path, offsets: &[u64]) -> Vec<(String, Option<u32>)> {
    let loader = addr2line_loader::Loader::new(module).unwrap();
    let header = header_address(module);
    let mut resolved = Vec::new();
    for offset in offsets {
        let address = header + offset - 1;
        let symbol = loader.find_symbol(address).unwrap_or_default().to_owned();
        let mut frames = loader.find_frames(address).unwrap();
        while let Some(frame) = frames.next().unwrap() {
            let function = frame
                .function
                .as_ref()
                .map(|function| function.raw_name().unwrap().into_owned())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| symbol.clone());
            resolved.push((function, frame.location.and_then(|location| location.line)));
        }
    }
    resolved
}

#[cfg(not(target_env = "msvc"))]
#[expect(
    clippy::unwrap_used,
    reason = "an unreadable executable fails the test"
)]
fn header_address(module: &Path) -> u64 {
    use object::{Object as _, ObjectSegment as _};
    let bytes = std::fs::read(module).unwrap();
    let file = object::File::parse(&*bytes).unwrap();
    if file.format() == object::BinaryFormat::MachO {
        file.segments()
            .find(|segment| segment.name() == Ok(Some("__TEXT")))
            .unwrap()
            .address()
    } else {
        file.relative_address_base()
    }
}

#[cfg(target_env = "msvc")]
#[expect(
    unsafe_code,
    reason = "dbghelp reads the PDB beside the test executable"
)]
#[expect(
    clippy::unwrap_used,
    reason = "a PDB dbghelp can't load fails the test"
)]
fn resolve(module: &Path, offsets: &[u64]) -> Vec<(String, Option<u32>)> {
    use std::os::windows::ffi::OsStrExt as _;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Diagnostics::Debug::{
        IMAGEHLP_LINEW64, MAX_SYM_NAME, SYMBOL_INFOW, SYMOPT_LOAD_LINES, SYMOPT_UNDNAME,
        SymCleanup, SymFromAddrW, SymGetLineFromAddrW64, SymInitializeW, SymLoadModuleExW,
        SymSetOptions,
    };
    use windows::core::PCWSTR;
    const LOADED_AT: u64 = 0x1000_0000;
    let process = HANDLE(std::ptr::without_provenance_mut(0x5eed));
    let path: Vec<u16> = module.as_os_str().encode_wide().chain([0]).collect();
    let mut resolved = Vec::new();
    unsafe {
        SymSetOptions(SYMOPT_UNDNAME | SYMOPT_LOAD_LINES);
        SymInitializeW(process, PCWSTR::null(), false).unwrap();
        let base = SymLoadModuleExW(
            process,
            None,
            PCWSTR(path.as_ptr()),
            PCWSTR::null(),
            LOADED_AT,
            0,
            None,
            None,
        );
        assert_ne!(base, 0, "dbghelp loads {}", module.display());
        for offset in offsets {
            let address = base + offset - 1;
            let mut buffer =
                vec![0_u64; (size_of::<SYMBOL_INFOW>() + MAX_SYM_NAME as usize * 2) / 8 + 1];
            let symbol = buffer.as_mut_ptr().cast::<SYMBOL_INFOW>();
            (*symbol).SizeOfStruct = u32::try_from(size_of::<SYMBOL_INFOW>()).unwrap();
            (*symbol).MaxNameLen = MAX_SYM_NAME;
            let function = SymFromAddrW(process, address, None, symbol).map_or_else(
                |_| String::new(),
                |()| {
                    let name = std::slice::from_raw_parts(
                        (&raw const (*symbol).Name).cast::<u16>(),
                        (*symbol).NameLen as usize,
                    );
                    String::from_utf16_lossy(name)
                },
            );
            let mut line = IMAGEHLP_LINEW64 {
                SizeOfStruct: u32::try_from(size_of::<IMAGEHLP_LINEW64>()).unwrap(),
                ..IMAGEHLP_LINEW64::default()
            };
            let mut displacement = 0;
            let number =
                SymGetLineFromAddrW64(process, address, &raw mut displacement, &raw mut line)
                    .ok()
                    .map(|()| line.LineNumber);
            resolved.push((function, number));
        }
        SymCleanup(process).unwrap();
    }
    resolved
}
