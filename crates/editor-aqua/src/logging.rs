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

pub fn init() {
    use tracing_subscriber::fmt::writer::MakeWriterExt as _;
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(DEFAULT_FILTER));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false);
    let _ = match file().map(std::sync::Mutex::new) {
        Some(file) => builder.with_writer(std::io::stderr.and(file)).try_init(),
        None => builder.with_writer(std::io::stderr).try_init(),
    };
}
