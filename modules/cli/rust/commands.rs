use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    net::SocketAddr,
    num::NonZeroU32,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use artisan_domain::{RequestId, UnixMillis};
use artisan_protocol::{
    ClientRequest, ErrorCode, FrameId, Hello, HelloCredential, LifecycleRequest, LifecycleResponse,
    LifecycleState, LifecycleStatus, LifecycleStopDisposition, LifecycleStopReceipt,
    ProtocolVersion, ResponsePayload, VersionOffer, WireEnvelope, WireEnvelopeBody,
};
use artisan_transport::{
    CancelHandle, ClientRequestError, ClientSession, ClientSessionError, ClientSessionLimits,
    LoopbackTarget, PinnedIdentity, RequestOutcome,
};
#[cfg(test)]
use artisan_transport::{DeadlineError, OperationKind};
use clap::{Parser, Subcommand, ValueEnum};
use rustls_pki_types::CertificateDer;

use crate::{
    CliError, Result,
    credentials::{
        self, ForgeCredentialError, ForgeCredentialPaths, ReconnectAttempt, ReconnectBinding,
        ReconnectCapabilityStore,
    },
    engine_catalog::{NativeOpenCode2Authority, OpenCode2Inspection},
    engine_install::{self, InstallOutcome},
    error::io,
    http::{self, PairResponse},
    instance::{
        self, NativeInstanceConfig, NativeListenerConfig, NativeRunConfig, NativeRunConfigInput,
    },
    manifest::InstallationManifest,
    paths::Layout,
    payload, process,
    telemetry::{self, Preference},
};

pub use crate::engine_profiles::{EngineProfileCommand, EngineProfileHomeArg};

const MAX_LOG_BYTES: u64 = 1024 * 1024;
const MAX_FOLLOW_BYTES: u64 = 64 * 1024;
// A cold installed Forge can take more than 20 seconds to initialize its SEA
// runtime and durable state; leave enough time for the first editor handoff.
const FORGE_READY_TIMEOUT: Duration = Duration::from_secs(30);
const FORGE_START_LAUNCH_URL: &str = "artisan://forge/start";
const AUTOSTART_TASK_NAME: &str = "Artisan Forge";
static NEXT_LIFECYCLE_FRAME: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Parser)]
#[command(name = "ae", version, about = "Artisan Editor and Forge")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Clone)]
pub struct NativeRunPromptDelivery(String);

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
    fn into_string(self) -> String {
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
        #[arg(long)]
        autostart: bool,
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
    /// Inspect the managed native engine catalog.
    Engine {
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
    /// List installed native engines and their verified generation metadata.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Install the certified native `OpenCode2` engine.
    Install,
    /// Manage explicit certified `OpenCode2` profile homes.
    Profile {
        #[command(subcommand)]
        command: EngineProfileCommand,
    },
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
    let is_install = matches!(
        command,
        Some(Commands::Engine {
            command: EngineCommand::Install,
        })
    );
    let is_profile = matches!(
        command,
        Some(Commands::Engine {
            command: EngineCommand::Profile { .. },
        })
    );
    let layout = Layout::discover().map_err(|error| {
        if is_install {
            CliError::OpenCode2Install {
                reason: "installation_invalid",
            }
        } else if is_profile {
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
        } => {
            require_installation(&layout)?;
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
            delegate_installer(&layout, "repair", false)?;
            if autostart {
                enable_autostart(&layout)?;
            }
            println!("Configured Forge");
            Ok(())
        }
        Commands::Start { foreground } => start(&layout, foreground).map(|_| ()),
        Commands::Stop { pid, if_idle } => stop(&layout, pid, if_idle),
        Commands::Restart { .. } | Commands::Uninstall { .. } => unsupported_lifecycle_control(),
        Commands::Status { json } => status(&layout, json),
        Commands::Logs { lines, follow } => logs(&layout, lines, follow),
        Commands::Doctor { fix, json } => doctor(&layout, fix, json),
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
        Commands::Autostart { disable } => autostart(disable),
        Commands::Update => delegate_installer(&layout, "update", false),
        Commands::Engine { command } => engine_command(&layout, &command),
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

fn require_installation(layout: &Layout) -> Result<InstallationManifest> {
    InstallationManifest::load(&layout.manifest)
}

fn engine_command(layout: &Layout, command: &EngineCommand) -> Result<()> {
    if matches!(command, EngineCommand::Install) {
        require_installation(layout).map_err(|_| CliError::OpenCode2Install {
            reason: "installation_invalid",
        })?;
        let instance = load_native_instance(layout).map_err(|_| CliError::OpenCode2Install {
            reason: "instance_invalid",
        })?;
        return match engine_install::install(&instance).map_err(|error| {
            CliError::OpenCode2Install {
                reason: error.cli_reason(),
            }
        })? {
            InstallOutcome::Installed => {
                println!("OpenCode2 installed");
                Ok(())
            }
            InstallOutcome::AlreadyInstalled => {
                println!("OpenCode2 already installed");
                Ok(())
            }
        };
    }

    let is_profile = matches!(command, EngineCommand::Profile { .. });
    if is_profile {
        require_installation(layout).map_err(|_| profile_surface_error())?;
    } else {
        require_installation(layout)?;
    }
    let instance = load_native_instance(layout).map_err(|error| {
        if is_profile {
            profile_surface_error()
        } else {
            match error {
                CliError::MissingInstance => CliError::MissingInstance,
                _ => CliError::OpenCode2Authority {
                    reason: "instance_invalid",
                },
            }
        }
    })?;
    match command {
        EngineCommand::List { json } => list_engines(&instance, *json),
        EngineCommand::Install => unreachable!("install is handled above"),
        EngineCommand::Profile { command } => crate::engine_profiles::run(&instance, command),
    }
}

fn profile_surface_error() -> CliError {
    CliError::OpenCode2Profile {
        reason: "profile_registry_invalid",
    }
}

fn list_engines(instance: &NativeInstanceConfig, json: bool) -> Result<()> {
    let authority = NativeOpenCode2Authority::new();
    let spec = NativeOpenCode2Authority::certified_install_spec();
    let inspection = authority.inspect(instance.database_path());
    match inspection {
        Ok(OpenCode2Inspection::UnsupportedPlatform) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "schema": "artisan-engine-list-v1",
                        "engines": [{
                            "engine_id": spec.engine_id(),
                            "status": "unsupported_platform",
                        }],
                    })
                );
            } else {
                println!("OpenCode2: unsupported platform");
            }
            Ok(())
        }
        Ok(OpenCode2Inspection::NotInstalled) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "schema": "artisan-engine-list-v1",
                        "engines": [{
                            "engine_id": spec.engine_id(),
                            "status": "not_installed",
                        }],
                    })
                );
            } else {
                println!("OpenCode2: not installed");
            }
            Ok(())
        }
        Ok(OpenCode2Inspection::Ready(generation)) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "schema": "artisan-engine-list-v1",
                        "engines": [{
                            "engine_id": spec.engine_id(),
                            "status": "ready",
                            "generation": generation.generation_id(),
                            "version": spec.version(),
                            "upstream_commit": spec.upstream_commit(),
                            "binary": spec.binary(),
                            "size_bytes": spec.executable_size_bytes(),
                            "sha256": spec.executable_sha256_hex(),
                        }],
                    })
                );
            } else {
                println!(
                    "OpenCode2: ready ({}, generation {})",
                    spec.version(),
                    generation.generation_id()
                );
            }
            Ok(())
        }
        Err(error) => {
            let reason = error.cli_reason();
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "schema": "artisan-engine-list-v1",
                        "engines": [{
                            "engine_id": spec.engine_id(),
                            "status": "invalid",
                            "reason": reason,
                        }],
                    })
                );
            }
            Err(CliError::OpenCode2Authority { reason })
        }
    }
}

#[derive(Debug)]
struct NativeSetupValues {
    database_path: PathBuf,
    custody_path: PathBuf,
    readiness_path: PathBuf,
    listener: NativeListenerConfig,
    native_run: NativeRunConfig,
}

fn parse_nonzero_u32(value: &str) -> std::result::Result<NonZeroU32, String> {
    let value = value
        .parse::<u32>()
        .map_err(|_| "must be a positive 32-bit integer".to_owned())?;
    NonZeroU32::new(value).ok_or_else(|| "must be greater than zero".to_owned())
}

fn parse_positive_u64(value: &str) -> std::result::Result<u64, String> {
    let value = value
        .parse::<u64>()
        .map_err(|_| "must be a positive 64-bit integer".to_owned())?;
    if value == 0 {
        return Err("must be greater than zero".to_owned());
    }
    Ok(value)
}

fn parse_native_run_duration_ms(value: &str) -> std::result::Result<u64, String> {
    let value = value
        .parse::<u64>()
        .map_err(|_| "must be a positive 64-bit duration in milliseconds".to_owned())?;
    if !instance::is_valid_native_run_duration_ms(value) {
        return Err("must be positive and fit the native-run duration range".to_owned());
    }
    Ok(value)
}

fn parse_native_run_prompt_delivery(
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

fn setup_native(layout: &Layout, values: NativeSetupValues) -> Result<()> {
    let credential_paths = ForgeCredentialPaths::from_home(&layout.root)?;
    let instance_path = layout.native_instance_path();
    let instance_id = match fs::symlink_metadata(&instance_path) {
        Ok(_) => NativeInstanceConfig::load(&instance_path)?.instance_id(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => instance::mint_instance_id()?,
        Err(source) => {
            return Err(CliError::Io {
                context: "inspect native Forge instance",
                source,
            });
        }
    };
    let config = NativeInstanceConfig::new_with_instance_id(
        instance_id,
        values.database_path,
        values.custody_path,
        values.readiness_path,
        credential_paths.manifest_path().to_path_buf(),
        values.listener,
        values.native_run,
    )?;
    fs::create_dir_all(&layout.root).map_err(io("create Artisan home directory"))?;
    let provisioned = credentials::provision_or_load(&layout.root)?;
    process::validate_credential_manifest(&config, &provisioned)?;
    config.write_to_home(&layout.root)?;
    Ok(())
}

fn load_native_instance(layout: &Layout) -> Result<NativeInstanceConfig> {
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

fn native_launch_spec(
    layout: &Layout,
    manifest: &InstallationManifest,
) -> Result<process::ForgeLaunchSpec> {
    let config = load_native_instance(layout)?;
    let credentials = credentials::provision_or_load(&layout.root)?;
    process::ForgeLaunchSpec::new(manifest, &config, &credentials)
}

fn start(layout: &Layout, foreground: bool) -> Result<process::StartResult> {
    let manifest = require_installation(layout)?;
    telemetry::load_or_create(layout)?;
    let spec = native_launch_spec(layout, &manifest)?;
    payload::require_verified(&manifest.version_root())?;
    process::start_until(&spec, foreground, Instant::now() + FORGE_READY_TIMEOUT)
}

fn unsupported_lifecycle_control() -> Result<()> {
    Err(CliError::UnsupportedLifecycleControl)
}

fn autostart(disable: bool) -> Result<()> {
    if disable {
        disable_autostart()?;
        println!("disabled");
    } else {
        println!(
            "{}",
            if autostart_enabled()? {
                "enabled"
            } else {
                "disabled"
            }
        );
    }
    Ok(())
}

fn enable_autostart(layout: &Layout) -> Result<()> {
    let manifest = require_installation(layout)?;
    let permanent_ae = manifest.permanent_ae_path.as_deref().ok_or_else(|| {
        CliError::Installation(
            "the installation has no permanent ae launcher path; run `ae doctor --fix` before enabling autostart"
                .into(),
        )
    })?;
    let launcher = stable_launcher_kind(permanent_ae).ok_or_else(|| {
        CliError::Installation(format!(
            "the permanent ae launcher at {} must be an absolute ae.exe, ae.cmd, or ae.bat file; run `ae doctor --fix` before enabling autostart",
            permanent_ae.display()
        ))
    })?;
    if !permanent_ae.is_absolute() || !permanent_ae.is_file() {
        return Err(CliError::Installation(format!(
            "the permanent ae launcher is unavailable at {}; run `ae doctor --fix` before enabling autostart",
            permanent_ae.display()
        )));
    }
    let action = scheduled_task_action(
        permanent_ae,
        launcher,
        match launcher {
            StableLauncher::Executable => None,
            StableLauncher::CommandScript => Some(trusted_windows_command_processor()?),
        },
    )?;
    create_autostart_task(&action)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StableLauncher {
    Executable,
    CommandScript,
}

fn stable_launcher_kind(path: &Path) -> Option<StableLauncher> {
    match path.file_name()?.to_str()? {
        name if name.eq_ignore_ascii_case("ae.exe") => Some(StableLauncher::Executable),
        name if name.eq_ignore_ascii_case("ae.cmd") || name.eq_ignore_ascii_case("ae.bat") => {
            Some(StableLauncher::CommandScript)
        }
        _ => None,
    }
}

fn scheduled_task_action(
    permanent_ae: &Path,
    launcher: StableLauncher,
    command_processor: Option<PathBuf>,
) -> Result<String> {
    match launcher {
        StableLauncher::Executable => Ok(format!("\"{}\" start", permanent_ae.display())),
        StableLauncher::CommandScript => {
            let command_processor = command_processor.ok_or_else(|| {
                CliError::Installation(
                    "no trusted Windows command processor is available for the permanent ae script"
                        .into(),
                )
            })?;
            reject_cmd_metacharacters(permanent_ae)?;
            reject_cmd_metacharacters(&command_processor)?;
            Ok(format!(
                "\"{}\" /d /s /c \"\"{}\" start\"",
                command_processor.display(),
                permanent_ae.display()
            ))
        }
    }
}

fn reject_cmd_metacharacters(path: &Path) -> Result<()> {
    let path = path.to_str().ok_or_else(|| {
        CliError::Installation("the permanent ae script path is not valid Unicode".into())
    })?;
    if path.chars().any(|character| {
        matches!(
            character,
            '%' | '!' | '^' | '&' | '|' | '<' | '>' | '(' | ')' | '"' | '\r' | '\n'
        )
    }) {
        return Err(CliError::Installation(
            "the permanent ae script path contains characters unsafe for Windows cmd.exe".into(),
        ));
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn trusted_windows_command_processor() -> Result<PathBuf> {
    let system_root = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| {
            CliError::Installation(
                "Windows SystemRoot is unavailable; cannot safely schedule the permanent ae script"
                    .into(),
            )
        })?;
    let command_processor = system_root.join("System32").join("cmd.exe");
    if !command_processor.is_file() {
        return Err(CliError::Installation(format!(
            "trusted Windows command processor is unavailable at {}; run `ae doctor --fix`",
            command_processor.display()
        )));
    }
    Ok(command_processor)
}

#[cfg(not(target_os = "windows"))]
fn trusted_windows_command_processor() -> Result<PathBuf> {
    Err(CliError::Unsupported(
        "Forge autostart uses Windows Task Scheduler and is unavailable on this platform".into(),
    ))
}

fn scheduled_task_create_args(action: &str) -> Vec<String> {
    vec![
        "/Create".into(),
        "/TN".into(),
        AUTOSTART_TASK_NAME.into(),
        "/TR".into(),
        action.into(),
        "/SC".into(),
        "ONLOGON".into(),
        "/RL".into(),
        "LIMITED".into(),
        "/F".into(),
    ]
}

#[cfg(target_os = "windows")]
fn create_autostart_task(action: &str) -> Result<()> {
    let status = hidden_schtasks(
        &scheduled_task_create_args(action),
        "create Forge autostart task",
    )?;
    if status.success() {
        Ok(())
    } else {
        Err(CliError::Control(format!(
            "could not create current-user Forge autostart task ({status})"
        )))
    }
}

#[cfg(not(target_os = "windows"))]
fn create_autostart_task(_: &str) -> Result<()> {
    Err(CliError::Unsupported(
        "Forge autostart uses Windows Task Scheduler and is unavailable on this platform".into(),
    ))
}

#[cfg(target_os = "windows")]
fn disable_autostart() -> Result<()> {
    let query_status = hidden_schtasks(
        &["/Query".into(), "/TN".into(), AUTOSTART_TASK_NAME.into()],
        "inspect Forge autostart task before removal",
    )?;
    if matches!(
        scheduled_task_deletion(query_status.success(), true),
        ScheduledTaskDeletion::AlreadyAbsent
    ) {
        return Ok(());
    }
    let delete_status = hidden_schtasks(
        &[
            "/Delete".into(),
            "/TN".into(),
            AUTOSTART_TASK_NAME.into(),
            "/F".into(),
        ],
        "remove Forge autostart task",
    )?;
    match scheduled_task_deletion(true, delete_status.success()) {
        ScheduledTaskDeletion::Deleted => Ok(()),
        ScheduledTaskDeletion::AlreadyAbsent => unreachable!("task existence was checked first"),
        ScheduledTaskDeletion::Failed => Err(CliError::Control(format!(
            "could not remove current-user Forge autostart task ({delete_status})"
        ))),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ScheduledTaskDeletion {
    AlreadyAbsent,
    Deleted,
    Failed,
}

/// Converts scheduler exit outcomes into idempotent removal semantics without
/// running a real task operation in unit tests.
fn scheduled_task_deletion(task_exists: bool, delete_succeeded: bool) -> ScheduledTaskDeletion {
    match (task_exists, delete_succeeded) {
        (false, _) => ScheduledTaskDeletion::AlreadyAbsent,
        (true, true) => ScheduledTaskDeletion::Deleted,
        (true, false) => ScheduledTaskDeletion::Failed,
    }
}

#[cfg(not(target_os = "windows"))]
fn disable_autostart() -> Result<()> {
    Err(CliError::Unsupported(
        "Forge autostart uses Windows Task Scheduler and is unavailable on this platform".into(),
    ))
}

#[cfg(target_os = "windows")]
fn autostart_enabled() -> Result<bool> {
    let status = hidden_schtasks(
        &["/Query".into(), "/TN".into(), AUTOSTART_TASK_NAME.into()],
        "inspect Forge autostart task",
    )?;
    Ok(status.success())
}

#[cfg(not(target_os = "windows"))]
fn autostart_enabled() -> Result<bool> {
    Err(CliError::Unsupported(
        "Forge autostart uses Windows Task Scheduler and is unavailable on this platform".into(),
    ))
}

#[cfg(target_os = "windows")]
fn hidden_schtasks(
    arguments: &[String],
    context: &'static str,
) -> Result<std::process::ExitStatus> {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    Command::new("schtasks.exe")
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map_err(io(context))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LifecycleOperation {
    Status,
    Stop,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum LifecycleResult {
    Status(LifecycleStatus),
    Stop(LifecycleStopReceipt),
}

struct LifecycleMaterial {
    certificate: CertificateDer<'static>,
    pinned_identity: PinnedIdentity,
    target: LoopbackTarget,
    binding: ReconnectBinding,
    limits: ClientSessionLimits,
}

fn status(layout: &Layout, json: bool) -> Result<()> {
    let manifest = require_installation(layout)?;
    let config = load_lifecycle_instance(layout)?;
    match process::readiness_status(config.readiness_path(), &manifest.forge_executable()) {
        process::ForgeReadinessStatus::Ready(readiness) => {
            let result =
                authenticated_lifecycle(layout, &config, &readiness, LifecycleOperation::Status)?;
            let LifecycleResult::Status(lifecycle) = result else {
                return Err(CliError::LifecycleService {
                    reason: "unexpected lifecycle response",
                });
            };
            print_lifecycle_status(&readiness, &lifecycle, json);
            Ok(())
        }
        process::ForgeReadinessStatus::Missing => {
            if json {
                println!(r#"{{"readiness":"missing"}}"#);
            } else {
                println!("missing");
            }
            Ok(())
        }
        process::ForgeReadinessStatus::Invalid => {
            if json {
                println!(r#"{{"readiness":"invalid"}}"#);
            } else {
                println!("invalid");
            }
            Ok(())
        }
    }
}

fn stop(layout: &Layout, pid: NonZeroU32, if_idle: bool) -> Result<()> {
    if !if_idle {
        return Err(CliError::Unsupported("stop requires --if-idle".to_owned()));
    }

    let manifest = require_installation(layout)?;
    let config = load_lifecycle_instance(layout)?;
    match process::readiness_status(config.readiness_path(), &manifest.forge_executable()) {
        process::ForgeReadinessStatus::Missing => Err(CliError::NotRunning),
        process::ForgeReadinessStatus::Invalid => Err(CliError::LifecycleReadiness {
            reason: "readiness receipt is invalid or stale",
        }),
        process::ForgeReadinessStatus::Ready(readiness) => {
            if readiness.pid() != pid.get() {
                return Err(CliError::LifecycleReadiness {
                    reason: "PID does not match the readiness receipt",
                });
            }
            let result =
                authenticated_lifecycle(layout, &config, &readiness, LifecycleOperation::Stop)?;
            let LifecycleResult::Stop(receipt) = result else {
                return Err(CliError::LifecycleService {
                    reason: "unexpected lifecycle response",
                });
            };
            print_stop_receipt(&receipt);
            Ok(())
        }
    }
}

fn load_lifecycle_instance(layout: &Layout) -> Result<NativeInstanceConfig> {
    match load_native_instance(layout) {
        Ok(config) => Ok(config),
        Err(CliError::MissingInstance) => Err(CliError::MissingInstance),
        Err(_) => Err(CliError::LifecycleService {
            reason: "native instance configuration is unavailable",
        }),
    }
}

fn authenticated_lifecycle(
    layout: &Layout,
    config: &NativeInstanceConfig,
    readiness: &process::ForgeReadiness,
    operation: LifecycleOperation,
) -> Result<LifecycleResult> {
    let material = lifecycle_material(layout, config, readiness)?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| CliError::LifecycleService {
            reason: "create lifecycle runtime",
        })?;
    let store = ReconnectCapabilityStore::from_home(&layout.root)
        .map_err(|error| lifecycle_credential_error(&error))?;
    let attempt = store
        .checkout(material.binding, credentials::RECONNECT_LOCK_TIMEOUT)
        .map_err(|error| lifecycle_credential_error(&error))?;
    runtime.block_on(authenticated_lifecycle_session(
        material, operation, attempt,
    ))
}

fn lifecycle_material(
    layout: &Layout,
    config: &NativeInstanceConfig,
    readiness: &process::ForgeReadiness,
) -> Result<LifecycleMaterial> {
    let identity = credentials::load_existing_client_identity(&layout.root)
        .map_err(|error| lifecycle_credential_error(&error))?;
    if config.credentials_manifest() != identity.paths().manifest_path() {
        return Err(CliError::LifecycleCredentialState {
            reason: "credential manifest does not match the instance",
        });
    }

    let certificate = identity.certificate().clone();
    let pinned_identity = PinnedIdentity::from_certificate(&certificate);
    let expected_pin = pinned_identity.to_hex();
    if readiness.certificate_sha256() != expected_pin
        || readiness.certificate_sha256() != readiness.certificate_sha256().to_ascii_lowercase()
    {
        return Err(CliError::LifecycleReadiness {
            reason: "readiness certificate does not match the client identity",
        });
    }

    let address =
        readiness
            .endpoint()
            .parse::<SocketAddr>()
            .map_err(|_| CliError::LifecycleReadiness {
                reason: "readiness endpoint is invalid",
            })?;
    let target = LoopbackTarget::new(address).map_err(|_| CliError::LifecycleReadiness {
        reason: "readiness endpoint is not exact loopback",
    })?;
    let pid = NonZeroU32::new(readiness.pid()).ok_or(CliError::LifecycleReadiness {
        reason: "readiness PID is zero",
    })?;
    let binding = ReconnectBinding::new(
        config.instance_id(),
        target.addr().port(),
        *pinned_identity.as_bytes(),
        pid,
    )
    .map_err(|error| lifecycle_credential_error(&error))?;
    let listener = config.listener();
    let limits = ClientSessionLimits {
        connect: lifecycle_duration(listener.admission_timeout_ms())?,
        handshake: lifecycle_duration(listener.handshake_timeout_ms())?,
        request: lifecycle_duration(listener.request_timeout_ms())?,
        shutdown: lifecycle_duration(listener.drain_timeout_ms())?,
        admission_budget: usize::try_from(listener.requests_per_connection().get()).map_err(
            |_| CliError::LifecycleService {
                reason: "request admission budget is not representable",
            },
        )?,
    };

    Ok(LifecycleMaterial {
        certificate,
        pinned_identity,
        target,
        binding,
        limits,
    })
}

fn lifecycle_duration(milliseconds: u64) -> Result<Duration> {
    if milliseconds == 0 {
        return Err(CliError::LifecycleService {
            reason: "listener timeout is zero",
        });
    }
    Ok(Duration::from_millis(milliseconds))
}

async fn authenticated_lifecycle_session(
    material: LifecycleMaterial,
    operation: LifecycleOperation,
    mut attempt: ReconnectAttempt,
) -> Result<LifecycleResult> {
    let cancel = CancelHandle::new();
    let capability = match attempt.take_credential() {
        Ok(capability) => capability,
        Err(error) => {
            let primary = lifecycle_credential_error(&error);
            return match attempt.quarantine() {
                Ok(()) => Err(primary),
                Err(custody) => Err(lifecycle_credential_error(&custody)),
            };
        }
    };
    let hello = match lifecycle_hello_with_capability(capability) {
        Ok(hello) => hello,
        Err((failure, capability)) => {
            return match attempt.restore_before_handshake(capability) {
                Ok(_) => Err(failure),
                Err(custody) => Err(lifecycle_credential_error(&custody)),
            };
        }
    };

    let connected = ClientSession::connect(
        material.target,
        material.certificate.clone(),
        material.pinned_identity,
        hello,
        material.limits,
        &cancel,
    )
    .await;
    let (session, welcome) = match connected {
        Ok(connected) => connected,
        Err(error) => {
            let failure = lifecycle_connect_error(&error);
            return match attempt.quarantine() {
                Ok(()) => Err(failure),
                Err(custody) => Err(lifecycle_credential_error(&custody)),
            };
        }
    };
    let reconnect_lease =
        match attempt.publish_next(material.binding, welcome.welcome.reconnect_capability) {
            Ok(lease) => lease,
            Err(error) => {
                let _ = session.shutdown(&cancel).await;
                return Err(lifecycle_credential_error(&error));
            }
        };

    if !session.lifecycle_control_supported() {
        let _ = session.shutdown(&cancel).await;
        drop(reconnect_lease);
        return Err(CliError::UnsupportedLifecycleControl);
    }

    let (request, expected_request_id) = match lifecycle_request(operation) {
        Ok(request) => request,
        Err(error) => {
            let _ = session.shutdown(&cancel).await;
            drop(reconnect_lease);
            return Err(error);
        }
    };
    let (session, resolved) = match session
        .request_acknowledging_response(request, &cancel)
        .await
    {
        Ok(response) => response,
        Err(error) => {
            let quarantine = lifecycle_request_requires_quarantine(&error);
            let failure = lifecycle_request_error(&error);
            if quarantine {
                return match reconnect_lease.quarantine() {
                    Ok(()) => Err(failure),
                    Err(custody) => Err(lifecycle_credential_error(&custody)),
                };
            }
            drop(reconnect_lease);
            return Err(failure);
        }
    };
    let result = classify_lifecycle_response(operation, &expected_request_id, &resolved);
    // The request stage has already settled its terminal outcome. Shutdown is
    // best-effort here; it cannot turn an acknowledged stop into a retryable
    // operation and the session is consumed even if the bounded drain fails.
    let _ = session.shutdown(&cancel).await;
    if lifecycle_response_requires_quarantine(&expected_request_id, &resolved, &result) {
        return match reconnect_lease.quarantine() {
            Ok(()) => result,
            Err(custody) => Err(lifecycle_credential_error(&custody)),
        };
    }
    drop(reconnect_lease);
    result
}

fn lifecycle_hello_with_capability(
    capability: artisan_protocol::ReconnectCapability,
) -> std::result::Result<WireEnvelope, (CliError, artisan_protocol::ReconnectCapability)> {
    let (frame_id, sent_at) = match lifecycle_frame_stamp() {
        Ok(stamp) => stamp,
        Err(error) => return Err((error, capability)),
    };
    let Ok(supported_versions) = VersionOffer::new(vec![1]) else {
        return Err((
            CliError::LifecycleService {
                reason: "build protocol version offer",
            },
            capability,
        ));
    };
    Ok(WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id,
        sent_at,
        body: WireEnvelopeBody::Hello(Hello {
            supported_versions,
            credential: HelloCredential::Reconnect(capability),
            supports_lifecycle_control: true,
        }),
    })
}

fn lifecycle_request(operation: LifecycleOperation) -> Result<(WireEnvelope, RequestId)> {
    let (frame_id, sent_at) = lifecycle_frame_stamp()?;
    let request_id = frame_id
        .to_request_id()
        .map_err(|_| CliError::LifecycleService {
            reason: "build request correlation",
        })?;
    let request = match operation {
        LifecycleOperation::Status => LifecycleRequest::Status,
        LifecycleOperation::Stop => LifecycleRequest::Stop { require_idle: true },
    };
    let envelope = WireEnvelope {
        protocol_version: ProtocolVersion::V1,
        frame_id,
        sent_at,
        body: WireEnvelopeBody::Request(ClientRequest::Lifecycle(request)),
    };
    envelope
        .validate_correlation()
        .map_err(|_| CliError::LifecycleService {
            reason: "validate request correlation",
        })?;
    Ok((envelope, request_id))
}

fn lifecycle_frame_stamp() -> Result<(FrameId, UnixMillis)> {
    let sent_at = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => {
            UnixMillis::from_millis(i64::try_from(duration.as_millis()).map_err(|_| {
                CliError::LifecycleService {
                    reason: "build frame timestamp",
                }
            })?)
        }
        Err(error) => UnixMillis::from_millis(
            i64::try_from(error.duration().as_millis())
                .map_err(|_| CliError::LifecycleService {
                    reason: "build frame timestamp",
                })?
                .saturating_neg(),
        ),
    };
    let sequence = NEXT_LIFECYCLE_FRAME
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            current.checked_add(1)
        })
        .map_err(|_| CliError::LifecycleService {
            reason: "lifecycle frame sequence exhausted",
        })?
        .checked_add(1)
        .ok_or(CliError::LifecycleService {
            reason: "lifecycle frame sequence exhausted",
        })?;
    let text = format!(
        "native-{}-{}-{}",
        std::process::id(),
        sent_at.as_millis(),
        sequence
    );
    let frame_id = FrameId::parse(text).map_err(|_| CliError::LifecycleService {
        reason: "build frame identity",
    })?;
    frame_id
        .to_request_id()
        .map_err(|_| CliError::LifecycleService {
            reason: "build frame correlation",
        })?;
    Ok((frame_id, sent_at))
}

fn classify_lifecycle_response(
    operation: LifecycleOperation,
    expected_request_id: &RequestId,
    resolved: &artisan_transport::ResolvedRequest,
) -> Result<LifecycleResult> {
    if resolved.request_id() != expected_request_id {
        return Err(CliError::LifecycleService {
            reason: "response correlation failed",
        });
    }
    match resolved.outcome() {
        RequestOutcome::Failure(failure) => classify_lifecycle_failure(operation, failure.code),
        RequestOutcome::Response(response) => match (&operation, &response.payload) {
            (
                LifecycleOperation::Status,
                ResponsePayload::Lifecycle(LifecycleResponse::Status(status)),
            ) => {
                if status.validate().is_err() {
                    return Err(CliError::LifecycleService {
                        reason: "lifecycle status was invalid",
                    });
                }
                Ok(LifecycleResult::Status(status.clone()))
            }
            (
                LifecycleOperation::Stop,
                ResponsePayload::Lifecycle(LifecycleResponse::Stop(receipt)),
            ) => {
                classify_stop_receipt(receipt)?;
                Ok(LifecycleResult::Stop(receipt.clone()))
            }
            _ => Err(CliError::LifecycleService {
                reason: "unexpected lifecycle response payload",
            }),
        },
    }
}

fn classify_stop_receipt(receipt: &LifecycleStopReceipt) -> Result<()> {
    if receipt.state != LifecycleState::Draining {
        return Err(CliError::LifecycleService {
            reason: "stop response did not enter draining",
        });
    }
    Ok(())
}

fn classify_lifecycle_failure(
    operation: LifecycleOperation,
    code: ErrorCode,
) -> Result<LifecycleResult> {
    match code {
        ErrorCode::UnsupportedFeature => Err(CliError::UnsupportedLifecycleControl),
        ErrorCode::LifecycleBusy if operation == LifecycleOperation::Stop => {
            Err(CliError::LifecycleBusy)
        }
        _ => Err(CliError::LifecycleService {
            reason: "Forge rejected the lifecycle request",
        }),
    }
}

fn lifecycle_connect_error(error: &ClientSessionError) -> CliError {
    if matches!(error, ClientSessionError::Handshake(_)) {
        CliError::LifecycleAmbiguous
    } else {
        CliError::LifecycleService {
            reason: "lifecycle connection failed",
        }
    }
}

fn lifecycle_request_error(error: &ClientRequestError) -> CliError {
    match error {
        ClientRequestError::UnsupportedFeature => CliError::UnsupportedLifecycleControl,
        ClientRequestError::Exchange(_) => CliError::LifecycleService {
            reason: "lifecycle response exchange failed",
        },
        ClientRequestError::Reply(_) => CliError::LifecycleService {
            reason: "lifecycle response was invalid",
        },
        ClientRequestError::NotARequest { .. }
        | ClientRequestError::VersionMismatch { .. }
        | ClientRequestError::Correlation(_)
        | ClientRequestError::Admission(_) => CliError::LifecycleService {
            reason: "lifecycle request was invalid",
        },
    }
}

fn lifecycle_request_requires_quarantine(error: &ClientRequestError) -> bool {
    matches!(
        error,
        ClientRequestError::Correlation(_)
            | ClientRequestError::Exchange(_)
            | ClientRequestError::Reply(_)
    )
}

fn lifecycle_response_requires_quarantine(
    expected_request_id: &RequestId,
    resolved: &artisan_transport::ResolvedRequest,
    result: &Result<LifecycleResult>,
) -> bool {
    if resolved.request_id() != expected_request_id {
        return true;
    }
    matches!(resolved.outcome(), RequestOutcome::Response(_)) && result.is_err()
}

fn lifecycle_credential_error(error: &ForgeCredentialError) -> CliError {
    match error {
        ForgeCredentialError::CapabilityBusy
        | ForgeCredentialError::ReconnectCapabilityUnavailable
        | ForgeCredentialError::ReconnectBindingMismatch
        | ForgeCredentialError::ReconnectStaleWriter
        | ForgeCredentialError::ReconnectGenerationOverflow
        | ForgeCredentialError::ReconnectInvalidBinding
        | ForgeCredentialError::ReconnectAttemptComplete
        | ForgeCredentialError::ReconnectRecordExists => CliError::LifecycleCustody {
            reason: "reconnect capability custody is unavailable",
        },
        _ => CliError::LifecycleCredentialState {
            reason: "reconnect capability or client identity is unavailable",
        },
    }
}

fn print_lifecycle_status(
    readiness: &process::ForgeReadiness,
    lifecycle: &LifecycleStatus,
    json: bool,
) {
    if json {
        println!(
            "{}",
            serde_json::json!({
                "certificate_sha256": readiness.certificate_sha256(),
                "endpoint": readiness.endpoint(),
                "lifecycle": {
                    "active_work_count": lifecycle.active_work_count,
                    "state": lifecycle_state_name(lifecycle.state),
                },
                "pid": readiness.pid(),
                "readiness": "ready",
                "schema": readiness.schema(),
            })
        );
    } else {
        println!(
            "ready (pid {} at {})",
            readiness.pid(),
            readiness.endpoint()
        );
        println!(
            "lifecycle: {} ({} active work item(s))",
            lifecycle_state_name(lifecycle.state),
            lifecycle.active_work_count
        );
    }
}

fn print_stop_receipt(receipt: &LifecycleStopReceipt) {
    match receipt.disposition {
        LifecycleStopDisposition::Accepted => println!("stop accepted (draining)"),
        LifecycleStopDisposition::Duplicate | LifecycleStopDisposition::AlreadyStopping => {
            println!("stop already in progress (draining)");
        }
    }
}

const fn lifecycle_state_name(state: LifecycleState) -> &'static str {
    match state {
        LifecycleState::Ready => "ready",
        LifecycleState::Busy => "busy",
        LifecycleState::Draining => "draining",
    }
}

fn logs(layout: &Layout, lines: usize, follow: bool) -> Result<()> {
    let (paths, _, _) = instance::load(layout)?;
    let mut file = File::open(&paths.log).map_err(io("open Forge log"))?;
    let size = file.metadata().map_err(io("inspect Forge log"))?.len();
    file.seek(SeekFrom::Start(size.saturating_sub(MAX_LOG_BYTES)))
        .map_err(io("seek Forge log"))?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_LOG_BYTES)
        .read_to_end(&mut bytes)
        .map_err(io("read Forge log"))?;
    let text = String::from_utf8_lossy(&bytes);
    let selected = text
        .lines()
        .rev()
        .take(lines.clamp(1, 10_000))
        .collect::<Vec<_>>();
    for line in selected.into_iter().rev() {
        println!("{line}");
    }
    if follow {
        follow_log(file, size)?;
    }
    Ok(())
}

fn follow_log(mut file: File, mut offset: u64) -> Result<()> {
    loop {
        let size = file.metadata().map_err(io("inspect Forge log"))?.len();
        if size < offset {
            file.seek(SeekFrom::Start(0))
                .map_err(io("seek rotated Forge log"))?;
            offset = 0;
        }
        if size > offset {
            file.seek(SeekFrom::Start(offset))
                .map_err(io("seek Forge log"))?;
            let mut bytes = Vec::new();
            file.by_ref()
                .take((size - offset).min(MAX_FOLLOW_BYTES))
                .read_to_end(&mut bytes)
                .map_err(io("follow Forge log"))?;
            offset += bytes.len() as u64;
            print!("{}", String::from_utf8_lossy(&bytes));
        }
        thread::sleep(Duration::from_millis(200));
    }
}

fn doctor(layout: &Layout, fix: bool, json: bool) -> Result<()> {
    if fix {
        delegate_installer(layout, "repair", false)?;
    }
    let installation = InstallationManifest::load(&layout.manifest);
    let protocol = if installation.is_ok() {
        delegate_installer(layout, "diagnose", false)
    } else {
        Err(CliError::Installation(
            "protocol health is unavailable without a valid installation".to_owned(),
        ))
    };
    let instance_state = instance::load(layout);
    // Payload drift (for example a development build copied over an installed
    // version) is reported, never repaired. Versions installed before payload
    // manifests existed stay honestly unverifiable without failing doctor.
    let payload_health = installation.as_ref().map_or(
        payload::PayloadHealth::Unverifiable,
        |manifest: &InstallationManifest| payload::verify(&manifest.version_root()),
    );
    // Repair never invents a Forge configuration. `ae setup` is the sole
    // explicit creator.
    let healthy = installation.is_ok()
        && protocol.is_ok()
        && instance_state.is_ok()
        && !matches!(payload_health, payload::PayloadHealth::Modified(_));
    if json {
        println!(
            "{}",
            serde_json::json!({
                "healthy": healthy,
                "installation": if installation.is_ok() { "ok" } else { "error" },
                "protocol": if protocol.is_ok() { "ok" } else { "error" },
                "instance": if instance_state.is_ok() { "ok" } else { "missing" },
                "payload": payload_health.as_str(),
                "payload_issues": match &payload_health {
                    payload::PayloadHealth::Modified(issues) => issues.clone(),
                    _ => Vec::new(),
                },
            })
        );
    } else {
        println!(
            "{}: installation",
            if installation.is_ok() { "ok" } else { "error" }
        );
        println!(
            "{}: artisan:// protocol",
            if protocol.is_ok() { "ok" } else { "error" }
        );
        println!(
            "{}: forge instance",
            if instance_state.is_ok() {
                "ok"
            } else {
                "error"
            }
        );
        match &payload_health {
            payload::PayloadHealth::Verified => println!("ok: version payload"),
            payload::PayloadHealth::Modified(issues) => {
                println!("error: version payload modified ({})", issues.join(", "));
            }
            payload::PayloadHealth::Unverifiable => {
                println!("warn: version payload (unverifiable: no payload manifest)");
            }
        }
    }
    if healthy {
        Ok(())
    } else {
        Err(CliError::Installation(
            "doctor found unresolved issues".into(),
        ))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OpenFlow {
    /// Launch the installed Electron editor; it obtains its own handoff.
    Editor,
    /// Launch the operating-system browser with a one-time pairing fragment.
    Browser,
    /// Print a one-time `{endpoint, pair_code}` JSON handoff on stdout.
    Handoff,
}

fn handle_protocol(layout: &Layout, url: &str) -> Result<()> {
    if url != FORGE_START_LAUNCH_URL {
        return Err(CliError::Control(
            "unsupported artisan:// launch request".to_owned(),
        ));
    }
    open(layout, None, OpenFlow::Editor)
}

#[derive(Clone, Debug)]
struct ReadyState {
    readiness: process::ForgeReadiness,
}

fn ready_state(layout: &Layout) -> Result<ReadyState> {
    let deadline = std::time::Instant::now() + FORGE_READY_TIMEOUT;
    start_until(layout, false, deadline)?;
    let manifest = require_installation(layout)?;
    let config = load_native_instance(layout)?;
    match process::readiness_status(config.readiness_path(), &manifest.forge_executable()) {
        process::ForgeReadinessStatus::Ready(readiness) => Ok(ReadyState { readiness }),
        process::ForgeReadinessStatus::Missing | process::ForgeReadinessStatus::Invalid => {
            Err(CliError::ForgeReadinessTimeout)
        }
    }
}

fn start_until(
    layout: &Layout,
    foreground: bool,
    readiness_deadline: std::time::Instant,
) -> Result<process::StartResult> {
    let manifest = require_installation(layout)?;
    let spec = native_launch_spec(layout, &manifest)?;
    payload::require_verified(&manifest.version_root())?;
    process::start_until(&spec, foreground, readiness_deadline)
}

fn forge_http_endpoint(readiness: &process::ForgeReadiness) -> String {
    format!("http://{}", readiness.endpoint())
}

fn mint_pair_code(layout: &Layout, readiness: &process::ForgeReadiness) -> Result<String> {
    let (paths, _, secrets) = instance::load(layout)?;
    let body = http::request(
        &forge_http_endpoint(readiness),
        "/api/pair/request",
        &secrets.auth_token,
        "POST",
    )?;
    let pair: PairResponse = serde_json::from_slice(&body).map_err(|source| CliError::Json {
        path: paths.config,
        source,
    })?;
    Ok(pair.code)
}

fn open(layout: &Layout, origin: Option<&str>, flow: OpenFlow) -> Result<()> {
    // A home without an installation has no editor payload to launch; the
    // paired browser against the (already running) Forge is the only
    // renderer there, so the default flow degrades to it instead of failing.
    let flow = resolved_open_flow(flow, layout.manifest.is_file());
    if matches!(flow, OpenFlow::Browser) && !layout.manifest.is_file() {
        eprintln!("no installation in this Artisan home; opening the paired browser instead");
    }
    if open_flow_requires_ready(flow) {
        let ready = ready_state(layout)?;
        open_ready(layout, origin, flow, &ready)
    } else {
        // The editor starts its own background handoff. Do not wait for Forge
        // here: a cold Forge must never delay a visible editor window.
        launch_editor(layout)
    }
}

fn resolved_open_flow(flow: OpenFlow, has_installation: bool) -> OpenFlow {
    if matches!(flow, OpenFlow::Editor) && !has_installation {
        OpenFlow::Browser
    } else {
        flow
    }
}

fn open_flow_requires_ready(flow: OpenFlow) -> bool {
    !matches!(flow, OpenFlow::Editor)
}

fn open_ready(
    layout: &Layout,
    origin: Option<&str>,
    flow: OpenFlow,
    ready: &ReadyState,
) -> Result<()> {
    match flow {
        OpenFlow::Editor => launch_editor(layout),
        OpenFlow::Browser => {
            let code = mint_pair_code(layout, &ready.readiness)?;
            let endpoint = forge_http_endpoint(&ready.readiness);
            let origin = resolve_browser_origin(origin, &endpoint)?;
            launch_url(&format!("{origin}/#pair={code}"))
        }
        OpenFlow::Handoff => {
            let code = mint_pair_code(layout, &ready.readiness)?;
            // The capability is one-time and short-lived; stdout reaches only
            // the trusted local process that invoked this hidden mode.
            println!("{}", handoff_json(ready, &code));
            Ok(())
        }
    }
}

fn handoff_json(ready: &ReadyState, pair_code: &str) -> serde_json::Value {
    serde_json::json!({
        "endpoint": forge_http_endpoint(&ready.readiness),
        "pair_code": pair_code,
        "version": 1,
    })
}

/// The installed editor renders the bundled frontend itself and performs its
/// own `ae open --handoff` exchange against this home's single Forge, so no
/// capability travels through argv.
fn launch_editor(layout: &Layout) -> Result<()> {
    let manifest = require_installation(layout)?;
    telemetry::load_or_create(layout)?;
    let editor = manifest.editor_executable();
    payload::require_verified(&manifest.version_root())?;
    if !editor.is_file() {
        return Err(CliError::Installation(format!(
            "the Artisan editor is missing at {}; run `ae doctor --fix` or use `ae open --browser`",
            editor.display()
        )));
    }
    let mut command = Command::new(&editor);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    detach_editor(&mut command);
    // The managed layout deliberately ships no `ae` shim inside the editor's
    // own resources, so the editor's handoff would resolve a path that does
    // not exist there. This launcher is the one process that knows where the
    // installation's `ae` actually is, so it says so explicitly.
    if let Some(permanent_ae) = manifest.permanent_ae_path.as_ref() {
        command.env("ARTISAN_AE_COMMAND", permanent_ae);
    }
    let diagnostics_directory = layout.root.join("diagnostics");
    command
        .env("ARTISAN_DIAGNOSTICS_DIR", &diagnostics_directory)
        .env("ARTISAN_TELEMETRY_CONFIG_PATH", telemetry::path(layout));
    if diagnostics_directory.join("profiling-enabled").is_file() {
        command
            .env("ARTISAN_EDITOR_RENDERER_DIAGNOSTICS", "1")
            .env("ARTISAN_EDITOR_TRACE_FREEZES", "1");
    }
    // This CLI itself runs under Node when invoked through the packaged
    // launcher scripts; the editor it starts must never inherit that, or
    // Electron degrades to a bare Node process that exits without a window.
    command.env_remove("ELECTRON_RUN_AS_NODE");
    command.spawn().map_err(io("start Artisan editor"))?;
    Ok(())
}

#[cfg(target_os = "windows")]
const EDITOR_CREATION_FLAGS: u32 = 0x0800_0000 | 0x0000_0008;

#[cfg(target_os = "windows")]
fn detach_editor(command: &mut Command) {
    use std::os::windows::process::CommandExt;

    // `ae open` is a launcher, not the editor's lifetime owner. In particular,
    // the first Electron process must not retain the invoking build's console
    // or pipe lifetime after `ae` itself exits.
    command.creation_flags(EDITOR_CREATION_FLAGS);
}

#[cfg(not(target_os = "windows"))]
fn detach_editor(_: &mut Command) {}

fn resolve_browser_origin(origin: Option<&str>, forge_endpoint: &str) -> Result<String> {
    validate_origin(origin.unwrap_or(forge_endpoint))
}

fn validate_origin(origin: &str) -> Result<String> {
    let authority = origin
        .strip_prefix("http://")
        .and_then(|value| value.strip_suffix('/').or(Some(value)))
        .filter(|value| {
            !value.is_empty()
                && !value.contains(['/', '?', '#', '@'])
                && !value.chars().any(char::is_whitespace)
        })
        .ok_or_else(|| CliError::Control("browser origin is invalid".into()))?;
    let host = if let Some(bracketed) = authority.strip_prefix('[') {
        let (host, suffix) = bracketed
            .split_once(']')
            .ok_or_else(|| CliError::Control("browser origin host is invalid".into()))?;
        if !suffix.is_empty() && (!suffix.starts_with(':') || suffix[1..].parse::<u16>().is_err()) {
            return Err(CliError::Control("browser origin port is invalid".into()));
        }
        host
    } else if let Some((host, port)) = authority.rsplit_once(':') {
        if port.parse::<u16>().is_err() {
            return Err(CliError::Control("browser origin port is invalid".into()));
        }
        host
    } else {
        authority
    };
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host.to_ascii_lowercase().ends_with(".localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    if !loopback {
        return Err(CliError::Control(
            "browser origin must be an uncredentialed loopback HTTP origin".into(),
        ));
    }
    Ok(origin.trim_end_matches('/').to_owned())
}

fn launch_url(url: &str) -> Result<()> {
    #[cfg(target_os = "windows")]
    let status = Command::new("rundll32.exe")
        .args(["url.dll,FileProtocolHandler", url])
        .status();
    #[cfg(target_os = "macos")]
    let status = Command::new("open").arg(url).status();
    #[cfg(all(unix, not(target_os = "macos")))]
    let status = Command::new("xdg-open").arg(url).status();
    let status = status.map_err(io("open browser"))?;
    if status.success() {
        Ok(())
    } else {
        Err(CliError::Control("browser launcher failed".into()))
    }
}

fn delegate_installer(layout: &Layout, operation: &str, remove_data: bool) -> Result<()> {
    let manifest = require_installation(layout)?;
    let bootstrap = manifest.installer_executable();
    if !bootstrap.is_file() {
        return Err(CliError::Installation(format!(
            "installer lifecycle binary is missing at {}; reinstall Artisan",
            bootstrap.display()
        )));
    }
    let mut command = Command::new(bootstrap);
    command
        .arg(operation)
        .arg("--install-root")
        .arg(&manifest.install_root);
    if remove_data {
        command.arg("--remove-data");
    }
    if operation == "diagnose" {
        command.stdout(Stdio::null()).stderr(Stdio::null());
    }
    let status = command.status().map_err(io("run installer lifecycle"))?;
    if status.success() {
        Ok(())
    } else {
        Err(CliError::Installation(format!(
            "{operation} failed with {status}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telemetry_commands_are_explicit_and_reset_requires_confirmation() {
        let analytics = Cli::try_parse_from(["ae", "telemetry", "analytics", "enable"])
            .expect("analytics telemetry command");
        assert!(matches!(
            analytics.command,
            Some(Commands::Telemetry {
                command: TelemetryCommand::Analytics {
                    choice: TelemetryChoice::Enable,
                },
            })
        ));
        assert!(Cli::try_parse_from(["ae", "telemetry", "reset-identity"]).is_err());
        assert!(Cli::try_parse_from(["ae", "telemetry", "reset-identity", "--yes",]).is_ok());
    }

    #[test]
    fn plain_invocation_maps_to_open() {
        let cli = Cli::try_parse_from(["ae"]).unwrap();
        assert!(cli.command.is_none());
    }

    #[test]
    fn open_defaults_to_the_editor_and_keeps_explicit_browser_and_handoff_flows() {
        let default_open = Cli::try_parse_from(["ae", "open"]).unwrap();
        assert!(matches!(
            default_open.command,
            Some(Commands::Open {
                browser: false,
                handoff: false,
                origin: None,
            })
        ));
        let browser = Cli::try_parse_from(["ae", "open", "--browser"]).unwrap();
        assert!(matches!(
            browser.command,
            Some(Commands::Open { browser: true, .. })
        ));
        let handoff = Cli::try_parse_from(["ae", "open", "--handoff"]).unwrap();
        assert!(matches!(
            handoff.command,
            Some(Commands::Open { handoff: true, .. })
        ));
        // The handoff prints a capability for a trusted local caller; it never
        // combines with a browser navigation that would expose it elsewhere.
        assert!(Cli::try_parse_from(["ae", "open", "--handoff", "--browser"]).is_err());
        assert!(
            Cli::try_parse_from(["ae", "open", "--handoff", "--origin", "http://127.0.0.1:1"])
                .is_err()
        );
    }

    #[test]
    fn idle_pid_stop_is_the_only_supported_stop_syntax() {
        let pid_stop = Cli::try_parse_from(["ae", "stop", "--pid", "6172", "--if-idle"]).unwrap();
        assert!(matches!(
            pid_stop.command,
            Some(Commands::Stop {
                pid,
                if_idle: true,
            }) if pid.get() == 6172
        ));
        assert!(Cli::try_parse_from(["ae", "stop"]).is_err());
        assert!(Cli::try_parse_from(["ae", "stop", "--pid", "6172"]).is_err());
        assert!(Cli::try_parse_from(["ae", "stop", "--if-idle"]).is_err());
        assert!(Cli::try_parse_from(["ae", "stop", "--pid", "0", "--if-idle"]).is_err());
        assert!(Cli::try_parse_from(["ae", "stop", "--pid", "6172", "--force"]).is_err());
        assert!(Cli::try_parse_from(["ae", "stop", "--instance-id", "forge-1"]).is_err());
        let disable = Cli::try_parse_from(["ae", "autostart", "--disable"]).unwrap();
        assert!(matches!(
            disable.command,
            Some(Commands::Autostart { disable: true })
        ));
    }

    #[test]
    fn installed_editor_flow_never_requires_forge_readiness_before_launch() {
        assert_eq!(resolved_open_flow(OpenFlow::Editor, true), OpenFlow::Editor);
        assert!(!open_flow_requires_ready(OpenFlow::Editor));
        assert!(open_flow_requires_ready(OpenFlow::Browser));
        assert!(open_flow_requires_ready(OpenFlow::Handoff));
        // A development home has no editor payload, so it deliberately falls
        // back to the browser flow that must pair with a ready Forge.
        assert_eq!(
            resolved_open_flow(OpenFlow::Editor, false),
            OpenFlow::Browser
        );
    }

    #[test]
    fn doctor_keeps_unverifiable_payload_diagnostic_while_launch_admission_rejects_it() {
        let root = tempfile::tempdir().unwrap();
        let health = payload::verify(root.path());

        assert_eq!(health, payload::PayloadHealth::Unverifiable);
        assert_eq!(health.as_str(), "unverifiable");
        assert!(!matches!(health, payload::PayloadHealth::Modified(_)));

        let error = payload::require_verified(root.path()).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Artisan is not installed correctly: active version payload is not verified"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn installed_editor_is_detached_from_the_ae_launcher() {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const DETACHED_PROCESS: u32 = 0x0000_0008;

        assert_eq!(EDITOR_CREATION_FLAGS, CREATE_NO_WINDOW | DETACHED_PROCESS);
    }

    #[test]
    fn restart_remains_explicitly_unsupported() {
        assert!(matches!(
            unsupported_lifecycle_control(),
            Err(CliError::UnsupportedLifecycleControl)
        ));
        assert_eq!(
            CliError::UnsupportedLifecycleControl.to_string(),
            "native Forge lifecycle control is unsupported by this Forge"
        );
    }

    #[test]
    fn lifecycle_requests_are_correlated_and_stop_is_idle_fenced() {
        let (status, status_id) = lifecycle_request(LifecycleOperation::Status).unwrap();
        assert_eq!(status.frame_id.to_request_id().unwrap(), status_id);
        assert!(matches!(
            status.body,
            WireEnvelopeBody::Request(ClientRequest::Lifecycle(LifecycleRequest::Status))
        ));

        let (stop, stop_id) = lifecycle_request(LifecycleOperation::Stop).unwrap();
        assert_eq!(stop.frame_id.to_request_id().unwrap(), stop_id);
        assert_ne!(status.frame_id, stop.frame_id);
        assert!(matches!(
            stop.body,
            WireEnvelopeBody::Request(ClientRequest::Lifecycle(LifecycleRequest::Stop {
                require_idle: true
            }))
        ));
    }

    #[test]
    fn lifecycle_failure_boundaries_are_typed_without_retryable_outcomes() {
        assert_eq!(
            classify_lifecycle_failure(LifecycleOperation::Stop, ErrorCode::LifecycleBusy)
                .unwrap_err()
                .exit_code(),
            5
        );
        assert_eq!(
            classify_lifecycle_failure(LifecycleOperation::Status, ErrorCode::Internal)
                .unwrap_err()
                .exit_code(),
            72
        );
        assert_eq!(
            classify_lifecycle_failure(LifecycleOperation::Status, ErrorCode::UnsupportedFeature)
                .unwrap_err()
                .exit_code(),
            1
        );
        assert_eq!(
            lifecycle_connect_error(&ClientSessionError::DeliveryAlreadyTaken).exit_code(),
            72
        );
    }

    #[test]
    fn lifecycle_handshake_and_request_failures_are_terminal_and_redacted() {
        let handshake_timeout = ClientSessionError::Handshake(DeadlineError::Timeout {
            operation: OperationKind::Handshake,
            limit: Duration::from_millis(10),
        });
        assert_eq!(lifecycle_connect_error(&handshake_timeout).exit_code(), 75);

        let connect_timeout = ClientSessionError::Connect(DeadlineError::Timeout {
            operation: OperationKind::Connect,
            limit: Duration::from_millis(10),
        });
        assert_eq!(lifecycle_connect_error(&connect_timeout).exit_code(), 72);

        let acknowledgement_timeout = ClientRequestError::Exchange(DeadlineError::Timeout {
            operation: OperationKind::Receive,
            limit: Duration::from_millis(10),
        });
        assert!(lifecycle_request_requires_quarantine(
            &acknowledgement_timeout
        ));
        let failure = lifecycle_request_error(&acknowledgement_timeout);
        assert_eq!(failure.exit_code(), 72);
        assert!(!failure.to_string().contains("10ms"));

        let correlation_failure =
            ClientRequestError::Correlation(artisan_domain::IdentifierError::Empty);
        assert!(lifecycle_request_requires_quarantine(&correlation_failure));
    }

    #[test]
    fn lifecycle_hello_advertises_control_with_only_reconnect_credential() {
        let hello =
            lifecycle_hello_with_capability(artisan_protocol::ReconnectCapability::from_bytes(
                [0xa5; artisan_protocol::RECONNECT_CAPABILITY_BYTES],
            ))
            .unwrap();
        let WireEnvelopeBody::Hello(hello) = hello.body else {
            panic!("lifecycle hello body");
        };
        assert!(hello.supports_lifecycle_control);
        assert!(matches!(hello.credential, HelloCredential::Reconnect(_)));
    }

    #[test]
    fn lifecycle_custody_states_keep_missing_malformed_and_stale_fences_distinct() {
        assert_eq!(
            lifecycle_credential_error(&ForgeCredentialError::ReconnectRecordMissing).exit_code(),
            64
        );
        assert_eq!(
            lifecycle_credential_error(&ForgeCredentialError::ReconnectRecordMalformed).exit_code(),
            64
        );
        assert_eq!(
            lifecycle_credential_error(&ForgeCredentialError::CapabilityBusy).exit_code(),
            75
        );
        assert_eq!(
            lifecycle_credential_error(&ForgeCredentialError::ReconnectStaleWriter).exit_code(),
            75
        );
        assert_eq!(
            lifecycle_credential_error(&ForgeCredentialError::ReconnectBindingMismatch).exit_code(),
            75
        );
    }

    #[test]
    fn lifecycle_status_accepts_all_valid_states_and_stop_only_drains() {
        for (state, count) in [
            (LifecycleState::Ready, 0),
            (LifecycleState::Busy, 2),
            (LifecycleState::Draining, 2),
        ] {
            let status = LifecycleStatus::new(state, count).unwrap();
            assert_eq!(
                lifecycle_state_name(state),
                match state {
                    LifecycleState::Ready => "ready",
                    LifecycleState::Busy => "busy",
                    LifecycleState::Draining => "draining",
                }
            );
            assert!(status.validate().is_ok());
        }
        let ready_receipt = LifecycleStopReceipt {
            disposition: LifecycleStopDisposition::Accepted,
            state: LifecycleState::Ready,
        };
        assert_eq!(
            classify_stop_receipt(&ready_receipt)
                .unwrap_err()
                .exit_code(),
            72
        );
        for disposition in [
            LifecycleStopDisposition::Accepted,
            LifecycleStopDisposition::Duplicate,
            LifecycleStopDisposition::AlreadyStopping,
        ] {
            let receipt = LifecycleStopReceipt {
                disposition,
                state: LifecycleState::Draining,
            };
            assert!(classify_stop_receipt(&receipt).is_ok());
        }
    }

    #[test]
    fn handoff_uses_only_validated_non_secret_readiness_data() {
        let readiness = process::ForgeReadiness::new(
            "artisan-forge-ready-v1",
            "127.0.0.1:4317",
            "a".repeat(64),
            42,
        )
        .unwrap();
        let handoff = handoff_json(&ReadyState { readiness }, "pair");
        assert_eq!(handoff["endpoint"], "http://127.0.0.1:4317");
        assert_eq!(handoff["pair_code"], "pair");
        assert!(handoff.get("owned_instance_id").is_none());
    }

    #[test]
    fn scheduled_task_uses_a_fixed_limited_current_user_logon_argv() {
        let action = scheduled_task_action(
            Path::new(r"C:\Program Files\Artisan\ae.exe"),
            StableLauncher::Executable,
            None,
        )
        .unwrap();
        let args = scheduled_task_create_args(&action);
        assert_eq!(
            args,
            [
                "/Create",
                "/TN",
                "Artisan Forge",
                "/TR",
                r#""C:\Program Files\Artisan\ae.exe" start"#,
                "/SC",
                "ONLOGON",
                "/RL",
                "LIMITED",
                "/F",
            ]
        );
    }

    #[test]
    fn scheduled_task_runs_stable_cmd_launchers_through_trusted_cmd() {
        let action = scheduled_task_action(
            Path::new(r"C:\Program Files\Artisan\bin\ae.cmd"),
            StableLauncher::CommandScript,
            Some(PathBuf::from(r"C:\Windows\System32\cmd.exe")),
        )
        .unwrap();
        assert_eq!(
            action,
            r#""C:\Windows\System32\cmd.exe" /d /s /c ""C:\Program Files\Artisan\bin\ae.cmd" start""#
        );
        assert_eq!(
            stable_launcher_kind(Path::new(r"C:\Program Files\Artisan\bin\ae.cmd")),
            Some(StableLauncher::CommandScript)
        );
        assert_eq!(
            stable_launcher_kind(Path::new(r"C:\Program Files\Artisan\bin\ae.bat")),
            Some(StableLauncher::CommandScript)
        );
        assert_eq!(
            stable_launcher_kind(Path::new(r"C:\Program Files\Artisan\bin\ae.ps1")),
            None
        );
        for unsafe_character in ['%', '!', '^', '&', '|', '<', '>', '(', ')'] {
            let path = PathBuf::from(format!(
                r"C:\Program Files\Artisan{unsafe_character}Co\bin\ae.cmd"
            ));
            assert!(
                scheduled_task_action(
                    &path,
                    StableLauncher::CommandScript,
                    Some(PathBuf::from(r"C:\Windows\System32\cmd.exe")),
                )
                .is_err()
            );
        }
    }

    #[test]
    fn scheduled_task_removal_is_idempotent_without_hiding_delete_failures() {
        assert_eq!(
            scheduled_task_deletion(false, false),
            ScheduledTaskDeletion::AlreadyAbsent
        );
        assert_eq!(
            scheduled_task_deletion(true, true),
            ScheduledTaskDeletion::Deleted
        );
        assert_eq!(
            scheduled_task_deletion(true, false),
            ScheduledTaskDeletion::Failed
        );
    }

    #[test]
    fn handoff_wait_budget_covers_the_renderer_cold_start_window() {
        assert_eq!(FORGE_READY_TIMEOUT, Duration::from_secs(30));
    }

    fn explicit_setup_args() -> Vec<String> {
        let (database, custody, readiness) = if cfg!(windows) {
            (
                r"C:\Artisan Street\data\forge.sqlite3",
                r"C:\Artisan Street\custody\forge.lock",
                r"C:\Artisan Street\readiness\forge.json",
            )
        } else {
            (
                "/tmp/Artisan Street/data/forge.sqlite3",
                "/tmp/Artisan Street/custody/forge.lock",
                "/tmp/Artisan Street/readiness/forge.json",
            )
        };
        [
            "ae",
            "setup",
            "--database-path",
            database,
            "--custody-path",
            custody,
            "--readiness-path",
            readiness,
            "--admission-timeout-ms",
            "101",
            "--handshake-timeout-ms",
            "202",
            "--request-timeout-ms",
            "303",
            "--drain-timeout-ms",
            "404",
            "--admission-capacity",
            "3",
            "--requests-per-connection",
            "4",
            "--native-run-claim-lease-ms",
            "505",
            "--native-run-poll-interval-ms",
            "506",
            "--native-run-retry-backoff-ms",
            "507",
            "--native-run-shutdown-budget-ms",
            "508",
            "--native-run-queue-capacity",
            "9",
            "--native-run-max-command-retries",
            "10",
            "--native-run-prompt-delivery",
            "queue",
            "--native-run-stream-after",
            "0",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    }

    fn replace_setup_value(arguments: &mut [String], option: &str, value: &str) {
        let position = arguments
            .iter()
            .position(|argument| argument == option)
            .expect("setup option should exist");
        arguments[position + 1] = value.to_owned();
    }

    #[test]
    fn setup_requires_explicit_native_values_and_preserves_exact_values() {
        let valid = Cli::try_parse_from(explicit_setup_args()).unwrap();
        assert!(matches!(
            valid.command,
            Some(Commands::Setup {
                database_path,
                custody_path,
                readiness_path,
                admission_timeout_ms: 101,
                handshake_timeout_ms: 202,
                request_timeout_ms: 303,
                drain_timeout_ms: 404,
                admission_capacity,
                requests_per_connection,
                native_run_claim_lease_ms: 505,
                native_run_poll_interval_ms: 506,
                native_run_retry_backoff_ms: 507,
                native_run_shutdown_budget_ms: 508,
                native_run_queue_capacity,
                native_run_max_command_retries,
                native_run_prompt_delivery,
                native_run_stream_after: 0,
                autostart: false,
            }) if database_path.is_absolute()
                && custody_path.is_absolute()
                && readiness_path.is_absolute()
                && admission_capacity.get() == 3
                && requests_per_connection.get() == 4
                && native_run_queue_capacity.get() == 9
                && native_run_max_command_retries.get() == 10
                && native_run_prompt_delivery.0 == "queue"
        ));

        assert!(Cli::try_parse_from(["ae", "setup"]).is_err());
        for option in [
            "--admission-timeout-ms",
            "--handshake-timeout-ms",
            "--request-timeout-ms",
            "--drain-timeout-ms",
            "--native-run-claim-lease-ms",
            "--native-run-poll-interval-ms",
            "--native-run-retry-backoff-ms",
            "--native-run-shutdown-budget-ms",
            "--native-run-queue-capacity",
            "--native-run-max-command-retries",
        ] {
            let mut arguments = explicit_setup_args();
            replace_setup_value(&mut arguments, option, "0");
            assert!(
                Cli::try_parse_from(arguments).is_err(),
                "zero argument {option}"
            );
        }
    }

    #[test]
    fn setup_rejects_native_run_boundary_and_legacy_values() {
        assert_eq!(parse_positive_u64(&u64::MAX.to_string()), Ok(u64::MAX));
        assert_eq!(
            parse_native_run_duration_ms(&(u64::MAX / 2).to_string()),
            Ok(u64::MAX / 2)
        );
        assert_eq!(
            parse_native_run_prompt_delivery(&"p".repeat(256))
                .map(NativeRunPromptDelivery::into_string),
            Ok("p".repeat(256))
        );
        assert_eq!(
            parse_native_run_duration_ms("0"),
            Err(String::from(
                "must be positive and fit the native-run duration range"
            ))
        );
        assert_eq!(
            parse_native_run_prompt_delivery("queue").map(NativeRunPromptDelivery::into_string),
            Ok(String::from("queue"))
        );
        for (option, invalid) in [
            ("--native-run-prompt-delivery", ""),
            ("--native-run-prompt-delivery", "line\nbreak"),
            ("--native-run-claim-lease-ms", "not-a-number"),
            ("--native-run-retry-backoff-ms", "-1"),
        ] {
            let mut arguments = explicit_setup_args();
            replace_setup_value(&mut arguments, option, invalid);
            assert!(Cli::try_parse_from(arguments).is_err(), "argument {option}");
        }
        let too_long_prompt = "p".repeat(257);
        let mut arguments = explicit_setup_args();
        replace_setup_value(
            &mut arguments,
            "--native-run-prompt-delivery",
            &too_long_prompt,
        );
        assert!(Cli::try_parse_from(arguments).is_err());
        for option in [
            "--native-run-claim-lease-ms",
            "--native-run-poll-interval-ms",
            "--native-run-retry-backoff-ms",
            "--native-run-shutdown-budget-ms",
        ] {
            let mut arguments = explicit_setup_args();
            replace_setup_value(&mut arguments, option, &(u64::MAX / 2 + 1).to_string());
            assert!(
                Cli::try_parse_from(arguments).is_err(),
                "overflow argument {option}"
            );
        }
        for (option, invalid) in [
            ("--native-run-queue-capacity", "4294967296"),
            ("--native-run-max-command-retries", "4294967296"),
        ] {
            let mut arguments = explicit_setup_args();
            replace_setup_value(&mut arguments, option, invalid);
            assert!(
                Cli::try_parse_from(arguments).is_err(),
                "overflow argument {option}"
            );
        }
        for legacy in [
            "--listen-port",
            "--listen-host",
            "--mode",
            "--data-root",
            "--serve-frontend",
            "--token",
        ] {
            let mut arguments = explicit_setup_args();
            arguments.push(legacy.to_owned());
            arguments.push("legacy".to_owned());
            assert!(
                Cli::try_parse_from(arguments).is_err(),
                "legacy option {legacy}"
            );
        }
    }

    #[test]
    fn native_setup_writes_the_v2_instance_and_missing_config_is_typed() {
        let temporary = tempfile::tempdir().unwrap();
        let layout = Layout {
            manifest: temporary.path().join("installation.json"),
            root: temporary.path().join("Artisan Street"),
        };
        assert!(matches!(
            load_native_instance(&layout),
            Err(CliError::MissingInstance)
        ));

        let values = NativeSetupValues {
            database_path: layout.root.join("data").join("forge.sqlite3"),
            custody_path: layout.root.join("custody").join("forge.lock"),
            readiness_path: layout.root.join("readiness").join("forge.json"),
            listener: NativeListenerConfig::new(
                101,
                202,
                303,
                404,
                NonZeroU32::new(3).unwrap(),
                NonZeroU32::new(4).unwrap(),
            ),
            native_run: NativeRunConfig::new(NativeRunConfigInput {
                claim_lease_ms: 505,
                poll_interval_ms: 506,
                retry_backoff_ms: 507,
                shutdown_budget_ms: 508,
                queue_capacity: 9,
                max_command_retries: 10,
                prompt_delivery: "queue".to_owned(),
                stream_after: 0,
            })
            .unwrap(),
        };
        setup_native(&layout, values).unwrap();

        let config = load_native_instance(&layout).unwrap();
        let credentials = ForgeCredentialPaths::from_home(&layout.root).unwrap();
        assert_eq!(config.credentials_manifest(), credentials.manifest_path());
        assert_eq!(config.listener().admission_timeout_ms(), 101);
        assert_eq!(config.listener().requests_per_connection().get(), 4);
        assert_eq!(config.native_run().claim_lease_ms(), 505);
        assert_eq!(config.native_run().poll_interval_ms(), 506);
        assert_eq!(config.native_run().retry_backoff_ms(), 507);
        assert_eq!(config.native_run().shutdown_budget_ms(), 508);
        assert_eq!(config.native_run().queue_capacity().get(), 9);
        assert_eq!(config.native_run().max_command_retries().get(), 10);
        assert_eq!(config.native_run().prompt_delivery(), "queue");
        assert_eq!(config.native_run().stream_after(), 0);
        assert!(layout.native_instance_path().is_file());

        let instance_id = config.instance_id();
        setup_native(
            &layout,
            NativeSetupValues {
                database_path: layout.root.join("data").join("replacement.sqlite3"),
                custody_path: layout.root.join("custody").join("replacement.lock"),
                readiness_path: layout.root.join("readiness").join("replacement.json"),
                listener: NativeListenerConfig::new(
                    111,
                    222,
                    333,
                    444,
                    NonZeroU32::new(5).unwrap(),
                    NonZeroU32::new(6).unwrap(),
                ),
                native_run: NativeRunConfig::new(NativeRunConfigInput {
                    claim_lease_ms: 555,
                    poll_interval_ms: 556,
                    retry_backoff_ms: 557,
                    shutdown_budget_ms: 558,
                    queue_capacity: 11,
                    max_command_retries: 12,
                    prompt_delivery: "replacement".to_owned(),
                    stream_after: 1,
                })
                .unwrap(),
            },
        )
        .unwrap();
        assert_eq!(
            load_native_instance(&layout).unwrap().instance_id(),
            instance_id
        );

        let instance_path = layout.native_instance_path();
        let malformed = br#"{"schema":"artisan-instance-v2","version":1}"#;
        fs::write(&instance_path, malformed).unwrap();
        let before = fs::read(&instance_path).unwrap();
        let result = setup_native(
            &layout,
            NativeSetupValues {
                database_path: layout.root.join("data").join("refused.sqlite3"),
                custody_path: layout.root.join("custody").join("refused.lock"),
                readiness_path: layout.root.join("readiness").join("refused.json"),
                listener: NativeListenerConfig::new(
                    1,
                    2,
                    3,
                    4,
                    NonZeroU32::new(1).unwrap(),
                    NonZeroU32::new(1).unwrap(),
                ),
                native_run: NativeRunConfig::new(NativeRunConfigInput {
                    claim_lease_ms: 1,
                    poll_interval_ms: 2,
                    retry_backoff_ms: 3,
                    shutdown_budget_ms: 4,
                    queue_capacity: 1,
                    max_command_retries: 1,
                    prompt_delivery: "queue".to_owned(),
                    stream_after: 0,
                })
                .unwrap(),
            },
        );
        assert!(matches!(result, Err(CliError::NativeInstance(_))));
        assert_eq!(fs::read(&instance_path).unwrap(), before);
    }

    #[test]
    fn native_setup_rejects_invalid_explicit_paths_before_provisioning() {
        let temporary = tempfile::tempdir().unwrap();
        let layout = Layout {
            manifest: temporary.path().join("installation.json"),
            root: temporary.path().join("Artisan Street"),
        };
        let values = NativeSetupValues {
            database_path: PathBuf::from("relative.sqlite3"),
            custody_path: layout.root.join("custody").join("forge.lock"),
            readiness_path: layout.root.join("readiness").join("forge.json"),
            listener: NativeListenerConfig::new(
                1,
                2,
                3,
                4,
                NonZeroU32::new(1).unwrap(),
                NonZeroU32::new(1).unwrap(),
            ),
            native_run: NativeRunConfig::new(NativeRunConfigInput {
                claim_lease_ms: 1,
                poll_interval_ms: 2,
                retry_backoff_ms: 3,
                shutdown_budget_ms: 4,
                queue_capacity: 1,
                max_command_retries: 1,
                prompt_delivery: "queue".to_owned(),
                stream_after: 0,
            })
            .unwrap(),
        };
        assert!(matches!(
            setup_native(&layout, values),
            Err(CliError::NativeInstance(_))
        ));
    }

    #[test]
    fn the_profile_flag_no_longer_exists() {
        // One Forge per Artisan home: naming an instance is not a concept.
        for arguments in [
            ["ae", "setup", "--profile", "default"],
            ["ae", "start", "--profile", "default"],
            ["ae", "open", "--profile", "default"],
        ] {
            assert!(Cli::try_parse_from(arguments).is_err());
        }
    }

    #[test]
    fn removal_of_data_is_explicit() {
        let cli = Cli::try_parse_from(["ae", "uninstall"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Uninstall { remove_data: false })
        ));
    }

    #[test]
    fn protocol_command_accepts_only_one_url_argument() {
        let cli =
            Cli::try_parse_from(["ae", "protocol", FORGE_START_LAUNCH_URL]).expect("protocol");
        assert!(matches!(
            cli.command,
            Some(Commands::Protocol { url }) if url == FORGE_START_LAUNCH_URL
        ));
        assert!(
            Cli::try_parse_from(["ae", "protocol", FORGE_START_LAUNCH_URL, "unexpected"]).is_err()
        );
    }

    #[test]
    fn protocol_decoder_rejects_every_non_capability_url() {
        let root = std::env::temp_dir().join("artisan-protocol-test-home");
        let layout = Layout {
            manifest: root.join("installation.json"),
            root,
        };
        for candidate in [
            "artisan://forge/start?command=calc",
            "artisan://forge/start#token",
            "artisan://forge/stop",
            "https://forge/start",
        ] {
            assert!(matches!(
                handle_protocol(&layout, candidate),
                Err(CliError::Control(message)) if message == "unsupported artisan:// launch request"
            ));
        }
    }

    #[test]
    fn project_root_is_not_a_supported_argument() {
        assert!(Cli::try_parse_from(["ae", "setup", "--project-root", "."]).is_err());
    }

    #[test]
    fn parsed_setup_debug_redacts_native_run_prompt_delivery() {
        let canary = "native-run-prompt-delivery-canary";
        let mut arguments = explicit_setup_args();
        replace_setup_value(&mut arguments, "--native-run-prompt-delivery", canary);
        let cli = Cli::try_parse_from(arguments).unwrap();
        let debug = format!("{cli:?}");
        assert!(!debug.contains(canary));
        assert!(debug.contains("NativeRunPromptDelivery"));
        assert!(debug.contains("byte_length:"));
        assert!(debug.contains("category: \"validated\""));
    }

    #[test]
    fn browser_origin_rejects_remote_and_credentialed_urls() {
        assert!(validate_origin("https://example.com").is_err());
        assert!(validate_origin("http://user@localhost").is_err());
        assert!(validate_origin("http://artisan-editor.localhost").is_ok());
        assert!(validate_origin("http://127.0.0.1:4317").is_ok());
    }

    #[test]
    fn browser_origin_defaults_to_the_live_forge_endpoint() {
        assert_eq!(
            resolve_browser_origin(None, "http://127.0.0.1:62244/").expect("Forge endpoint"),
            "http://127.0.0.1:62244"
        );
        assert_eq!(
            resolve_browser_origin(
                Some("http://artisan-editor.localhost"),
                "http://127.0.0.1:62244/"
            )
            .expect("explicit local forwarding origin"),
            "http://artisan-editor.localhost"
        );
    }
}
