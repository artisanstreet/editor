use crate::RecentLines;
use artisan_paths::App;
use std::backtrace::Backtrace;
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing_error::SpanTrace;

pub struct CrashReporter {
    pub app: App,
    pub version: &'static str,
    pub data: PathBuf,
    pub recent: RecentLines,
}

#[derive(Debug, thiserror::Error)]
pub enum WriteReportError {
    #[error("create the crash report folder {path}")]
    CreateFolder {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("write the crash report {path}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

struct Report<'a> {
    app: App,
    version: &'a str,
    thread: &'a str,
    message: &'a str,
    location: String,
    backtrace: Backtrace,
    spans: SpanTrace,
    log: Vec<String>,
}

impl Report<'_> {
    fn render(&self) -> String {
        let log = self.log.join("\n");
        format!(
            "{} {} crashed\nthread: {}\nmessage: {}\nlocation: {}\n\nspan trace:\n{}\n\nbacktrace:\n{}\n\nrecent log ({} lines):\n{log}\n",
            self.app.display_name(),
            self.version,
            self.thread,
            self.message,
            self.location,
            self.spans,
            self.backtrace,
            self.log.len(),
        )
    }
}

impl CrashReporter {
    pub fn folder(data: &Path) -> PathBuf {
        data.join("crashes")
    }

    pub fn install(self) {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            match self.write(info) {
                Ok(path) => eprintln!("crash report: {}", path.display()),
                Err(error) => eprintln!("{}", crate::chain(&error)),
            }
            previous(info);
        }));
    }

    fn write(&self, info: &PanicHookInfo<'_>) -> Result<PathBuf, WriteReportError> {
        let current = std::thread::current();
        let report = Report {
            app: self.app,
            version: self.version,
            thread: current.name().unwrap_or("unnamed"),
            message: info.payload_as_str().unwrap_or("non-string panic payload"),
            location: info.location().map_or_else(
                || "unknown".to_owned(),
                |location| {
                    format!(
                        "{}:{}:{}",
                        location.file(),
                        location.line(),
                        location.column()
                    )
                },
            ),
            backtrace: Backtrace::force_capture(),
            spans: SpanTrace::capture(),
            log: self.recent.snapshot(),
        };
        let folder = Self::folder(&self.data);
        std::fs::create_dir_all(&folder).map_err(|source| WriteReportError::CreateFolder {
            path: folder.clone(),
            source,
        })?;
        let path = folder.join(format!(
            "{}-{}-{}.txt",
            self.app.id(),
            stamp(SystemTime::now()),
            std::process::id()
        ));
        std::fs::write(&path, report.render()).map_err(|source| WriteReportError::Write {
            path: path.clone(),
            source,
        })?;
        Ok(path)
    }
}

fn stamp(now: SystemTime) -> String {
    match now.duration_since(UNIX_EPOCH) {
        Ok(elapsed) => elapsed.as_millis().to_string(),
        Err(before) => format!("before-epoch-{}", before.duration().as_millis()),
    }
}
