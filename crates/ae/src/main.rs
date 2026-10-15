//! `ae`: Artisan's one host binary. Installs the Editor and Forge, manages
//! the Forge service, its paired devices and engines, and is Forge itself
//! (`ae forge serve`).

mod editor;
mod engine;
mod forge;
mod install;
mod paths;
mod service;

use anyhow::Result;
use artisan_api::EngineId;
use clap::{Args, Parser, Subcommand};
use std::net::SocketAddr;
use std::path::PathBuf;

/// `0.4.2 (abc1234, x86_64-unknown-linux-gnu)`
const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("AE_COMMIT"),
    ", ",
    env!("AE_TARGET"),
    ")"
);

#[derive(Parser)]
#[command(name = "ae", version = LONG_VERSION, about = "Install and run Artisan: the Editor, Forge and engines")]
struct Cli {
    /// Forge's data directory.
    #[arg(long, global = true, env = "ARTISAN_FORGE_DATA")]
    data_dir: Option<PathBuf>,
    /// Print changes outside Artisan's own directories (PATH, shortcuts,
    /// links, the service, other processes) instead of making them.
    #[arg(long, global = true)]
    dry_run: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Install the Editor or a Forge (asks where when no target is given).
    Install(install::InstallArgs),
    /// Update every installed product to the latest release.
    Update {
        /// A release.json URL or a directory holding a release.
        #[arg(long, value_name = "SOURCE")]
        from: Option<String>,
    },
    /// Remove Artisan's programs, shortcuts and service (keeps Forge data).
    Uninstall,
    /// Check the installation; `--fix` repairs what it can.
    Doctor {
        #[arg(long)]
        fix: bool,
    },
    /// Run and manage Forge.
    Forge(ForgeArgs),
    /// Manage engine installs and sign-in.
    Engine {
        #[command(subcommand)]
        command: EngineCommand,
    },
    /// Open the Editor.
    Editor {
        #[command(subcommand)]
        command: EditorCommand,
    },
}

#[derive(Args)]
struct ForgeArgs {
    /// Run the command on another host, e.g. `wsl:Ubuntu`.
    #[arg(long, global = true, value_name = "HOST")]
    on: Option<String>,
    #[command(subcommand)]
    command: ForgeCommand,
}

#[derive(Subcommand)]
enum ForgeCommand {
    /// Start the Forge service.
    Start,
    /// Stop the Forge service.
    Stop,
    /// Restart the Forge service.
    Restart,
    /// Whether the Forge service runs.
    Status,
    /// Show Forge's log.
    Logs {
        /// Keep printing new lines.
        #[arg(short, long)]
        follow: bool,
    },
    /// Run Forge in the foreground (what the service runs).
    Serve {
        #[arg(long, env = "ARTISAN_FORGE_LISTEN", default_value_t = forge::default_listen())]
        listen: SocketAddr,
    },
    /// Make a one-time code that pairs an Editor with this Forge.
    Pair {
        /// Print `{code, forge_certificate, expires_at, address}` as JSON.
        #[arg(long)]
        json: bool,
        /// The address to tell the Editor (defaults to the listen address).
        #[arg(long, env = "ARTISAN_FORGE_LISTEN", default_value_t = forge::default_listen())]
        listen: SocketAddr,
    },
    /// Copy the threads, images, skills and engines of the previous Forge
    /// into this one. Safe to repeat; already imported rows are skipped.
    Import {
        /// The old Forge's data directory (default: the installed one).
        #[arg(long)]
        from: Option<PathBuf>,
    },
    /// Normalize retired agent-message records in an offline Forge database.
    Normalize,
    /// List paired devices, or revoke one.
    Devices {
        #[command(subcommand)]
        command: Option<DevicesCommand>,
    },
}

#[derive(Subcommand)]
enum DevicesCommand {
    /// Unpair a device; the id may be shortened.
    Revoke { id: String },
}

#[derive(Subcommand)]
enum EngineCommand {
    /// Every engine and its installed version.
    List,
    /// Install state and sign-in of one engine (or all).
    Status { engine: Option<EngineId> },
    /// Install the newest version unless one is installed.
    Install { engine: EngineId },
    /// Install the newest version.
    Update { engine: EngineId },
    /// Switch to a specific version.
    Use { engine: EngineId, version: String },
    /// Switch back to the previous version.
    Rollback { engine: EngineId },
    /// Sign the engine in to its account.
    Login { engine: EngineId },
}

#[derive(Subcommand)]
enum EditorCommand {
    /// Launch the Editor, optionally on a folder.
    Open { path: Option<PathBuf> },
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    init_tracing(&cli);
    match run(cli) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    let custom_data_dir = cli.data_dir.clone();
    let data_dir = cli.data_dir.clone().map_or_else(paths::forge_data, Ok);
    let dry_run = cli.dry_run;
    match cli.command {
        Command::Install(args) => install::install(&args, custom_data_dir.as_deref(), dry_run),
        Command::Update { from } => {
            install::update(custom_data_dir.as_deref(), from.as_deref(), dry_run)
        }
        Command::Uninstall => install::uninstall(&data_dir?, dry_run),
        Command::Doctor { fix } => install::doctor(fix, dry_run, &data_dir?),
        Command::Forge(ForgeArgs { on: Some(host), .. }) => service::forward(&host),
        Command::Forge(ForgeArgs { on: None, command }) => {
            let data_dir = data_dir?;
            match command {
                ForgeCommand::Serve { listen } => forge::serve(data_dir, listen),
                ForgeCommand::Pair { json, listen } => forge::pair(&data_dir, listen, json),
                ForgeCommand::Import { from } => forge::import(&data_dir, from),
                ForgeCommand::Normalize => forge::normalize(&data_dir, dry_run),
                ForgeCommand::Devices { command: None } => forge::devices(&data_dir),
                ForgeCommand::Devices {
                    command: Some(DevicesCommand::Revoke { id }),
                } => forge::revoke(&data_dir, &id),
                ForgeCommand::Start => service::start(&data_dir, dry_run),
                ForgeCommand::Stop => service::stop(&data_dir, dry_run),
                ForgeCommand::Restart => service::restart(&data_dir, dry_run),
                ForgeCommand::Status => {
                    service::status(&data_dir);
                    Ok(())
                }
                ForgeCommand::Logs { follow } => service::logs(&data_dir, follow),
            }
        }
        Command::Engine { command } => engine::run(command, &data_dir?),
        Command::Editor {
            command: EditorCommand::Open { path },
        } => editor::open(path.as_deref(), dry_run),
    }
}

/// Logs go to stderr; under systemd that is the journal. `ae forge serve`
/// logs at info, everything else only warnings unless `RUST_LOG` says more.
fn init_tracing(cli: &Cli) {
    let serving = matches!(
        &cli.command,
        Command::Forge(ForgeArgs {
            on: None,
            command: ForgeCommand::Serve { .. }
        })
    );
    let default = if serving { "info" } else { "warn" };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();
}
