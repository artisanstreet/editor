use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    num::NonZeroU32,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use clap::{Parser, Subcommand, ValueEnum};

use crate::{
    CliError, Result,
    credentials::{self, ForgeCredentialPaths},
    error::io,
    http::{self, PairResponse},
    instance::{self, NativeInstanceConfig, NativeListenerConfig, State},
    manifest::InstallationManifest,
    paths::Layout,
    payload, process,
    telemetry::{self, Preference},
};

const MAX_LOG_BYTES: u64 = 1024 * 1024;
const MAX_FOLLOW_BYTES: u64 = 64 * 1024;
// A cold installed Forge can take more than 20 seconds to initialize its SEA
// runtime and durable state; leave enough time for the first editor handoff.
const FORGE_READY_TIMEOUT: Duration = Duration::from_secs(30);
const FORGE_READY_PROBE_TIMEOUT: Duration = Duration::from_millis(250);
const FORGE_READY_INTERVAL: Duration = Duration::from_millis(100);
const FORGE_START_LAUNCH_URL: &str = "artisan://forge/start";
const AUTOSTART_TASK_NAME: &str = "Artisan Forge";

#[derive(Debug, Parser)]
#[command(name = "ae", version, about = "Artisan Editor and Forge")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
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
        #[arg(long, required = true)]
        admission_timeout_ms: u64,
        #[arg(long, required = true)]
        handshake_timeout_ms: u64,
        #[arg(long, required = true)]
        request_timeout_ms: u64,
        #[arg(long, required = true)]
        drain_timeout_ms: u64,
        #[arg(long, required = true, value_parser = parse_nonzero_u32)]
        admission_capacity: NonZeroU32,
        #[arg(long, required = true, value_parser = parse_nonzero_u32)]
        requests_per_connection: NonZeroU32,
        #[arg(long)]
        autostart: bool,
    },
    Start {
        #[arg(long)]
        foreground: bool,
    },
    Stop {
        /// Stop only this exact Forge instance. Intended for editor cleanup.
        #[arg(long, hide = true, conflicts_with = "pid")]
        instance_id: Option<String>,
        /// Stop only the authenticated Forge with this process identity.
        /// Intended for installer retirement.
        #[arg(long, hide = true, conflicts_with = "instance_id")]
        pid: Option<u32>,
        /// Refuse shutdown when Forge reports live model work. Intended for
        /// installer retirement before an update is activated.
        #[arg(long, hide = true, requires = "pid")]
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
    /// Inspect or change privacy-preserving observability preferences.
    Telemetry {
        #[command(subcommand)]
        command: TelemetryCommand,
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

pub fn run(cli: Cli) -> Result<()> {
    let layout = Layout::discover()?;
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
        Commands::Stop {
            instance_id,
            pid,
            if_idle,
        } => match pid {
            Some(pid) => stop_pid(&layout, pid, if_idle),
            None => stop(&layout, instance_id.as_deref()),
        },
        Commands::Restart { foreground } => {
            let _ = stop(&layout, None);
            start(&layout, foreground).map(|_| ())
        }
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
        Commands::Telemetry { command } => telemetry_command(&layout, command),
        Commands::Uninstall { remove_data } => {
            match stop(&layout, None) {
                Ok(()) | Err(CliError::NotRunning | CliError::MissingInstance) => {}
                Err(error) => return Err(error),
            }
            disable_autostart_if_supported()?;
            delegate_installer(&layout, "uninstall", remove_data)
        }
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

#[derive(Debug)]
struct NativeSetupValues {
    database_path: PathBuf,
    custody_path: PathBuf,
    readiness_path: PathBuf,
    listener: NativeListenerConfig,
}

fn parse_nonzero_u32(value: &str) -> std::result::Result<NonZeroU32, String> {
    let value = value
        .parse::<u32>()
        .map_err(|_| "must be a positive 32-bit integer".to_owned())?;
    NonZeroU32::new(value).ok_or_else(|| "must be greater than zero".to_owned())
}

fn setup_native(layout: &Layout, values: NativeSetupValues) -> Result<()> {
    let credential_paths = ForgeCredentialPaths::from_home(&layout.root)?;
    let config = NativeInstanceConfig::new(
        values.database_path,
        values.custody_path,
        values.readiness_path,
        credential_paths.manifest_path().to_path_buf(),
        values.listener,
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
    process::start(&spec, foreground)
}

fn stop(layout: &Layout, instance_id: Option<&str>) -> Result<()> {
    let (paths, _, secrets) = instance::load(layout)?;
    process::stop_with_instance_id(&paths, &secrets, instance_id)
}

fn stop_pid(layout: &Layout, pid: u32, if_idle: bool) -> Result<()> {
    let (paths, _, secrets) = instance::load(layout)?;
    if if_idle {
        process::stop_with_pid_if_idle(&paths, &secrets, pid)
    } else {
        process::stop_with_pid(&paths, &secrets, pid)
    }
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

fn disable_autostart_if_supported() -> Result<()> {
    #[cfg(target_os = "windows")]
    return disable_autostart();
    #[cfg(not(target_os = "windows"))]
    Ok(())
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

fn status(layout: &Layout, json: bool) -> Result<()> {
    let (paths, _, secrets) = instance::load(layout)?;
    let running = process::live_state_until(
        &paths,
        &secrets,
        None,
        std::time::Instant::now() + Duration::from_secs(2),
    )?
    .is_some();
    if json {
        println!(
            "{}",
            serde_json::json!({
                "state": if running { "running" } else { "stopped" },
            })
        );
    } else {
        println!("{}", if running { "running" } else { "stopped" });
    }
    Ok(())
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
    state: State,
    owned_instance_id: Option<String>,
}

fn ready_state(layout: &Layout) -> Result<ReadyState> {
    let deadline = std::time::Instant::now() + FORGE_READY_TIMEOUT;
    // An already-healthy Forge needs no launch, so opening against one works
    // even in a home whose installation manifest is unavailable.
    if !layout.manifest.is_file() {
        let (paths, _, secrets) = instance::load(layout)?;
        if let Some(candidate) =
            process::live_state_until(&paths, &secrets, None, probe_deadline(deadline))?
        {
            return Ok(ReadyState {
                state: candidate,
                owned_instance_id: None,
            });
        }
    }
    let start_result = start_until(layout, false, deadline)?;
    let (paths, _, secrets) = instance::load(layout)?;
    while std::time::Instant::now() < deadline {
        if let Some(candidate) =
            process::live_state_until(&paths, &secrets, None, probe_deadline(deadline))?
        {
            return Ok(ReadyState {
                owned_instance_id: owned_instance_id(start_result, &candidate),
                state: candidate,
            });
        }
        sleep_until(deadline, FORGE_READY_INTERVAL);
    }
    Err(CliError::Control("Forge did not become ready".into()))
}

fn start_until(
    layout: &Layout,
    foreground: bool,
    health_deadline: std::time::Instant,
) -> Result<process::StartResult> {
    let manifest = require_installation(layout)?;
    let spec = native_launch_spec(layout, &manifest)?;
    process::start_until(&spec, foreground, health_deadline)
}

fn probe_deadline(deadline: std::time::Instant) -> std::time::Instant {
    deadline.min(std::time::Instant::now() + FORGE_READY_PROBE_TIMEOUT)
}

fn sleep_until(deadline: std::time::Instant, interval: Duration) {
    if let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) {
        thread::sleep(interval.min(remaining));
    }
}

fn mint_pair_code(layout: &Layout, state: &State) -> Result<String> {
    let (paths, _, secrets) = instance::load(layout)?;
    let body = http::request(
        &state.endpoint,
        "/api/pair/request",
        &secrets.auth_token,
        "POST",
    )?;
    let pair: PairResponse = serde_json::from_slice(&body).map_err(|source| CliError::Json {
        path: paths.state,
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
            let code = mint_pair_code(layout, &ready.state)?;
            let origin = resolve_browser_origin(origin, &ready.state.endpoint)?;
            launch_url(&format!("{origin}/#pair={code}"))
        }
        OpenFlow::Handoff => {
            let code = match mint_pair_code(layout, &ready.state) {
                Ok(code) => code,
                Err(error) => {
                    cleanup_failed_handoff(layout, ready);
                    return Err(error);
                }
            };
            // The capability is one-time and short-lived; stdout reaches only
            // the trusted local process that invoked this hidden mode.
            println!("{}", handoff_json(ready, &code));
            Ok(())
        }
    }
}

fn cleanup_failed_handoff(layout: &Layout, ready: &ReadyState) {
    if let Some(instance_id) = handoff_cleanup_instance_id(ready) {
        // Preserve the pairing error: cleanup is best-effort and exact, never
        // an ordinary shutdown that could affect a replacement Forge.
        let _ = stop(layout, Some(instance_id));
    }
}

fn handoff_cleanup_instance_id(ready: &ReadyState) -> Option<&str> {
    ready.owned_instance_id.as_deref()
}

fn owned_instance_id(start_result: process::StartResult, state: &State) -> Option<String> {
    match start_result {
        process::StartResult::Spawned { pid } if pid == state.pid => {
            Some(state.instance_id.clone())
        }
        process::StartResult::AlreadyRunning
        | process::StartResult::Spawned { .. }
        | process::StartResult::ForegroundExited => None,
    }
}

fn handoff_json(ready: &ReadyState, pair_code: &str) -> serde_json::Value {
    let mut handoff = serde_json::json!({
        "endpoint": ready.state.endpoint,
        "pair_code": pair_code,
        "version": 1,
    });
    if let Some(owned_instance_id) = &ready.owned_instance_id {
        handoff["owned_instance_id"] = serde_json::Value::String(owned_instance_id.clone());
    }
    handoff
}

/// The installed editor renders the bundled frontend itself and performs its
/// own `ae open --handoff` exchange against this home's single Forge, so no
/// capability travels through argv.
fn launch_editor(layout: &Layout) -> Result<()> {
    let manifest = require_installation(layout)?;
    telemetry::load_or_create(layout)?;
    let editor = manifest.editor_executable();
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
    fn hidden_exact_stop_and_autostart_commands_parse_without_widening_stop() {
        let exact_stop = Cli::try_parse_from(["ae", "stop", "--instance-id", "forge-1"]).unwrap();
        assert!(matches!(
            exact_stop.command,
            Some(Commands::Stop {
                instance_id: Some(id),
                pid: None,
                if_idle: false,
            }) if id == "forge-1"
        ));
        let pid_stop = Cli::try_parse_from(["ae", "stop", "--pid", "6172", "--if-idle"]).unwrap();
        assert!(matches!(
            pid_stop.command,
            Some(Commands::Stop {
                instance_id: None,
                pid: Some(6172),
                if_idle: true,
            })
        ));
        assert!(
            Cli::try_parse_from(["ae", "stop", "--instance-id", "forge-1", "--pid", "6172",])
                .is_err()
        );
        let ordinary_stop = Cli::try_parse_from(["ae", "stop"]).unwrap();
        assert!(matches!(
            ordinary_stop.command,
            Some(Commands::Stop {
                instance_id: None,
                pid: None,
                if_idle: false,
            })
        ));
        assert!(Cli::try_parse_from(["ae", "stop", "--if-idle"]).is_err());
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

    #[cfg(target_os = "windows")]
    #[test]
    fn installed_editor_is_detached_from_the_ae_launcher() {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const DETACHED_PROCESS: u32 = 0x0000_0008;

        assert_eq!(EDITOR_CREATION_FLAGS, CREATE_NO_WINDOW | DETACHED_PROCESS);
    }

    #[test]
    fn handoff_ownership_is_emitted_only_for_the_spawned_ready_pid() {
        let state = State {
            endpoint: "http://127.0.0.1:4317".into(),
            instance_id: "forge-owned".into(),
            pid: 42,
        };
        assert_eq!(
            owned_instance_id(process::StartResult::Spawned { pid: 42 }, &state),
            Some("forge-owned".into())
        );
        assert_eq!(
            owned_instance_id(process::StartResult::Spawned { pid: 7 }, &state),
            None
        );
        let owned = ReadyState {
            state: state.clone(),
            owned_instance_id: Some("forge-owned".into()),
        };
        assert_eq!(
            handoff_json(&owned, "pair")["owned_instance_id"],
            "forge-owned"
        );
        let existing = ReadyState {
            state,
            owned_instance_id: None,
        };
        assert!(
            handoff_json(&existing, "pair")
                .get("owned_instance_id")
                .is_none()
        );
        assert_eq!(handoff_cleanup_instance_id(&owned), Some("forge-owned"));
        assert_eq!(handoff_cleanup_instance_id(&existing), None);
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
        assert!(FORGE_READY_PROBE_TIMEOUT < FORGE_READY_TIMEOUT);
        assert!(FORGE_READY_INTERVAL < FORGE_READY_TIMEOUT);
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
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    }

    #[test]
    fn setup_requires_explicit_native_values_and_rejects_invalid_values() {
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
                autostart: false,
            }) if database_path.is_absolute()
                && custody_path.is_absolute()
                && readiness_path.is_absolute()
                && admission_capacity.get() == 3
                && requests_per_connection.get() == 4
        ));

        assert!(Cli::try_parse_from(["ae", "setup"]).is_err());
        for (index, invalid) in [(17, "0"), (19, "0"), (17, "not-a-number"), (9, "-1")] {
            let mut arguments = explicit_setup_args();
            arguments[index] = invalid.to_owned();
            assert!(Cli::try_parse_from(arguments).is_err(), "argument {index}");
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
        };
        setup_native(&layout, values).unwrap();

        let config = load_native_instance(&layout).unwrap();
        let credentials = ForgeCredentialPaths::from_home(&layout.root).unwrap();
        assert_eq!(config.credentials_manifest(), credentials.manifest_path());
        assert_eq!(config.listener().admission_timeout_ms(), 101);
        assert_eq!(config.listener().requests_per_connection().get(), 4);
        assert!(layout.native_instance_path().is_file());
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
