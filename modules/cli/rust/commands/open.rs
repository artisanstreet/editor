use std::process::{Command, Stdio};

use crate::{
    CliError, Result,
    error::io,
    http::{self, PairResponse},
    instance,
    paths::Layout,
    payload, process, telemetry,
};

use super::{
    FORGE_READY_TIMEOUT, FORGE_START_LAUNCH_URL, load_native_instance, native_launch_spec,
    require_launchable_installation,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum OpenFlow {
    /// Launch the installed Electron editor; it obtains its own handoff.
    Editor,
    /// Launch the operating-system browser with a one-time pairing fragment.
    Browser,
    /// Print a one-time `{endpoint, pair_code}` JSON handoff on stdout.
    Handoff,
}

pub(super) fn handle_protocol(layout: &Layout, url: &str) -> Result<()> {
    if url != FORGE_START_LAUNCH_URL {
        return Err(CliError::Control(
            "unsupported artisan:// launch request".to_owned(),
        ));
    }
    open(layout, None, OpenFlow::Editor)
}

#[derive(Clone, Debug)]
pub(super) struct ReadyState {
    pub(super) readiness: process::ForgeReadiness,
}

pub(super) fn ready_state(layout: &Layout) -> Result<ReadyState> {
    require_launchable_installation(layout)?;
    let deadline = std::time::Instant::now() + FORGE_READY_TIMEOUT;
    start_until(layout, false, deadline)?;
    let manifest = require_launchable_installation(layout)?;
    let config = load_native_instance(layout)?;
    match process::readiness_status(config.readiness_path(), &manifest.forge_executable()) {
        process::ForgeReadinessStatus::Ready(readiness) => Ok(ReadyState { readiness }),
        process::ForgeReadinessStatus::Missing | process::ForgeReadinessStatus::Invalid => {
            Err(CliError::ForgeReadinessTimeout)
        }
    }
}

pub(super) fn start_until(
    layout: &Layout,
    foreground: bool,
    readiness_deadline: std::time::Instant,
) -> Result<process::StartResult> {
    let manifest = require_launchable_installation(layout)?;
    payload::require_verified(&manifest.version_root())?;
    let spec = native_launch_spec(layout)?;
    process::start_until(&spec, foreground, readiness_deadline)
}

fn forge_http_endpoint(readiness: &process::ForgeReadiness) -> String {
    format!("http://{}", readiness.endpoint())
}

pub(super) fn mint_pair_code(
    layout: &Layout,
    readiness: &process::ForgeReadiness,
) -> Result<String> {
    require_launchable_installation(layout)?;
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

pub(super) fn open(layout: &Layout, origin: Option<&str>, flow: OpenFlow) -> Result<()> {
    require_launchable_installation(layout)?;
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

pub(super) fn resolved_open_flow(flow: OpenFlow, has_installation: bool) -> OpenFlow {
    if matches!(flow, OpenFlow::Editor) && !has_installation {
        OpenFlow::Browser
    } else {
        flow
    }
}

pub(super) fn open_flow_requires_ready(flow: OpenFlow) -> bool {
    !matches!(flow, OpenFlow::Editor)
}

pub(super) fn open_ready(
    layout: &Layout,
    origin: Option<&str>,
    flow: OpenFlow,
    ready: &ReadyState,
) -> Result<()> {
    match flow {
        OpenFlow::Editor => launch_editor(layout),
        OpenFlow::Browser => {
            require_launchable_installation(layout)?;
            let code = mint_pair_code(layout, &ready.readiness)?;
            let endpoint = forge_http_endpoint(&ready.readiness);
            let origin = resolve_browser_origin(origin, &endpoint)?;
            launch_url(&format!("{origin}/#pair={code}"))
        }
        OpenFlow::Handoff => {
            require_launchable_installation(layout)?;
            let code = mint_pair_code(layout, &ready.readiness)?;
            // The capability is one-time and short-lived; stdout reaches only
            // the trusted local process that invoked this hidden mode.
            println!("{}", handoff_json(ready, &code));
            Ok(())
        }
    }
}

pub(super) fn handoff_json(ready: &ReadyState, pair_code: &str) -> serde_json::Value {
    serde_json::json!({
        "endpoint": forge_http_endpoint(&ready.readiness),
        "pair_code": pair_code,
        "version": 1,
    })
}

/// The installed editor renders the bundled frontend itself and performs its
/// own `ae open --handoff` exchange against this home's single Forge, so no
/// capability travels through argv.
pub(super) fn launch_editor(layout: &Layout) -> Result<()> {
    let manifest = require_launchable_installation(layout)?;
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
pub(super) const EDITOR_CREATION_FLAGS: u32 = 0x0800_0000 | 0x0000_0008;

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

pub(super) fn resolve_browser_origin(origin: Option<&str>, forge_endpoint: &str) -> Result<String> {
    validate_origin(origin.unwrap_or(forge_endpoint))
}

pub(super) fn validate_origin(origin: &str) -> Result<String> {
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
