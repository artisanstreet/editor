use std::{fs, num::NonZeroU32, path::PathBuf};

use artisan_native_engine::ManagedEngine;
use clap::{Parser, Subcommand, ValueEnum};

use crate::{
    CliError, Result, credentials,
    host_access::{HostAccess, ListenAddress},
    instance::{
        self, NativeInstanceConfig, NativeListenerConfig, NativeRunConfig, NativeRunConfigInput,
    },
    manifest::{InstallationFinalization, InstallationManifest},
    paths::Layout,
    process,
    telemetry::{self, Preference},
};

use super::{
    EngineProfileCommand, INCOMPLETE_INSTALLATION_GUIDANCE, INVALID_INSTALLATION_GUIDANCE,
    engine::{engine_command, profile_surface_error},
};

use super::autostart::{
    autostart, delegate_installer, enable_autostart, setup_native, start,
    unsupported_lifecycle_control,
};
use super::doctor::{doctor, logs};
use super::lifecycle::{status, stop};
use super::open::{OpenFlow, handle_protocol, open};

#[derive(Debug, Parser)]
#[command(
    name = "ae",
    version = artisan_build_info::version_line(),
    about = "Artisan Editor and Forge"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Clone)]
pub struct NativeRunPromptDelivery(pub(super) String);

impl std::fmt::Debug for NativeRunPromptDelivery {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NativeRunPromptDelivery")
            .field("byte_length", &self.0.len())
            .field("category", &"validated")
            .finish()
    }
}

impl NativeRunPromptDelivery {
    pub(super) fn into_string(self) -> String {
        self.0
    }
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Handle one fixed operating-system URL capability.
    #[command(hide = true)]
    Protocol {
        url: String,
    },
    /// Explicitly create or update this home's Forge configuration.
    Setup {
        #[arg(long, required = true, value_name = "PATH")]
        database_path: PathBuf,
        #[arg(long, required = true, value_name = "PATH")]
        custody_path: PathBuf,
        #[arg(long, required = true, value_name = "PATH")]
        readiness_path: PathBuf,
        #[arg(long, required = true, value_parser = parse_positive_u64)]
        admission_timeout_ms: u64,
        #[arg(long, required = true, value_parser = parse_positive_u64)]
        handshake_timeout_ms: u64,
        #[arg(long, required = true, value_parser = parse_positive_u64)]
        request_timeout_ms: u64,
        #[arg(long, required = true, value_parser = parse_positive_u64)]
        drain_timeout_ms: u64,
        #[arg(long, required = true, value_parser = parse_nonzero_u32)]
        admission_capacity: NonZeroU32,
        #[arg(long, required = true, value_parser = parse_nonzero_u32)]
        requests_per_connection: NonZeroU32,
        #[arg(
            long = "native-run-claim-lease-ms",
            required = true,
            value_parser = parse_native_run_duration_ms
        )]
        native_run_claim_lease_ms: u64,
        #[arg(
            long = "native-run-poll-interval-ms",
            required = true,
            value_parser = parse_native_run_duration_ms
        )]
        native_run_poll_interval_ms: u64,
        #[arg(
            long = "native-run-retry-backoff-ms",
            required = true,
            value_parser = parse_native_run_duration_ms
        )]
        native_run_retry_backoff_ms: u64,
        #[arg(
            long = "native-run-shutdown-budget-ms",
            required = true,
            value_parser = parse_native_run_duration_ms
        )]
        native_run_shutdown_budget_ms: u64,
        #[arg(
            long = "native-run-queue-capacity",
            required = true,
            value_parser = parse_nonzero_u32
        )]
        native_run_queue_capacity: NonZeroU32,
        #[arg(
            long = "native-run-max-command-retries",
            required = true,
            value_parser = parse_nonzero_u32
        )]
        native_run_max_command_retries: NonZeroU32,
        #[arg(
            long = "native-run-prompt-delivery",
            required = true,
            value_parser = parse_native_run_prompt_delivery
        )]
        native_run_prompt_delivery: NativeRunPromptDelivery,
        #[arg(long = "native-run-stream-after", required = true)]
        native_run_stream_after: u64,
        /// Also start the Forge with the user's session: a systemd user
        /// service on Linux, a logon task on Windows.
        #[arg(long)]
        autostart: bool,
        /// Listen for other machines at IP:PORT, or at `auto:PORT` (the
        /// default route's IPv4 address, resolved at each start), and publish
        /// a host invitation. Without it the Forge is loopback-only.
        #[arg(long, value_name = "ADDRESS", requires = "host_name")]
        listen: Option<ListenAddress>,
        /// The machine name host invitations carry.
        #[arg(long, value_name = "NAME", requires = "listen")]
        host_name: Option<String>,
    },
    Start {
        #[arg(long)]
        foreground: bool,
    },
    Stop {
        /// Stop only the authenticated Forge with this readiness process identity.
        #[arg(long, hide = true, required = true, value_parser = parse_nonzero_u32)]
        pid: NonZeroU32,
        /// Refuse shutdown when Forge reports live model work. Intended for
        /// installer retirement before an update is activated.
        #[arg(long, hide = true, required = true)]
        if_idle: bool,
    },
    Restart {
        #[arg(long)]
        foreground: bool,
    },
    Status {
        #[arg(long)]
        json: bool,
    },
    Logs {
        #[arg(long, default_value_t = 200)]
        lines: usize,
        #[arg(long)]
        follow: bool,
    },
    Doctor {
        #[arg(long)]
        fix: bool,
        #[arg(long)]
        json: bool,
        #[arg(
            long = "finalization-check",
            hide = true,
            requires = "json",
            conflicts_with = "fix"
        )]
        finalization_check: bool,
    },
    Open {
        /// Open a paired browser at this loopback origin instead of the editor.
        #[arg(long, conflicts_with = "handoff")]
        origin: Option<String>,
        /// Open the paired browser flow instead of the installed editor.
        #[arg(long, conflicts_with = "handoff")]
        browser: bool,
        /// Print a one-time `{endpoint, pair_code}` handoff as JSON on stdout
        /// for a trusted local caller (the installed editor) instead of
        /// launching anything.
        #[arg(long, hide = true)]
        handoff: bool,
    },
    /// Inspect or disable the current-user Forge logon task.
    Autostart {
        /// Remove the current-user Forge logon task.
        #[arg(long)]
        disable: bool,
    },
    Update,
    Uninstall {
        /// Permanently remove Forge data, projects, and conversations.
        #[arg(long)]
        remove_data: bool,
    },
    /// Manage the Forge-owned engine binaries.
    Engine {
        /// Forge database whose state directory owns the engines (defaults to
        /// this installation's native instance).
        #[arg(long, global = true, value_name = "PATH")]
        database: Option<PathBuf>,
        #[command(subcommand)]
        command: EngineCommand,
    },
    /// Inspect or change privacy-preserving observability preferences.
    Telemetry {
        #[command(subcommand)]
        command: TelemetryCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum EngineCommand {
    /// List every managed engine with its install state and selection.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Show one engine's generations, selection, and pending switch.
    Status {
        #[arg(value_enum)]
        engine: EngineArg,
        #[arg(long)]
        json: bool,
    },
    /// List the vendor's published versions, newest first.
    Versions {
        #[arg(value_enum)]
        engine: EngineArg,
        #[arg(long)]
        json: bool,
    },
    /// Install the selected version of one engine, or of every supported engine.
    Install {
        #[arg(value_enum)]
        engine: Option<EngineArg>,
    },
    /// Check the vendor for a newer release and install it when following `latest`.
    Update {
        #[arg(value_enum)]
        engine: Option<EngineArg>,
    },
    /// Hold an engine at an exact version, or return it to `latest`.
    Use {
        #[arg(value_enum)]
        engine: EngineArg,
        /// An exact version such as `2.1.282`, or `latest`.
        selection: String,
    },
    /// Switch back to the previously active generation and hold it.
    Rollback {
        #[arg(value_enum)]
        engine: EngineArg,
    },
    /// Sign in with the managed engine's own login flow under its Forge home.
    Login {
        #[arg(value_enum)]
        engine: EngineArg,
        /// Arguments for the engine's login command (default: its sign-in command).
        #[arg(last = true)]
        args: Vec<String>,
    },
    /// Manage explicit certified `OpenCode2` profile homes.
    Profile {
        #[command(subcommand)]
        command: EngineProfileCommand,
    },
}

/// A managed engine named on the command line.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum EngineArg {
    Claude,
    Codex,
    Cursor,
    Grok,
    Opencode2,
}

impl EngineArg {
    pub(crate) const fn engine(self) -> ManagedEngine {
        match self {
            Self::Claude => ManagedEngine::Claude,
            Self::Codex => ManagedEngine::Codex,
            Self::Cursor => ManagedEngine::Cursor,
            Self::Grok => ManagedEngine::Grok,
            Self::Opencode2 => ManagedEngine::OpenCode2,
        }
    }
}

#[derive(Clone, Copy, Debug, Subcommand)]
pub enum TelemetryCommand {
    /// Print the two independent consent choices without exposing installation identity.
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Change anonymous usage analytics.
    Analytics {
        #[arg(value_enum)]
        choice: TelemetryChoice,
    },
    /// Change sanitized crash reporting.
    CrashReports {
        #[arg(value_enum)]
        choice: TelemetryChoice,
    },
    /// Replace the anonymous installation identifier without changing consent.
    ResetIdentity {
        #[arg(long, required = true)]
        yes: bool,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum TelemetryChoice {
    Enable,
    Disable,
    Unset,
}

impl From<TelemetryChoice> for Preference {
    fn from(value: TelemetryChoice) -> Self {
        match value {
            TelemetryChoice::Enable => Self::Enabled,
            TelemetryChoice::Disable => Self::Disabled,
            TelemetryChoice::Unset => Self::Unset,
        }
    }
}

fn discover_layout(command: Option<&Commands>) -> Result<Layout> {
    let is_profile = matches!(
        command,
        Some(Commands::Engine {
            command: EngineCommand::Profile { .. },
            ..
        })
    );
    let layout = Layout::discover().map_err(|error| {
        if is_profile {
            profile_surface_error()
        } else {
            error
        }
    })?;
    Ok(layout)
}

pub fn run(cli: Cli) -> Result<()> {
    let layout = discover_layout(cli.command.as_ref())?;
    match cli.command.unwrap_or(Commands::Open {
        origin: None,
        browser: false,
        handoff: false,
    }) {
        Commands::Protocol { url } => handle_protocol(&layout, &url),
        Commands::Setup {
            database_path,
            custody_path,
            readiness_path,
            admission_timeout_ms,
            handshake_timeout_ms,
            request_timeout_ms,
            drain_timeout_ms,
            admission_capacity,
            requests_per_connection,
            native_run_claim_lease_ms,
            native_run_poll_interval_ms,
            native_run_retry_backoff_ms,
            native_run_shutdown_budget_ms,
            native_run_queue_capacity,
            native_run_max_command_retries,
            native_run_prompt_delivery,
            native_run_stream_after,
            autostart,
            listen,
            host_name,
        } => {
            require_installation(&layout)?;
            let before = configuration_snapshot(&layout);
            setup_native(
                &layout,
                NativeSetupValues {
                    database_path,
                    custody_path,
                    readiness_path,
                    listener: NativeListenerConfig::new(
                        admission_timeout_ms,
                        handshake_timeout_ms,
                        request_timeout_ms,
                        drain_timeout_ms,
                        admission_capacity,
                        requests_per_connection,
                    ),
                    native_run: NativeRunConfig::new(NativeRunConfigInput {
                        claim_lease_ms: native_run_claim_lease_ms,
                        poll_interval_ms: native_run_poll_interval_ms,
                        retry_backoff_ms: native_run_retry_backoff_ms,
                        shutdown_budget_ms: native_run_shutdown_budget_ms,
                        queue_capacity: native_run_queue_capacity.get(),
                        max_command_retries: native_run_max_command_retries.get(),
                        prompt_delivery: native_run_prompt_delivery.into_string(),
                        stream_after: native_run_stream_after,
                    })?,
                },
            )?;
            match listen.zip(host_name) {
                Some((listen, name)) => HostAccess::new(listen, name)?.write(&layout.root)?,
                None => HostAccess::remove(&layout.root)?,
            }
            delegate_installer(&layout, "repair", false)?;
            if autostart {
                enable_autostart(&layout, configuration_snapshot(&layout) != before)?;
            }
            println!("Configured Forge");
            Ok(())
        }
        Commands::Start { foreground } => start(&layout, foreground).map(|_| ()),
        Commands::Stop { pid, if_idle } => stop(&layout, pid, if_idle),
        Commands::Restart { .. } | Commands::Uninstall { .. } => unsupported_lifecycle_control(),
        Commands::Status { json } => status(&layout, json),
        Commands::Logs { lines, follow } => logs(&layout, lines, follow),
        Commands::Doctor {
            fix,
            json,
            finalization_check,
        } => doctor(&layout, fix, json, finalization_check),
        Commands::Open {
            origin,
            browser,
            handoff,
        } => {
            let flow = if handoff {
                OpenFlow::Handoff
            } else if browser || origin.is_some() {
                OpenFlow::Browser
            } else {
                OpenFlow::Editor
            };
            open(&layout, origin.as_deref(), flow)
        }
        Commands::Autostart { disable } => autostart(&layout, disable),
        Commands::Update => delegate_installer(&layout, "update", false),
        Commands::Engine { database, command } => {
            engine_command(&layout, database.as_deref(), &command)
        }
        Commands::Telemetry { command } => telemetry_command(&layout, command),
    }
}

fn telemetry_command(layout: &Layout, command: TelemetryCommand) -> Result<()> {
    match command {
        TelemetryCommand::Status { json } => {
            let preferences = telemetry::load_or_create(layout)?;
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "crash_reports": preferences.crash_reports,
                        "identity_configured": true,
                        "usage_analytics": preferences.usage_analytics,
                        "version": preferences.version,
                    })
                );
            } else {
                println!("Usage analytics: {}", preferences.usage_analytics.as_str());
                println!("Crash reports: {}", preferences.crash_reports.as_str());
                println!("Anonymous identity: configured");
            }
            Ok(())
        }
        TelemetryCommand::Analytics { choice } => {
            let updated = telemetry::set_usage_analytics(layout, choice.into())?;
            println!("Usage analytics: {}", updated.usage_analytics.as_str());
            Ok(())
        }
        TelemetryCommand::CrashReports { choice } => {
            let updated = telemetry::set_crash_reports(layout, choice.into())?;
            println!("Crash reports: {}", updated.crash_reports.as_str());
            Ok(())
        }
        TelemetryCommand::ResetIdentity { yes } => {
            debug_assert!(yes, "clap requires --yes");
            telemetry::reset_identity(layout)?;
            println!("Anonymous telemetry identity reset");
            Ok(())
        }
    }
}

pub(super) fn require_installation(layout: &Layout) -> Result<InstallationManifest> {
    InstallationManifest::load(&layout.manifest)
}

pub(super) fn require_launchable_installation(layout: &Layout) -> Result<InstallationManifest> {
    let (finalization, manifest) = InstallationManifest::inspect(&layout.manifest);
    match finalization {
        InstallationFinalization::Complete => {
            manifest.ok_or_else(|| CliError::Installation(INVALID_INSTALLATION_GUIDANCE.to_owned()))
        }
        InstallationFinalization::Pending | InstallationFinalization::Missing => Err(
            CliError::Installation(INCOMPLETE_INSTALLATION_GUIDANCE.to_owned()),
        ),
        InstallationFinalization::Invalid => Err(CliError::Installation(
            INVALID_INSTALLATION_GUIDANCE.to_owned(),
        )),
    }
}

#[derive(Debug)]
pub(super) struct NativeSetupValues {
    pub(super) database_path: PathBuf,
    pub(super) custody_path: PathBuf,
    pub(super) readiness_path: PathBuf,
    pub(super) listener: NativeListenerConfig,
    pub(super) native_run: NativeRunConfig,
}

fn parse_nonzero_u32(value: &str) -> std::result::Result<NonZeroU32, String> {
    let value = value
        .parse::<u32>()
        .map_err(|_| "must be a positive 32-bit integer".to_owned())?;
    NonZeroU32::new(value).ok_or_else(|| "must be greater than zero".to_owned())
}

pub(super) fn parse_positive_u64(value: &str) -> std::result::Result<u64, String> {
    let value = value
        .parse::<u64>()
        .map_err(|_| "must be a positive 64-bit integer".to_owned())?;
    if value == 0 {
        return Err("must be greater than zero".to_owned());
    }
    Ok(value)
}

pub(super) fn parse_native_run_duration_ms(value: &str) -> std::result::Result<u64, String> {
    let value = value
        .parse::<u64>()
        .map_err(|_| "must be a positive 64-bit duration in milliseconds".to_owned())?;
    if !instance::is_valid_native_run_duration_ms(value) {
        return Err("must be positive and fit the native-run duration range".to_owned());
    }
    Ok(value)
}

pub(super) fn parse_native_run_prompt_delivery(
    value: &str,
) -> std::result::Result<NativeRunPromptDelivery, String> {
    if !instance::is_valid_native_run_prompt_delivery(value) {
        return Err(
            "must be nonempty, at most 256 bytes, and contain no control characters or line breaks"
                .to_owned(),
        );
    }
    Ok(NativeRunPromptDelivery(value.to_owned()))
}

pub(super) fn load_native_instance(layout: &Layout) -> Result<NativeInstanceConfig> {
    let path = layout.native_instance_path();
    match fs::symlink_metadata(&path) {
        Ok(_) => instance::load_native_config(&path).map_err(CliError::NativeInstance),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err(CliError::MissingInstance)
        }
        Err(source) => Err(CliError::Io {
            context: "inspect native Forge instance",
            source,
        }),
    }
}

pub(super) fn native_launch_spec(layout: &Layout) -> Result<process::ForgeLaunchSpec> {
    let manifest = require_launchable_installation(layout)?;
    let config = load_native_instance(layout)?;
    let credentials = credentials::provision_or_load(&layout.root)?;
    let spec = process::ForgeLaunchSpec::new(&manifest, &config, &credentials)?;
    match HostAccess::load(&layout.root)? {
        Some(access) => Ok(spec.listening_on(access.listen().resolve()?)),
        None => Ok(spec),
    }
}

/// The configuration a running Forge was started with: its instance and
/// host access files. Setup restarts a service Forge when they change.
fn configuration_snapshot(layout: &Layout) -> [Option<Vec<u8>>; 2] {
    [
        fs::read(layout.native_instance_path()).ok(),
        fs::read(HostAccess::path(&layout.root)).ok(),
    ]
}
