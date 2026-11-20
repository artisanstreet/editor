use std::path::{Path, PathBuf};

const REAL_TIME_READS: [&str; 5] = [
    "Instant::now",
    "SystemTime::now",
    "artisan_api::now",
    "Utc::now",
    ".elapsed()",
];

const HARNESSES: [&str; 2] = ["capture", "qualification"];

fn sources(directory: &Path, found: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("{} is readable: {error}", directory.display()));
    for entry in entries {
        let path = entry
            .unwrap_or_else(|error| panic!("{} lists: {error}", directory.display()))
            .path();
        if path.is_dir() {
            sources(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
}

#[test]
fn only_the_stopwatch_reads_real_time_so_a_test_clock_controls_every_shell_timer() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let stopwatch = root.join("shell").join("stopwatch.rs");
    let mut files = Vec::new();
    sources(&root, &mut files);
    let mut reads = Vec::new();
    for file in files.iter().filter(|file| **file != stopwatch) {
        let relative = file
            .strip_prefix(&root)
            .expect("every source sits under src");
        if HARNESSES
            .iter()
            .any(|harness| relative.starts_with(harness))
        {
            continue;
        }
        let text = std::fs::read_to_string(file)
            .unwrap_or_else(|error| panic!("{} is readable: {error}", file.display()));
        for (line, content) in text.lines().enumerate() {
            for read in REAL_TIME_READS {
                if content.contains(read) {
                    reads.push(format!("src/{}:{}: {read}", relative.display(), line + 1));
                }
            }
        }
    }
    assert!(
        reads.is_empty(),
        "these read real time instead of the injected clock (core.clock()), so a test clock can't drive them:\n{}",
        reads.join("\n")
    );
}
