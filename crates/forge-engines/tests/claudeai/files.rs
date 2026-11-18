use std::time::Duration;

use super::{ReadFileError, read_bounded};

fn read_soon(path: &std::path::Path, limit: u64) -> Result<Vec<u8>, ReadFileError> {
    let (sender, receiver) = std::sync::mpsc::channel();
    let path = path.to_path_buf();
    std::thread::spawn(move || sender.send(read_bounded(&path, limit)));
    receiver
        .recv_timeout(Duration::from_secs(10))
        .expect("the read returned instead of blocking or reading forever")
}

#[test]
fn a_regular_file_is_read_whole_up_to_the_limit() {
    let dir = tempfile::tempdir().expect("a dir");
    let file = dir.path().join("a.txt");
    std::fs::write(&file, "four").expect("the file");
    assert_eq!(read_soon(&file, 4).expect("read"), b"four");
    assert!(matches!(
        read_soon(&file, 3),
        Err(ReadFileError::TooLarge { limit: 3, .. })
    ));
    assert!(matches!(
        read_soon(dir.path(), 4),
        Err(ReadFileError::Directory { .. })
    ));
}

#[cfg(unix)]
#[test]
fn an_endless_device_is_refused_instead_of_read_forever() {
    assert!(matches!(
        read_soon(std::path::Path::new("/dev/zero"), 1024),
        Err(ReadFileError::NotRegular { .. })
    ));
}

#[cfg(unix)]
#[test]
fn a_fifo_without_a_writer_is_refused_instead_of_blocking() {
    let dir = tempfile::tempdir().expect("a dir");
    let fifo = dir.path().join("pipe");
    let made = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .expect("mkfifo");
    assert!(made.success());
    assert!(matches!(
        read_soon(&fifo, 1024),
        Err(ReadFileError::NotRegular { .. })
    ));
}
