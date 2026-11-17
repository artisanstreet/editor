use file_rotate::{ContentLimit, FileRotate, compression::Compression, suffix::AppendCount};
use std::{fs::OpenOptions, io, path::Path};

const FILE_BYTES: usize = 8 * 1024 * 1024;
const ARCHIVES: usize = 6;
const DEFAULT_FILTER: &str = "info,gpui=warn";

pub fn file() -> Option<FileRotate<AppendCount>> {
    let directory = crate::core::EditorFiles::default_location()
        .dir
        .join("logs");
    open(&directory, FILE_BYTES).ok()
}

pub fn open(directory: &Path, bytes: usize) -> io::Result<FileRotate<AppendCount>> {
    std::fs::create_dir_all(directory)?;
    let path = directory.join("editor.log");
    drop(OpenOptions::new().create(true).append(true).open(&path)?);
    Ok(FileRotate::new(
        path,
        AppendCount::new(ARCHIVES),
        ContentLimit::Bytes(bytes),
        Compression::None,
        None,
    ))
}

pub fn init() -> Result<diagnostics::RecentLines, diagnostics::SubscribeError> {
    use tracing_subscriber::fmt::writer::MakeWriterExt as _;
    match file().map(std::sync::Mutex::new) {
        Some(file) => diagnostics::subscribe(DEFAULT_FILTER, std::io::stderr.and(file)),
        None => diagnostics::subscribe(DEFAULT_FILTER, std::io::stderr),
    }
}
