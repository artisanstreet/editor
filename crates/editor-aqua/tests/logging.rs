use editor_aqua::logging::open;
use std::io::Write as _;

#[test]
fn oversized_writes_and_reopening_keep_size_and_retention_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let limit = 64;
    let mut writer = open(dir.path(), limit).unwrap();
    writer.write_all(&vec![b'x'; limit * 20 + 3]).unwrap();
    writer.flush().unwrap();
    drop(writer);
    let mut writer = open(dir.path(), limit).unwrap();
    writer.write_all(&vec![b'y'; limit * 2]).unwrap();
    writer.flush().unwrap();
    let files: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(files.len(), 7);
    assert!(
        files
            .iter()
            .all(|file| file.metadata().unwrap().len() <= limit as u64)
    );
    assert_eq!(
        std::fs::read(dir.path().join("editor.log")).unwrap(),
        b"yyy"
    );
}
