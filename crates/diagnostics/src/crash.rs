use crate::RecentLines;
use crate::frames::Frames;
use artisan_paths::App;
use std::backtrace::Backtrace;
use std::cell::{Cell, RefCell};
use std::panic::{PanicHookInfo, UnwindSafe};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing_error::SpanTrace;

thread_local! {
    static CATCHING: Cell<usize> = const { Cell::new(0) };
    static CAUGHT: RefCell<Option<CaughtPanic>> = const { RefCell::new(None) };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanicStrategy {
    Abort,
    Unwind,
}

impl PanicStrategy {
    const BUILT: Self = if cfg!(panic = "abort") {
        Self::Abort
    } else {
        Self::Unwind
    };

    #[must_use]
    pub const fn catches(self, catching: usize) -> bool {
        matches!(self, Self::Unwind) && catching > 0
    }
}

#[derive(Debug, thiserror::Error)]
#[error("panicked at {location}: {message}")]
pub struct CaughtPanic {
    pub message: String,
    pub location: String,
}

pub fn catch<T>(run: impl FnOnce() -> T + UnwindSafe) -> Result<T, CaughtPanic> {
    CATCHING.with(|depth| depth.set(depth.get() + 1));
    let result = std::panic::catch_unwind(run);
    CATCHING.with(|depth| depth.set(depth.get() - 1));
    result.map_err(|payload| {
        CAUGHT.take().unwrap_or_else(|| CaughtPanic {
            message: payload
                .downcast_ref::<&str>()
                .map(|message| (*message).to_owned())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "a non-text panic payload".to_owned()),
            location: "unknown".to_owned(),
        })
    })
}

fn location(info: &PanicHookInfo<'_>) -> String {
    info.location().map_or_else(
        || "unknown".to_owned(),
        |location| {
            format!(
                "{}:{}:{}",
                location.file(),
                location.line(),
                location.column()
            )
        },
    )
}

#[derive(Debug)]
pub struct CrashReporter {
    pub app: App,
    pub version: &'static str,
    pub data: PathBuf,
    pub recent: RecentLines,
}

#[derive(Debug, thiserror::Error)]
pub enum PruneReportsError {
    #[error("list the crash reports in {folder}")]
    List {
        folder: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("remove the old crash report {path}")]
    Remove {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
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
    frames: Frames,
    spans: SpanTrace,
    log: Vec<String>,
}

impl Report<'_> {
    fn render(&self) -> String {
        let log = self.log.join("\n");
        format!(
            "{} {} crashed\nthread: {}\nmessage: {}\nlocation: {}\n\nspan trace:\n{}\n\nbacktrace:\n{}\n\n{}\n{}\nrecent log ({} lines):\n{log}\n",
            self.app.display_name(),
            self.version,
            self.thread,
            self.message,
            self.location,
            self.spans,
            self.backtrace,
            crate::frames::HEADING,
            self.frames,
            self.log.len(),
        )
    }
}

impl CrashReporter {
    pub const KEPT_REPORTS: usize = 20;

    pub fn folder(data: &Path) -> PathBuf {
        data.join("crashes")
    }

    #[expect(
        clippy::print_stderr,
        reason = "a crashing process may have no working logger, so the report path goes to stderr"
    )]
    pub fn install(self) {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if PanicStrategy::BUILT.catches(CATCHING.with(Cell::get)) {
                CAUGHT.set(Some(CaughtPanic {
                    message: info
                        .payload_as_str()
                        .unwrap_or("non-string panic payload")
                        .to_owned(),
                    location: location(info),
                }));
                return;
            }
            match self.write(info) {
                Ok(path) => {
                    eprintln!("crash report: {}", path.display());
                    if let Err(error) = prune(&Self::folder(&self.data), self.app) {
                        eprintln!("{}", crate::chain(&error));
                    }
                }
                Err(error) => eprintln!("{}", crate::chain(&error)),
            }
            previous(info);
            std::process::abort();
        }));
    }

    fn write(&self, info: &PanicHookInfo<'_>) -> Result<PathBuf, WriteReportError> {
        let current = std::thread::current();
        let report = Report {
            app: self.app,
            version: self.version,
            thread: current.name().unwrap_or("unnamed"),
            message: info.payload_as_str().unwrap_or("non-string panic payload"),
            location: location(info),
            backtrace: Backtrace::force_capture(),
            frames: Frames::capture(),
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

fn prune(folder: &Path, app: App) -> Result<(), PruneReportsError> {
    let listed = |source| PruneReportsError::List {
        folder: folder.to_owned(),
        source,
    };
    let prefix = format!("{}-", app.id());
    let mut reports = Vec::new();
    for entry in std::fs::read_dir(folder).map_err(listed)? {
        let entry = entry.map_err(listed)?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(&prefix) && name.ends_with(".txt") {
            let written = entry
                .metadata()
                .map_err(listed)?
                .modified()
                .map_err(listed)?;
            reports.push((written, entry.path()));
        }
    }
    reports.sort_by(|(left, _), (right, _)| right.cmp(left));
    reports
        .into_iter()
        .skip(CrashReporter::KEPT_REPORTS)
        .try_for_each(|(_, path)| {
            std::fs::remove_file(&path).map_err(|source| PruneReportsError::Remove { path, source })
        })
}

fn stamp(now: SystemTime) -> String {
    match now.duration_since(UNIX_EPOCH) {
        Ok(elapsed) => elapsed.as_millis().to_string(),
        Err(before) => format!("before-epoch-{}", before.duration().as_millis()),
    }
}
