//! `forge`: Artisan Forge. Serves its data folder to paired Editors until
//! stopped. `ae forge start` runs it as a service; run by hand it logs to
//! the terminal. On Windows it has no console (its autostart would open
//! one), so it always logs to `forge.log` in its data folder.
#![cfg_attr(windows, windows_subsystem = "windows")]

use anyhow::{Context, Result};
use artisan_paths::App;
use clap::Parser;
use std::net::SocketAddr;
use std::path::PathBuf;

#[cfg(feature = "dhat-heap")]
#[global_allocator]
static ALLOCATOR: dhat::Alloc = dhat::Alloc;

/// `0.4.2 (abc1234, x86_64-unknown-linux-gnu)`
const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("ARTISAN_COMMIT"),
    ", ",
    env!("ARTISAN_TARGET"),
    ")"
);

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
}

fn main() -> std::process::ExitCode {
    let mut cli = Cli::parse();
    if cfg!(windows) && cli.log.is_none() {
        cli.log = cli
            .data
            .clone()
            .or_else(|| App::Forge.data().ok())
            .map(|data| data.join("forge.log"));
    }
    if let Err(error) = init_tracing(cli.log.as_deref()) {
        eprintln!("error: {error:#}");
        return std::process::ExitCode::FAILURE;
    }
    match run(cli) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!("{error:#}");
            eprintln!("error: {error:#}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    let data_dir = match cli.data {
        Some(dir) => dir,
        None => App::Forge.data().context("find Forge's data folder")?,
    };
    let listen = match cli.listen {
        Some(listen) => listen,
        None => forge_http::configured_listen(&data_dir)?,
    };
    tracing::info!(version = LONG_VERSION, data = %data_dir.display(), %listen, "starting Forge");
    #[cfg(feature = "dhat-heap")]
    let profiler = {
        let path = data_dir.join("dhat-heap.json");
        tracing::info!(
            "heap profile: writing {} when asked to stop",
            path.display()
        );
        dhat::Profiler::builder()
            .file_name(path)
            .trim_backtraces(Some(32))
            .build()
    };
    let stop = async move {
        stop_requested().await;
        // Written before the runs settle, so "At t-end" is Forge's live state.
        #[cfg(feature = "dhat-heap")]
        drop(profiler);
    };
    tokio::runtime::Runtime::new()
        .context("start the async runtime")?
        .block_on(forge_http::serve(
            forge_http::ServeConfig { data_dir, listen },
            stop,
        ))
}

/// Resolves when the service manager or the terminal asks Forge to stop
/// (SIGTERM, SIGINT, Ctrl+C), so live runs settle before it exits.
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

/// Info and up, unless `RUST_LOG` says otherwise. Under systemd stderr is
/// the journal.
fn init_tracing(log: Option<&std::path::Path>) -> Result<()> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false);
    match log {
        Some(path) => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("create {}", parent.display()))?;
            }
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .with_context(|| format!("open {}", path.display()))?;
            builder.with_writer(std::sync::Mutex::new(file)).init();
        }
        None => builder.with_writer(std::io::stderr).init(),
    }
    Ok(())
}
