//! Byte-bounded logs; the writer library owns rotation and retention.
use file_rotate::{ContentLimit, FileRotate, compression::Compression, suffix::AppendCount};
use std::{fs::OpenOptions, io, path::Path};

const FILE_BYTES: usize = 8 * 1024 * 1024;
const ARCHIVES: usize = 6;

pub(crate) fn file() -> Option<FileRotate<AppendCount>> {
    let directory = editor_core::hosts::EditorFiles::default_location()
        .dir
        .join("logs");
    open(&directory, FILE_BYTES).ok()
}

fn open(directory: &Path, bytes: usize) -> io::Result<FileRotate<AppendCount>> {
    std::fs::create_dir_all(directory)?;
    let path = directory.join("editor.log");
    // Fail back to stderr if this location cannot be opened; FileRotate's constructor
    // does not return I/O errors. No file content or rotation is managed here.
    drop(OpenOptions::new().create(true).append(true).open(&path)?);
    Ok(FileRotate::new(
        path,
        AppendCount::new(ARCHIVES),
        ContentLimit::Bytes(bytes),
        Compression::None,
        None,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
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
        assert_eq!(files.len(), ARCHIVES + 1);
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

    #[test]
    fn unavailable_log_directory_returns_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a-file");
        std::fs::write(&path, "occupied").unwrap();
        assert!(open(&path, FILE_BYTES).is_err());
    }
}
