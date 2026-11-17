use diagnostics::log_file::Writer;
use diagnostics::{Limit, OpenLogError, RecentLines, SubscribeError};
use std::error::Error;
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
        usual: OpenLogError,
    },
    Missing {
        usual: OpenLogError,
        fallback: OpenLogError,
    },
}

fn path(data: &Path) -> PathBuf {
    data.join("logs").join("editor.log")
}

fn open(data: &Path) -> LogFile {
    match diagnostics::log_file::open(&path(data), LIMIT) {
        Ok(file) => LogFile::Usual(file),
        Err(usual) => {
            let fallback = path(&std::env::temp_dir().join("artisan-editor"));
            match diagnostics::log_file::open(&fallback, LIMIT) {
                Ok(file) => LogFile::Fallback {
                    file,
                    path: fallback,
                    usual,
                },
                Err(fallback) => LogFile::Missing { usual, fallback },
            }
        }
    }
}

pub fn init(data: &Path) -> Result<RecentLines, SubscribeError> {
    match open(data) {
        LogFile::Usual(file) => {
            diagnostics::subscribe(DEFAULT_FILTER, std::io::stderr.and(Mutex::new(file)))
        }
        LogFile::Fallback { file, path, usual } => {
            let recent =
                diagnostics::subscribe(DEFAULT_FILTER, std::io::stderr.and(Mutex::new(file)))?;
            tracing::error!(
                error = &usual as &dyn Error,
                fallback = %path.display(),
                "the Editor logs to a temporary folder"
            );
            Ok(recent)
        }
        LogFile::Missing { usual, fallback } => {
            let recent = diagnostics::subscribe(DEFAULT_FILTER, std::io::stderr)?;
            tracing::error!(
                error = &usual as &dyn Error,
                fallback_error = &fallback as &dyn Error,
                "the Editor has no log file"
            );
            Ok(recent)
        }
    }
}
