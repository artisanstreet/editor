use diagnostics::{Limit, log_file::open};
use std::io::Write as _;
use std::path::Path;

fn lines(dir: &Path) -> Vec<String> {
    let mut lines: Vec<String> = std::fs::read_dir(dir)
        .expect("list the log folder")
        .map(|entry| entry.expect("read a log folder entry").path())
        .filter(|path| path.extension().is_none_or(|extension| extension != "lock"))
        .flat_map(|path| {
            std::fs::read_to_string(path)
                .expect("read a log file")
                .lines()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .collect();
    lines.sort();
    lines
}

fn logs(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .expect("list the log folder")
        .filter(|entry| {
            entry
                .as_ref()
                .expect("read a log folder entry")
                .path()
                .extension()
                .is_none_or(|extension| extension != "lock")
        })
        .count()
}

#[test]
fn two_processes_rotating_one_log_keep_every_line() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("forge.log");
    let limit = Limit {
        bytes: 64,
        archives: 100,
    };
    let mut gateway = open(&path, limit).unwrap();
    let mut backend = open(&path, limit).unwrap();
    let mut written = Vec::new();
    for turn in 0..60 {
        for (name, writer) in [("gateway", &mut gateway), ("backend", &mut backend)] {
            let line = format!("{name} {turn:02}");
            writer.write_all(format!("{line}\n").as_bytes()).unwrap();
            written.push(line);
        }
    }
    written.sort();
    assert_eq!(lines(dir.path()), written);
    assert!(logs(dir.path()) > 2, "the log rotated");
}

#[test]
fn rotation_keeps_the_newest_archives_and_drops_older_ones() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("forge.log");
    let limit = Limit {
        bytes: 32,
        archives: 2,
    };
    let mut writer = open(&path, limit).unwrap();
    for line in 0..40 {
        writer
            .write_all(format!("line {line:02} of the log\n").as_bytes())
            .unwrap();
    }
    drop(writer);
    let mut writer = open(&path, limit).unwrap();
    writer.write_all(b"after reopening\n").unwrap();

    assert_eq!(logs(dir.path()), 3);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "after reopening\n");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("forge.log.1")).unwrap(),
        "line 38 of the log\nline 39 of the log\n"
    );
    assert!(
        !lines(dir.path()).contains(&"line 00 of the log".to_owned()),
        "the oldest lines left with the oldest archive"
    );
}
