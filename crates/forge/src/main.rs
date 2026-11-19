#![cfg_attr(windows, windows_subsystem = "windows")]

use artisan_paths::App;
use clap::Parser;
#[cfg(unix)]
use diagnostics::Destination;
use std::net::SocketAddr;
use std::path::PathBuf;

#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOCATOR: dhat::Alloc = dhat::Alloc;

const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("ARTISAN_COMMIT"),
    ", ",
    env!("ARTISAN_TARGET"),
    ")"
);

const LOG_LIMIT: diagnostics::Limit = diagnostics::Limit {
    bytes: 16 * 1024 * 1024,
    archives: 4,
};

#[derive(Parser)]
#[command(name = "forge", version = LONG_VERSION, about = "Artisan Forge")]
struct Cli {
    #[arg(
        long,
        value_name = "DIR",
        help = "The data folder (default: Forge's folder under Artisan Street)"
    )]
    data: Option<PathBuf>,
    #[arg(
        long,
        value_name = "ADDRESS",
        help = "Where to listen (default: `listen` in the data folder's config.json, else 127.0.0.1:47900)"
    )]
    listen: Option<SocketAddr>,
    #[arg(
        long,
        value_name = "FILE",
        help = "Append the log to this file instead of the terminal"
    )]
    log: Option<PathBuf>,
    #[arg(long, hide = true)]
    schema_version: bool,
    #[cfg(unix)]
    #[arg(long, hide = true)]
    backend: bool,
    #[cfg(unix)]
    #[arg(long, hide = true)]
    ready_file: Option<PathBuf>,
    #[cfg(unix)]
    #[arg(long, hide = true)]
    handoff: Option<PathBuf>,
}

fn main() -> std::process::ExitCode {
    let mut cli = Cli::parse();
    if cli.schema_version {
        use std::io::Write as _;
        return match writeln!(
            std::io::stdout(),
            "{}",
            forge_store::Store::schema_version()
        ) {
            Ok(()) => std::process::ExitCode::SUCCESS,
            Err(error) => {
                report(format_args!("print the schema version: {error}"));
                std::process::ExitCode::FAILURE
            }
        };
    }
    let data = cli.data.clone().map_or_else(|| App::Forge.data(), Ok);
    if cfg!(windows)
        && cli.log.is_none()
        && let Ok(data) = &data
    {
        cli.log = Some(data.join("forge.log"));
    }
    let recent = match init_tracing(cli.log.as_deref()) {
        Ok(recent) => recent,
        Err(error) => {
            report(format_args!("{}", diagnostics::chain(&error)));
            return std::process::ExitCode::FAILURE;
        }
    };
    let data_dir = match data {
        Ok(dir) => dir,
        Err(error) => {
            tracing::error!("find Forge's data folder: {error}");
            report(format_args!("find Forge's data folder: {error}"));
            return std::process::ExitCode::FAILURE;
        }
    };
    diagnostics::CrashReporter {
        app: App::Forge,
        version: LONG_VERSION,
        data: data_dir.clone(),
        recent,
    }
    .install();
    match run(&cli, data_dir) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            let chain = diagnostics::chain(&error);
            tracing::error!(error = chain, "Forge stopped");
            report(format_args!("{chain}"));
            std::process::ExitCode::FAILURE
        }
    }
}

#[expect(
    clippy::print_stderr,
    reason = "the Forge command reports why it could not run on stderr"
)]
fn report(error: std::fmt::Arguments<'_>) {
    eprintln!("error: {error}");
}

#[derive(Debug, thiserror::Error)]
enum RunError {
    #[error(transparent)]
    Listen(#[from] forge_http::ConfiguredListenError),
    #[error("start the async runtime")]
    Runtime(#[source] std::io::Error),
    #[error(transparent)]
    Start(#[from] forge_http::StartError),
    #[error(transparent)]
    Platform(#[from] PlatformError),
}

#[cfg(unix)]
#[derive(Debug, thiserror::Error)]
enum PlatformError {
    #[error("open the runtime registry in {}", data.display())]
    Registry {
        data: PathBuf,
        #[source]
        source: forge_runs::runtime::RegistryFileError,
    },
    #[error("a backend Forge needs --ready-file")]
    NoReadyFile,
    #[error("write the readiness file {}", path.display())]
    Ready {
        path: PathBuf,
        #[source]
        source: forge_runs::runtime::RegistryFileError,
    },
    #[error("find this Forge executable")]
    Executable(#[source] std::io::Error),
    #[error(transparent)]
    Gateway(#[from] forge_http::upgrade::ServeError),
}

#[cfg(not(unix))]
#[derive(Debug, thiserror::Error)]
enum PlatformError {
    #[error("remove {}", path.display())]
    Remove {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("write {}", path.display())]
    WritePid {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("watch {} for a stop request", path.display())]
    WatchStop {
        path: PathBuf,
        #[source]
        source: notify::Error,
    },
}

#[derive(Debug, thiserror::Error)]
enum TracingError {
    #[error(transparent)]
    Open(#[from] diagnostics::OpenLogError),
    #[error(transparent)]
    Subscribe(#[from] diagnostics::SubscribeError),
}

fn run(cli: &Cli, data_dir: PathBuf) -> Result<(), RunError> {
    let listen = match cli.listen {
        Some(listen) => listen,
        None => forge_http::configured_listen(&data_dir)?,
    };
    tracing::info!(version = LONG_VERSION, data = %data_dir.display(), %listen, "starting Forge");
    for outcome in forge_isolation::protect_forge() {
        match outcome {
            forge_isolation::Outcome::Applied(measure) => tracing::info!("isolation: {measure}"),
            forge_isolation::Outcome::Skipped { measure, reason } => {
                tracing::warn!("isolation unavailable: {measure}: {reason}");
            }
        }
    }
    #[cfg(feature = "dhat-heap")]
    let profiler = {
        #[cfg(unix)]
        let enabled = cli.backend;
        #[cfg(not(unix))]
        let enabled = true;
        enabled.then(|| {
            let path = data_dir.join(format!("dhat-heap-{}.json", std::process::id()));
            tracing::info!(
                "heap profile: writing {} when asked to stop",
                path.display()
            );
            dhat::Profiler::builder()
                .file_name(path)
                .trim_backtraces(Some(32))
                .build()
        })
    };
    let stop = async move {
        stop_requested().await;
        #[cfg(feature = "dhat-heap")]
        drop(profiler);
    };
    let runtime = tokio::runtime::Runtime::new().map_err(RunError::Runtime)?;
    let config = forge_http::ServeConfig { data_dir, listen };
    #[cfg(unix)]
    {
        runtime.block_on(async move {
            if cli.backend {
                let registry =
                    forge_runs::runtime::Registry::open(&config.data_dir).map_err(|source| {
                        PlatformError::Registry {
                            data: config.data_dir.clone(),
                            source,
                        }
                    })?;
                let server = forge_http::start_generation(config, registry).await?;
                let ready = cli
                    .ready_file
                    .as_deref()
                    .ok_or(PlatformError::NoReadyFile)?;
                forge_http::upgrade::write(
                    ready,
                    &server
                        .hub()
                        .generation()
                        .expect("invariant: a backend Forge runs as a generation")
                        .member(),
                )
                .map_err(|source| PlatformError::Ready {
                    path: ready.to_owned(),
                    source,
                })?;
                server.run_until(stop).await;
                Ok(())
            } else {
                let log = cli
                    .log
                    .clone()
                    .map_or(Destination::Stderr, Destination::File);
                let binary = std::env::current_exe()
                    .and_then(|binary| binary.canonicalize())
                    .map_err(PlatformError::Executable)?;
                forge_http::upgrade::serve(config, binary, cli.handoff.clone(), log, stop)
                    .await
                    .map_err(|error| PlatformError::from(error).into())
            }
        })
    }
    #[cfg(not(unix))]
    {
        runtime.block_on(serve_recorded(config, stop))
    }
}

#[cfg(not(unix))]
async fn serve_recorded(
    config: forge_http::ServeConfig,
    stop: impl Future<Output = ()>,
) -> Result<(), RunError> {
    let stop_file = config.data_dir.join("forge.stop");
    let pid_file = config.data_dir.join("forge.pid");
    let own = std::process::id();
    let forge = forge_http::start(config).await?;
    let stop_watch = StopWatch::start(&stop_file)?;
    std::fs::write(&pid_file, own.to_string()).map_err(|source| PlatformError::WritePid {
        path: pid_file.clone(),
        source,
    })?;
    forge
        .run_until(async {
            tokio::select! {
                () = stop => {},
                () = stop_watch.requested(&stop_file, own) => tracing::info!("asked to stop by {}", stop_file.display()),
            }
        })
        .await;
    remove_if_present(&pid_file)?;
    remove_if_present(&stop_file)?;
    Ok(())
}

#[cfg(not(unix))]
fn remove_if_present(path: &std::path::Path) -> Result<(), PlatformError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(PlatformError::Remove {
            path: path.to_owned(),
            source,
        }),
    }
}

#[cfg(not(unix))]
struct StopWatch {
    _watcher: notify::RecommendedWatcher,
    changes: tokio::sync::watch::Receiver<()>,
}

#[cfg(not(unix))]
impl StopWatch {
    fn start(path: &std::path::Path) -> Result<Self, PlatformError> {
        use notify::Watcher as _;
        let watching = |source| PlatformError::WatchStop {
            path: path.to_owned(),
            source,
        };
        let (signal, changes) = tokio::sync::watch::channel(());
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                if let Err(error) = event {
                    tracing::warn!(%error, "watching for a stop request");
                }
                signal.send_replace(());
            })
            .map_err(watching)?;
        let folder = path.parent().unwrap_or(path);
        watcher
            .watch(folder, notify::RecursiveMode::NonRecursive)
            .map_err(watching)?;
        Ok(Self {
            _watcher: watcher,
            changes,
        })
    }

    async fn requested(mut self, path: &std::path::Path, own: u32) {
        while !names(path, own) {
            if self.changes.changed().await.is_err() {
                unreachable!("invariant: the stop watch outlives its own sender");
            }
        }
    }
}

#[cfg(not(unix))]
fn names(path: &std::path::Path, own: u32) -> bool {
    match std::fs::read_to_string(path) {
        Ok(text) => match text.trim().parse::<u32>() {
            Ok(pid) => pid == own,
            Err(error) => {
                tracing::debug!(%error, "waiting for a whole stop request");
                false
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => {
            tracing::warn!(%error, path = %path.display(), "read the stop request");
            false
        }
    }
}

async fn stop_requested() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {
                    _ = terminate.recv() => {},
                    _ = tokio::signal::ctrl_c() => {},
                }
            }
            Err(error) => {
                tracing::warn!("listen for SIGTERM: {error}");
                if let Err(error) = tokio::signal::ctrl_c().await {
                    tracing::warn!("listen for Ctrl+C: {error}");
                }
            }
        }
    }
    #[cfg(not(unix))]
    {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::warn!("listen for Ctrl+C: {error}");
        }
    }
    tracing::info!("asked to stop");
}

fn init_tracing(log: Option<&std::path::Path>) -> Result<diagnostics::RecentLines, TracingError> {
    match log {
        Some(path) => {
            let file = diagnostics::log_file::open(path, LOG_LIMIT)?;
            Ok(diagnostics::subscribe("info", std::sync::Mutex::new(file))?)
        }
        None => Ok(diagnostics::subscribe("info", std::io::stderr)?),
    }
}
