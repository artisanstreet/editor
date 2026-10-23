//! `ae`: installs and manages Artisan's apps (Forge, the Editor and itself)
//! on this machine, and Forge's paired devices and engines.
//!
//! In a terminal it asks what it needs to know; every question has a flag,
//! so agents and scripts run it without one.

mod editor;
mod engine;
mod forge;
mod install;
mod legacy;
mod platform;
mod release;
mod service;
mod status;
mod sys;
mod ui;
mod wsl;

use anyhow::Result;
use artisan_api::EngineId;
use artisan_paths::App;
use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// `0.4.2 (abc1234, x86_64-unknown-linux-gnu)`
pub const LONG_VERSION: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    " (",
    env!("ARTISAN_COMMIT"),
    ", ",
    env!("ARTISAN_TARGET"),
    ")"
);

#[derive(Parser)]
#[command(name = "ae", version = LONG_VERSION, about = "Install and manage Artisan: Forge, the Editor and engines")]
struct Cli {
    /// The WSL distribution whose Forge to use (Windows, when several have one).
    #[arg(long, global = true, value_name = "NAME")]
    distro: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Install or reinstall the Editor and Forge (asks where).
    Install(install::InstallArgs),
    /// Update what is installed to the newest release.
    Update {
        /// Only these apps.
        apps: Vec<AppArg>,
        /// Don't ask which.
        #[arg(long, short)]
        yes: bool,
    },
    /// Remove apps (asks which); their data stays unless --purge.
    Uninstall {
        /// Only these apps.
        apps: Vec<AppArg>,
        /// Also delete their data: threads, settings, paired devices.
        #[arg(long)]
        purge: bool,
        /// Don't ask.
        #[arg(long, short)]
        yes: bool,
    },
    /// What is installed, whether Forge runs, and problems.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Open the Editor, optionally on a folder.
    Open { path: Option<PathBuf> },
    /// Start, stop or restart Forge.
    Forge {
        #[command(subcommand)]
        command: ForgeCommand,
    },
    /// Make a one-time code that pairs an Editor with Forge.
    Pair {
        /// Print `{code, forge_certificate, forge_fingerprint, expires_at, address}` and return.
        #[arg(long)]
        json: bool,
    },
    /// List the devices paired with Forge, or unpair one.
    Devices {
        #[arg(long)]
        json: bool,
        #[command(subcommand)]
        command: Option<DevicesCommand>,
    },
    /// Forge's engines: list, install, sign in.
    Engine {
        #[command(subcommand)]
        command: EngineCommand,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum AppArg {
    Ae,
    Forge,
    Editor,
}

impl From<AppArg> for App {
    fn from(app: AppArg) -> Self {
        match app {
            AppArg::Ae => Self::Ae,
            AppArg::Forge => Self::Forge,
            AppArg::Editor => Self::Editor,
        }
    }
}

#[derive(Subcommand)]
enum ForgeCommand {
    Start,
    Stop,
    Restart,
}

#[derive(Subcommand)]
enum DevicesCommand {
    /// Unpair a device (asks which); the id may be shortened.
    Revoke {
        id: Option<String>,
        #[arg(long, short)]
        yes: bool,
    },
}

#[derive(Subcommand)]
pub enum EngineCommand {
    /// Every engine, its version and account.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Install engines, or update them to the newest version (asks which).
    Install { engines: Vec<EngineId> },
    /// Sign an engine in to its account (asks which).
    Login {
        engine: Option<EngineId>,
        /// Claude only: sign in to claude.ai for Native Claude Mode, in a
        /// browser window on this device.
        #[arg(long)]
        claude_ai: bool,
        /// Read the claude.ai session from stdin: an `ae` on the device
        /// with the browser captured it.
        #[arg(long, hide = true)]
        session_stdin: bool,
        /// Capture the claude.ai session here and print it, for an `ae` in
        /// WSL to keep.
        #[arg(long, hide = true)]
        capture_only: bool,
    },
    /// Sign an engine out of its account (asks which).
    Logout {
        engine: Option<EngineId>,
        /// Claude only: forget the claude.ai session.
        #[arg(long)]
        claude_ai: bool,
    },
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    init_tracing();
    match run(cli) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    let apps = |apps: Vec<AppArg>| -> Vec<App> { apps.into_iter().map(App::from).collect() };
    // Forge's commands go to WSL when Forge is there rather than here.
    // A claude.ai capture runs where the browser is, never forwarded.
    let forge_command = matches!(
        cli.command,
        Command::Forge { .. }
            | Command::Pair { .. }
            | Command::Devices { .. }
            | Command::Engine { .. }
    ) && !matches!(
        cli.command,
        Command::Engine {
            command: EngineCommand::Login {
                capture_only: true,
                ..
            }
        }
    );
    if forge_command && let Some(distro) = wsl::forge_target(cli.distro.as_deref())? {
        // The browser is on this side: sign in here, and hand the session
        // to the `ae` beside the Forge.
        if let Command::Engine {
            command:
                EngineCommand::Login {
                    claude_ai: true,
                    session_stdin: false,
                    ..
                },
        } = &cli.command
        {
            return engine::claude_ai_into_wsl(&distro);
        }
        return wsl::forward(&distro);
    }
    match cli.command {
        Command::Install(args) => install::install(&args),
        Command::Update { apps: chosen, yes } => install::update(&apps(chosen), yes),
        Command::Uninstall {
            apps: chosen,
            purge,
            yes,
        } => install::uninstall(&apps(chosen), purge, yes),
        Command::Status { json } => {
            status::status(json);
            Ok(())
        }
        Command::Open { path } => editor::open(path.as_deref()),
        Command::Forge { command } => forge_service(&command),
        Command::Pair { json } => forge::pair(json),
        Command::Devices { json, command } => match command {
            None => forge::devices(json),
            Some(DevicesCommand::Revoke { id, yes }) => forge::revoke(id.as_deref(), yes),
        },
        Command::Engine { command } => engine::run(command),
    }
}

fn forge_service(command: &ForgeCommand) -> Result<()> {
    anyhow::ensure!(
        App::Forge.binary()?.is_file(),
        "Forge isn't installed here; run `ae install`"
    );
    let address = service::address();
    let started = |pid: Option<u32>| {
        let pid = pid.map(|pid| format!(" (pid {pid})")).unwrap_or_default();
        format!("Forge is running on {address}{pid}")
    };
    match command {
        ForgeCommand::Start => {
            if service::is_running() && service::listening(address) {
                ui::success(format!("Forge is already running on {address}"));
                return Ok(());
            }
            ui::task("Starting Forge", "Forge started", service::start)
                .map(|pid| ui::success(started(pid)))
        }
        ForgeCommand::Stop => {
            let was = ui::task("Stopping Forge", "Forge stopped", service::stop)?;
            if !was {
                ui::info("Forge wasn't running.");
            }
            Ok(())
        }
        ForgeCommand::Restart => ui::task("Restarting Forge", "Forge restarted", service::restart)
            .map(|pid| ui::success(started(pid))),
    }
}

/// Warnings and up on stderr, unless `RUST_LOG` says more.
fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();
}
