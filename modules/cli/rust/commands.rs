use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use clap::{Parser, Subcommand, ValueEnum};

use crate::{
    CliError, Result,
    error::io,
    http::{self, PairResponse},
    instance::{self, ForgeMode, State},
    manifest::InstallationManifest,
    paths::Layout,
    payload, process,
};

const MAX_LOG_BYTES: u64 = 1024 * 1024;
const MAX_FOLLOW_BYTES: u64 = 64 * 1024;
const FORGE_READY_TIMEOUT: Duration = Duration::from_secs(15);
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
        #[arg(long, default_value_t = 0)]
        listen_port: u16,
        #[arg(long, value_enum, default_value_t = Mode::Local)]
        mode: Mode,
        #[arg(long)]
        data_root: Option<PathBuf>,
        #[arg(long)]
        autostart: bool,
        /// Serve the bundled web frontend from this Forge (development homes
        /// only; installed homes render through the editor).
        #[arg(long)]
        serve_frontend: bool,
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
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
pub enum Mode {
    #[default]
    Local,
    Headless,
}

impl From<Mode> for ForgeMode {
    fn from(value: Mode) -> Self {
        match value {
            Mode::Local => Self::Local,
            Mode::Headless => Self::Headless,
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
            listen_port,
            mode,
            data_root,
            autostart,
            serve_frontend,
        } => {
            require_installation(&layout)?;
            let data_root = data_root.as_deref().map(validate_data_root).transpose()?;
            instance::setup(
                &layout,
                mode.into(),
                listen_port,
                data_root.as_deref(),
                serve_frontend,
            )?;
            delegate_installer(&layout, "repair", false)?;
            if autostart {
                enable_autostart(&layout)?;
            }
            println!("Configured Forge");
            Ok(())
        }
        Commands::Start { foreground } => start(&layout, foreground).map(|_| ()),
        Commands::Stop { instance_id, pid } => match pid {
            Some(pid) => stop_pid(&layout, pid),
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

fn require_installation(layout: &Layout) -> Result<InstallationManifest> {
    InstallationManifest::load(&layout.manifest)
}

fn start(layout: &Layout, foreground: bool) -> Result<process::StartResult> {
    let manifest = require_installation(layout)?;
    let (paths, config, secrets) = instance::load(layout)?;
    process::start(&manifest, &paths, &config, &secrets, foreground)
}

fn stop(layout: &Layout, instance_id: Option<&str>) -> Result<()> {
    let (paths, _, secrets) = instance::load(layout)?;
    process::stop_with_instance_id(&paths, &secrets, instance_id)
}

fn stop_pid(layout: &Layout, pid: u32) -> Result<()> {
    let (paths, _, secrets) = instance::load(layout)?;
    process::stop_with_pid(&paths, &secrets, pid)
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
    let (paths, _, secrets) = instance::load(layout)?;
    // An already-healthy Forge needs no launch, so opening against one works
    // even in homes without an installation manifest (the repo development
    // Forge runs from `.dist/forge`, started by its own CLI).
    if let Some(candidate) =
        process::live_state_until(&paths, &secrets, None, probe_deadline(deadline))?
    {
        return Ok(ReadyState {
            state: candidate,
            owned_instance_id: None,
        });
    }
    let start_result = start_until(layout, false, deadline)?;
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
    let (paths, config, secrets) = instance::load(layout)?;
    process::start_until(
        &manifest,
        &paths,
        &config,
        &secrets,
        foreground,
        health_deadline,
    )
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
    // The managed layout deliberately ships no `ae` shim inside the editor's
    // own resources, so the editor's handoff would resolve a path that does
    // not exist there. This launcher is the one process that knows where the
    // installation's `ae` actually is, so it says so explicitly.
    if let Some(permanent_ae) = manifest.permanent_ae_path.as_ref() {
        command.env("ARTISAN_AE_COMMAND", permanent_ae);
    }
    // This CLI itself runs under Node when invoked through the packaged
    // launcher scripts; the editor it starts must never inherit that, or
    // Electron degrades to a bare Node process that exits without a window.
    command.env_remove("ELECTRON_RUN_AS_NODE");
    command.spawn().map_err(io("start Artisan editor"))?;
    Ok(())
}

fn resolve_browser_origin(origin: Option<&str>, forge_endpoint: &str) -> Result<String> {
    validate_origin(origin.unwrap_or(forge_endpoint))
}

fn validate_data_root(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() || path.parent().is_none() {
        return Err(CliError::UnsafePath(path.to_path_buf()));
    }
    Ok(path.to_path_buf())
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
            }) if id == "forge-1"
        ));
        let pid_stop = Cli::try_parse_from(["ae", "stop", "--pid", "6172"]).unwrap();
        assert!(matches!(
            pid_stop.command,
            Some(Commands::Stop {
                instance_id: None,
                pid: Some(6172),
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
            })
        ));
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
        assert_eq!(FORGE_READY_TIMEOUT, Duration::from_secs(15));
        assert!(FORGE_READY_PROBE_TIMEOUT < FORGE_READY_TIMEOUT);
        assert!(FORGE_READY_INTERVAL < FORGE_READY_TIMEOUT);
    }

    #[test]
    fn setup_keeps_static_hosting_an_explicit_opt_in() {
        let default_setup = Cli::try_parse_from(["ae", "setup"]).unwrap();
        assert!(matches!(
            default_setup.command,
            Some(Commands::Setup {
                serve_frontend: false,
                ..
            })
        ));
        let dev_setup = Cli::try_parse_from(["ae", "setup", "--serve-frontend"]).unwrap();
        assert!(matches!(
            dev_setup.command,
            Some(Commands::Setup {
                serve_frontend: true,
                ..
            })
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
    fn setup_accepts_only_absolute_data_roots() {
        assert!(validate_data_root(Path::new("relative")).is_err());
        let absolute = if cfg!(windows) {
            Path::new(r"C:\ArtisanData")
        } else {
            Path::new("/tmp/artisan")
        };
        assert!(validate_data_root(absolute).is_ok());
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
