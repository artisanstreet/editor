use diagnostics::{Limit, log_file::open};
use std::io::Write as _;

#[test]
fn oversized_writes_and_reopening_keep_size_and_retention_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("forge.log");
    let limit = Limit {
        bytes: 64,
        archives: 6,
    };
    let mut writer = open(&path, limit).unwrap();
    writer.write_all(&vec![b'x'; limit.bytes * 20 + 3]).unwrap();
    writer.flush().unwrap();
    drop(writer);
    let mut writer = open(&path, limit).unwrap();
    writer.write_all(&vec![b'y'; limit.bytes * 2]).unwrap();
    writer.flush().unwrap();
    let files: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(files.len(), 7);
    assert!(
        files
            .iter()
            .all(|file| file.metadata().unwrap().len() <= limit.bytes as u64)
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"yyy");
}
