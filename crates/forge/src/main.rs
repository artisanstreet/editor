#![expect(
    clippy::print_stderr,
    reason = "pre-existing violations of the workspace lint policy, cleared in plan.md phase 2"
)]
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
    /// The data folder (default: Forge's folder under Artisan Street).
    #[arg(long, value_name = "DIR")]
    data: Option<PathBuf>,
    /// Where to listen (default: `listen` in the data folder's config.json,
    /// else 127.0.0.1:47900).
    #[arg(long, value_name = "ADDRESS")]
    listen: Option<SocketAddr>,
    /// Append the log to this file instead of the terminal.
    #[arg(long, value_name = "FILE")]
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
                eprintln!("error: print the schema version: {error}");
                std::process::ExitCode::FAILURE
            }
        };
    }
    if cfg!(windows) && cli.log.is_none() {
        cli.log = cli
            .data
            .clone()
            .or_else(|| App::Forge.data().ok())
            .map(|data| data.join("forge.log"));
    }
    let recent = match init_tracing(cli.log.as_deref()) {
        Ok(recent) => recent,
        Err(error) => {
            eprintln!("error: {}", diagnostics::chain(&error));
            return std::process::ExitCode::FAILURE;
        }
    };
    let data_dir = match cli.data.clone() {
        Some(dir) => dir,
        None => match App::Forge.data() {
            Ok(dir) => dir,
            Err(error) => {
                tracing::error!("find Forge's data folder: {error}");
                eprintln!("error: find Forge's data folder: {error}");
                return std::process::ExitCode::FAILURE;
            }
        },
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
            eprintln!("error: {chain}");
            std::process::ExitCode::FAILURE
        }
    }
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
                        .member,
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
                forge_http::upgrade::serve(config, cli.handoff.clone(), log, stop)
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
    let forge = forge_http::start(config).await?;
    remove_if_present(&stop_file)?;
    std::fs::write(&pid_file, std::process::id().to_string()).map_err(|source| {
        PlatformError::WritePid {
            path: pid_file.clone(),
            source,
        }
    })?;
    forge
        .run_until(async {
            tokio::select! {
                () = stop => {},
                () = stop_file_appears(&stop_file) => tracing::info!("asked to stop by {}", stop_file.display()),
            }
        })
        .await;
    remove_if_present(&pid_file)?;
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
async fn stop_file_appears(path: &std::path::Path) {
    while !path.exists() {
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
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
