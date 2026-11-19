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

#[test]
fn a_rotation_that_cannot_happen_keeps_logging_reports_once_and_rotates_once_it_can() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("forge.log");
    let blocker = dir.path().join("forge.log.1");
    std::fs::create_dir(&blocker).unwrap();
    std::fs::write(blocker.join("in the way"), "").unwrap();
    let limit = Limit {
        bytes: 32,
        archives: 1,
    };
    let mut writer = open(&path, limit).unwrap();
    let written: Vec<String> = (0..40)
        .map(|line| format!("line {line:02} of the log"))
        .collect();
    for line in &written {
        writer.write_all(format!("{line}\n").as_bytes()).unwrap();
    }

    let text = std::fs::read_to_string(&path).unwrap();
    let logged: Vec<&str> = text
        .lines()
        .filter(|line| line.starts_with("line "))
        .collect();
    assert_eq!(logged, written, "every line still lands in the log");
    let reports: Vec<&str> = text
        .lines()
        .filter(|line| line.contains("rotating the log failed"))
        .collect();
    assert_eq!(reports.len(), 1, "{text}");
    assert!(
        reports[0].contains(&blocker.display().to_string()),
        "the report names what is in the way: {}",
        reports[0]
    );

    std::fs::remove_dir_all(&blocker).unwrap();
    writer.write_all(b"after the way cleared\n").unwrap();

    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "after the way cleared\n",
        "the log rotates again once it can"
    );
    assert_eq!(std::fs::read_to_string(&blocker).unwrap(), text);
}

#[test]
fn a_rotation_after_a_half_finished_one_fills_the_gap_instead_of_dropping_an_archive() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("forge.log");
    std::fs::write(&path, "the full log\n").unwrap();
    std::fs::write(dir.path().join("forge.log.2"), "older\n").unwrap();
    std::fs::write(dir.path().join("forge.log.3"), "oldest\n").unwrap();
    let limit = Limit {
        bytes: 8,
        archives: 3,
    };

    open(&path, limit).unwrap().write_all(b"new\n").unwrap();

    let archived = |number: usize| {
        std::fs::read_to_string(dir.path().join(format!("forge.log.{number}"))).unwrap()
    };
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "new\n");
    assert_eq!(archived(1), "the full log\n");
    assert_eq!(archived(2), "older\n");
    assert_eq!(archived(3), "oldest\n");
}
