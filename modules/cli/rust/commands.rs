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
const FORGE_START_LAUNCH_URL: &str = "artisan://forge/start";

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
    Stop,
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
            if autostart {
                return Err(CliError::Unsupported(
                    "`ae setup --autostart` is not available in the Rust CLI yet".into(),
                ));
            }
            let data_root = data_root.as_deref().map(validate_data_root).transpose()?;
            instance::setup(
                &layout,
                mode.into(),
                listen_port,
                data_root.as_deref(),
                serve_frontend,
            )?;
            delegate_bootstrap(&layout, "repair", false)?;
            println!("Configured Forge");
            Ok(())
        }
        Commands::Start { foreground } => start(&layout, foreground),
        Commands::Stop => stop(&layout),
        Commands::Restart { foreground } => {
            let _ = stop(&layout);
            start(&layout, foreground)
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
        Commands::Update => delegate_bootstrap(&layout, "update", false),
        Commands::Uninstall { remove_data } => {
            match stop(&layout) {
                Ok(()) | Err(CliError::NotRunning | CliError::MissingInstance) => {}
                Err(error) => return Err(error),
            }
            delegate_bootstrap(&layout, "uninstall", remove_data)
        }
    }
}

fn require_installation(layout: &Layout) -> Result<InstallationManifest> {
    InstallationManifest::load(&layout.manifest)
}

fn start(layout: &Layout, foreground: bool) -> Result<()> {
    let manifest = require_installation(layout)?;
    let (paths, config, secrets) = instance::load(layout)?;
    process::start(&manifest, &paths, &config, &secrets, foreground)
}

fn stop(layout: &Layout) -> Result<()> {
    let (paths, _, secrets) = instance::load(layout)?;
    process::stop(&paths, &secrets)
}

fn status(layout: &Layout, json: bool) -> Result<()> {
    let (paths, _, secrets) = instance::load(layout)?;
    let state = instance::read_json::<State>(&paths.state).ok();
    let running = state
        .as_ref()
        .is_some_and(|state| http::healthy(&state.endpoint, &secrets.auth_token));
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
        delegate_bootstrap(layout, "repair", false)?;
    }
    let installation = InstallationManifest::load(&layout.manifest);
    let protocol = if installation.is_ok() {
        delegate_bootstrap(layout, "diagnose", false)
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

fn ready_state(layout: &Layout) -> Result<State> {
    let (paths, _, secrets) = instance::load(layout)?;
    // An already-healthy Forge needs no launch, so opening against one works
    // even in homes without an installation manifest (the repo development
    // Forge runs from `.dist/forge`, started by its own CLI).
    if let Ok(candidate) = instance::read_json::<State>(&paths.state)
        && http::healthy(&candidate.endpoint, &secrets.auth_token)
    {
        return Ok(candidate);
    }
    start(layout, false)?;
    for _ in 0..50 {
        if let Ok(candidate) = instance::read_json::<State>(&paths.state)
            && http::healthy(&candidate.endpoint, &secrets.auth_token)
        {
            return Ok(candidate);
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err(CliError::Control("Forge did not become ready".into()))
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
    let state = ready_state(layout)?;
    // A home without an installation has no editor payload to launch; the
    // paired browser against the (already running) Forge is the only
    // renderer there, so the default flow degrades to it instead of failing.
    let flow = if matches!(flow, OpenFlow::Editor) && !layout.manifest.is_file() {
        eprintln!("no installation in this Artisan home; opening the paired browser instead");
        OpenFlow::Browser
    } else {
        flow
    };
    match flow {
        OpenFlow::Editor => launch_editor(layout),
        OpenFlow::Browser => {
            let code = mint_pair_code(layout, &state)?;
            let origin = resolve_browser_origin(origin, &state.endpoint)?;
            launch_url(&format!("{origin}/#pair={code}"))
        }
        OpenFlow::Handoff => {
            let code = mint_pair_code(layout, &state)?;
            // The capability is one-time and short-lived; stdout reaches only
            // the trusted local process that invoked this hidden mode.
            println!(
                "{}",
                serde_json::json!({
                    "endpoint": state.endpoint,
                    "pair_code": code,
                    "version": 1,
                })
            );
            Ok(())
        }
    }
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
    Command::new(&editor)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(io("start Artisan editor"))?;
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

fn delegate_bootstrap(layout: &Layout, operation: &str, remove_data: bool) -> Result<()> {
    let manifest = require_installation(layout)?;
    let bootstrap = manifest.bootstrap_executable();
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
