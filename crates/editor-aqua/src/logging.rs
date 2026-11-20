use diagnostics::log_file::Writer;
use diagnostics::{Limit, OpenLogError, RecentLines, SubscribeError};
use std::error::Error;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tracing_subscriber::fmt::writer::MakeWriterExt as _;

const LIMIT: Limit = Limit {
    bytes: 8 * 1024 * 1024,
    archives: 6,
};
const DEFAULT_FILTER: &str = "info,gpui=warn";

enum LogFile {
    Usual(Writer),
    Fallback {
        file: Writer,
        path: PathBuf,
        usual: Option<OpenLogError>,
    },
    Missing {
        usual: Option<OpenLogError>,
        fallback: OpenLogError,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum RecordError {
    #[error("open the temporary log")]
    Open(#[source] OpenLogError),
    #[error("write the temporary log {}", path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

fn path(data: &Path) -> PathBuf {
    data.join("logs").join("editor.log")
}

fn fallback_path() -> PathBuf {
    path(&std::env::temp_dir().join("artisan-editor"))
}

fn open(data: Option<&Path>) -> LogFile {
    let usual = match data.map(|data| diagnostics::log_file::open(&path(data), LIMIT)) {
        Some(Ok(file)) => return LogFile::Usual(file),
        Some(Err(usual)) => Some(usual),
        None => None,
    };
    let fallback = fallback_path();
    match diagnostics::log_file::open(&fallback, LIMIT) {
        Ok(file) => LogFile::Fallback {
            file,
            path: fallback,
            usual,
        },
        Err(fallback) => LogFile::Missing { usual, fallback },
    }
}

pub(crate) fn init(data: Option<&Path>) -> Result<RecentLines, SubscribeError> {
    match open(data) {
        LogFile::Usual(file) => {
            diagnostics::subscribe(DEFAULT_FILTER, std::io::stderr.and(Mutex::new(file)))
        }
        LogFile::Fallback { file, path, usual } => {
            let recent =
                diagnostics::subscribe(DEFAULT_FILTER, std::io::stderr.and(Mutex::new(file)))?;
            let error = usual.as_ref().map(|usual| -> &dyn Error { usual });
            tracing::error!(
                error,
                fallback = %path.display(),
                "the Editor logs to a temporary folder"
            );
            Ok(recent)
        }
        LogFile::Missing { usual, fallback } => {
            let recent = diagnostics::subscribe(DEFAULT_FILTER, std::io::stderr)?;
            let error = usual.as_ref().map(|usual| -> &dyn Error { usual });
            let fallback_error: &dyn Error = &fallback;
            tracing::error!(error, fallback_error, "the Editor has no log file");
            Ok(recent)
        }
    }
}

pub(crate) fn record_in_fallback(failure: &SubscribeError) -> Result<(), RecordError> {
    let path = fallback_path();
    let mut file = diagnostics::log_file::open(&path, LIMIT).map_err(RecordError::Open)?;
    writeln!(
        file,
        "ERROR the Editor could not launch error=start logging: {}",
        diagnostics::chain(failure)
    )
    .map_err(|source| RecordError::Write { path, source })
}
