mod account;
mod bundle;
mod editor;
mod engine;
mod forge;
mod install;
mod legacy;
mod paths;
mod platform;
mod release;
mod service;
mod status;
mod sys;
mod ui;
mod wsl;

use artisan_api::EngineId;
use artisan_paths::App;
use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

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
    #[arg(
        long,
        global = true,
        value_name = "NAME",
        help = "The WSL distribution whose Forge to use (Windows, when several have one)"
    )]
    distro: Option<String>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(about = "Install or reinstall the Editor and Forge (asks where)")]
    Install(Box<install::InstallArgs>),
    #[command(about = "Update what is installed to the newest release")]
    Update {
        #[arg(long, short, help = "Don't ask to confirm")]
        yes: bool,
    },
    #[command(about = "Remove apps (asks which); their data stays unless --purge")]
    Uninstall {
        #[arg(help = "Only these apps")]
        apps: Vec<AppArg>,
        #[arg(
            long,
            help = "Also delete their data: threads, settings, paired devices"
        )]
        purge: bool,
        #[arg(long, short, help = "Don't ask")]
        yes: bool,
    },
    #[command(about = "What is installed, whether Forge runs, and problems")]
    Status {
        #[arg(long)]
        json: bool,
    },
    #[command(about = "Open the Editor, optionally on a folder")]
    Open { path: Option<PathBuf> },
    #[command(about = "Start, stop or restart Forge")]
    Forge {
        #[command(subcommand)]
        command: ForgeCommand,
    },
    #[command(about = "Make a one-time code that pairs an Editor with Forge")]
    Pair {
        #[arg(
            long,
            help = "Print `{code, forge_certificate, forge_fingerprint, expires_at, address}` and return"
        )]
        json: bool,
    },
    #[command(about = "List the devices paired with Forge, or unpair one")]
    Devices {
        #[arg(long)]
        json: bool,
        #[command(subcommand)]
        command: Option<DevicesCommand>,
    },
    #[command(about = "Enroll this Forge machine into an Artisan account")]
    Account(account::AccountArgs),
    #[command(about = "Forge's engines: list, install, sign in")]
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
    #[command(about = "Unpair a device (asks which); the id may be shortened")]
    Revoke {
        id: Option<String>,
        #[arg(long, short)]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum EngineCommand {
    #[command(about = "Every engine, its version and account")]
    List {
        #[arg(long)]
        json: bool,
    },
    #[command(about = "Install engines, or update them to the newest version (asks which)")]
    Install { engines: Vec<EngineId> },
    #[command(about = "Sign an engine in to its account (asks which)")]
    Login {
        engine: Option<EngineId>,
        #[arg(
            long,
            help = "Claude only: sign in to claude.ai for claude.ai mode, in a browser window on this device"
        )]
        claude_ai: bool,
        #[arg(
            long,
            hide = true,
            help = "Read the claude.ai session from stdin: an `ae` on the device with the browser captured it"
        )]
        session_stdin: bool,
        #[arg(
            long,
            hide = true,
            help = "Capture the claude.ai session here and print it, for an `ae` in WSL to keep"
        )]
        capture_only: bool,
    },
    #[command(about = "Sign an engine out of its account (asks which)")]
    Logout {
        engine: Option<EngineId>,
        #[arg(long, help = "Claude only: forget the claude.ai session")]
        claude_ai: bool,
    },
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    let recent = match diagnostics::subscribe("warn", std::io::stderr) {
        Ok(recent) => recent,
        Err(error) => {
            ui::status(format_args!("error: {}", diagnostics::chain(&error)));
            return std::process::ExitCode::FAILURE;
        }
    };
    match App::Ae.data() {
        Ok(data) => diagnostics::CrashReporter {
            app: App::Ae,
            version: LONG_VERSION,
            data,
            recent,
        }
        .install(),
        Err(error) => tracing::warn!(
            "crash reports are off: find ae's data folder: {}",
            diagnostics::chain(&error)
        ),
    }
    match run(cli) {
        Ok(code) => code,
        Err(error) => {
            ui::status(format_args!("error: {}", diagnostics::chain(&error)));
            std::process::ExitCode::FAILURE
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum AeError {
    #[error(transparent)]
    ForgeTarget(#[from] wsl::ForgeTargetError),
    #[error(transparent)]
    Forward(#[from] wsl::ForwardError),
    #[error(transparent)]
    ClaudeAiIntoWsl(#[from] engine::IntoWslError),
    #[error(transparent)]
    Install(#[from] install::InstallError),
    #[error(transparent)]
    Update(#[from] install::UpdateError),
    #[error(transparent)]
    Uninstall(#[from] install::UninstallError),
    #[error(transparent)]
    Open(#[from] editor::OpenError),
    #[error(transparent)]
    ForgeService(#[from] ForgeServiceError),
    #[error(transparent)]
    Pair(#[from] forge::PairError),
    #[error(transparent)]
    Devices(#[from] forge::ListDevicesError),
    #[error(transparent)]
    Unpair(#[from] forge::UnpairError),
    #[error(transparent)]
    Account(#[from] account::AccountError),
    #[error(transparent)]
    Engine(#[from] engine::EngineError),
}

#[derive(Debug, thiserror::Error)]
enum ForgeServiceError {
    #[error(transparent)]
    Locate(#[from] paths::LocateError),
    #[error("check whether Forge is running")]
    Running(#[source] service::RunningError),
    #[error("Forge isn't installed here; run `ae install`")]
    NotInstalled,
    #[error(transparent)]
    Address(#[from] service::AddressError),
    #[error(transparent)]
    Start(#[from] service::StartError),
    #[error(transparent)]
    Stop(#[from] service::StopError),
    #[error(transparent)]
    Restart(#[from] service::RestartError),
}

fn run(cli: Cli) -> Result<std::process::ExitCode, AeError> {
    let apps = |apps: Vec<AppArg>| -> Vec<App> { apps.into_iter().map(App::from).collect() };
    let forge_command = matches!(
        cli.command,
        Command::Forge { .. }
            | Command::Pair { .. }
            | Command::Devices { .. }
            | Command::Account(_)
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
        if let Command::Engine {
            command:
                EngineCommand::Login {
                    claude_ai: true,
                    session_stdin: false,
                    ..
                },
        } = &cli.command
        {
            return Ok(engine::claude_ai_into_wsl(&distro)?);
        }
        return Ok(wsl::forward(&distro)?);
    }
    let ran: Result<(), AeError> = match cli.command {
        Command::Install(args) => Ok(install::install(&args)?),
        Command::Update { yes } => Ok(install::update(yes)?),
        Command::Uninstall {
            apps: chosen,
            purge,
            yes,
        } => Ok(install::uninstall(&apps(chosen), purge, yes)?),
        Command::Status { json } => {
            status::status(json);
            Ok(())
        }
        Command::Open { path } => Ok(editor::open(path.as_deref())?),
        Command::Forge { command } => Ok(forge_service(&command)?),
        Command::Pair { json } => Ok(forge::pair(json)?),
        Command::Devices { json, command } => match command {
            None => Ok(forge::devices(json)?),
            Some(DevicesCommand::Revoke { id, yes }) => Ok(forge::revoke(id.as_deref(), yes)?),
        },
        Command::Account(args) => Ok(account::run(&args)?),
        Command::Engine { command } => Ok(engine::run(command)?),
    };
    ran.map(|()| std::process::ExitCode::SUCCESS)
}

fn forge_service(command: &ForgeCommand) -> Result<(), ForgeServiceError> {
    if !paths::binary(App::Forge)?.is_file() {
        return Err(ForgeServiceError::NotInstalled);
    }
    let address = service::address()?;
    let started = |pid: Option<u32>| {
        let pid = pid.map(|pid| format!(" (pid {pid})")).unwrap_or_default();
        format!("Forge is running on {address}{pid}")
    };
    match command {
        ForgeCommand::Start => {
            if service::is_running().map_err(ForgeServiceError::Running)?
                && service::listening(address)
            {
                ui::success(format!("Forge is already running on {address}"));
                return Ok(());
            }
            let pid = ui::task("Starting Forge", "Forge started", || {
                service::start(service::Readiness::Recorded)
            })?;
            ui::success(started(pid));
            Ok(())
        }
        ForgeCommand::Stop => {
            let was = ui::task("Stopping Forge", "Forge stopped", service::stop)?;
            if !was {
                ui::info("Forge wasn't running.");
            }
            Ok(())
        }
        ForgeCommand::Restart => {
            let pid = ui::task("Restarting Forge", "Forge restarted", service::restart)?;
            ui::success(started(pid));
            Ok(())
        }
    }
}
