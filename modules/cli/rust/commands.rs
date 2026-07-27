use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::Command,
    thread,
    time::Duration,
};

use clap::{Parser, Subcommand, ValueEnum};

use crate::{
    CliError, Result,
    error::io,
    http::{self, PairResponse},
    manifest::InstallationManifest,
    paths::Layout,
    process,
    profile::{self, ForgeMode, State},
};

const MAX_LOG_BYTES: u64 = 1024 * 1024;
const MAX_FOLLOW_BYTES: u64 = 64 * 1024;

#[derive(Debug, Parser)]
#[command(name = "ae", version, about = "Artisan Editor and Forge")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Explicitly create or update a Forge profile.
    Setup {
        #[arg(long, default_value = "default")]
        profile: String,
        #[arg(long, default_value_t = 0)]
        listen_port: u16,
        #[arg(long, value_enum, default_value_t = Mode::Local)]
        mode: Mode,
        #[arg(long)]
        data_root: Option<PathBuf>,
        #[arg(long)]
        autostart: bool,
    },
    Start {
        #[arg(long, default_value = "default")]
        profile: String,
        #[arg(long)]
        foreground: bool,
    },
    Stop {
        #[arg(long, default_value = "default")]
        profile: String,
    },
    Restart {
        #[arg(long, default_value = "default")]
        profile: String,
        #[arg(long)]
        foreground: bool,
    },
    Status {
        #[arg(long, default_value = "default")]
        profile: String,
        #[arg(long)]
        json: bool,
    },
    Logs {
        #[arg(long, default_value = "default")]
        profile: String,
        #[arg(long, default_value_t = 200)]
        lines: usize,
        #[arg(long)]
        follow: bool,
    },
    Doctor {
        #[arg(long)]
        fix: bool,
        #[arg(long, default_value = "default")]
        profile: String,
        #[arg(long)]
        json: bool,
    },
    Open {
        #[arg(long, default_value = "default")]
        profile: String,
        #[arg(long)]
        origin: Option<String>,
    },
    Update,
    Uninstall {
        /// Permanently remove Forge profiles, projects, and conversations.
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
        profile: "default".into(),
        origin: None,
    }) {
        Commands::Setup {
            profile,
            listen_port,
            mode,
            data_root,
            autostart,
        } => {
            require_installation(&layout)?;
            if autostart {
                return Err(CliError::Unsupported(
                    "`ae setup --autostart` is not available in the Rust CLI yet".into(),
                ));
            }
            let data_root = data_root.as_deref().map(validate_data_root).transpose()?;
            profile::setup(
                &layout,
                &profile,
                mode.into(),
                listen_port,
                data_root.as_deref(),
            )?;
            println!("Configured Forge profile {profile}");
            Ok(())
        }
        Commands::Start {
            profile,
            foreground,
        } => start(&layout, &profile, foreground),
        Commands::Stop { profile } => stop(&layout, &profile),
        Commands::Restart {
            profile,
            foreground,
        } => {
            let _ = stop(&layout, &profile);
            start(&layout, &profile, foreground)
        }
        Commands::Status { profile, json } => status(&layout, &profile, json),
        Commands::Logs {
            profile,
            lines,
            follow,
        } => logs(&layout, &profile, lines, follow),
        Commands::Doctor { fix, profile, json } => doctor(&layout, &profile, fix, json),
        Commands::Open { profile, origin } => open(&layout, &profile, origin.as_deref()),
        Commands::Update => delegate_bootstrap(&layout, "update", false),
        Commands::Uninstall { remove_data } => {
            stop_all_profiles(&layout)?;
            delegate_bootstrap(&layout, "uninstall", remove_data)
        }
    }
}

fn require_installation(layout: &Layout) -> Result<InstallationManifest> {
    InstallationManifest::load(&layout.manifest)
}

fn start(layout: &Layout, name: &str, foreground: bool) -> Result<()> {
    let manifest = require_installation(layout)?;
    let (paths, profile, secrets) = profile::load(layout, name)?;
    process::start(&manifest, name, &paths, &profile, &secrets, foreground)
}

fn stop(layout: &Layout, name: &str) -> Result<()> {
    let (paths, _, secrets) = profile::load(layout, name)?;
    process::stop(name, &paths, &secrets)
}

fn stop_all_profiles(layout: &Layout) -> Result<()> {
    for name in profile::list_names(layout)? {
        let (paths, _, secrets) = profile::load(layout, &name)?;
        match process::stop(&name, &paths, &secrets) {
            Ok(()) | Err(CliError::NotRunning(_)) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn status(layout: &Layout, name: &str, json: bool) -> Result<()> {
    let (paths, _, secrets) = profile::load(layout, name)?;
    let state = profile::read_json::<State>(&paths.state).ok();
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

fn logs(layout: &Layout, name: &str, lines: usize, follow: bool) -> Result<()> {
    let (paths, _, _) = profile::load(layout, name)?;
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

fn doctor(layout: &Layout, name: &str, fix: bool, json: bool) -> Result<()> {
    if fix {
        delegate_bootstrap(layout, "repair", false)?;
    }
    let installation = InstallationManifest::load(&layout.manifest);
    let profile_state = profile::load(layout, name);
    // Repair never invents a profile. `ae setup` is the sole explicit creator.
    let healthy = installation.is_ok() && profile_state.is_ok();
    if json {
        println!(
            "{}",
            serde_json::json!({
                "healthy": healthy,
                "installation": if installation.is_ok() { "ok" } else { "error" },
                "profile": if profile_state.is_ok() { "ok" } else { "missing" },
            })
        );
    } else {
        println!(
            "{}: installation",
            if installation.is_ok() { "ok" } else { "error" }
        );
        println!(
            "{}: profile {name}",
            if profile_state.is_ok() { "ok" } else { "error" }
        );
    }
    if healthy {
        Ok(())
    } else {
        Err(CliError::Installation(
            "doctor found unresolved issues".into(),
        ))
    }
}

fn open(layout: &Layout, name: &str, origin: Option<&str>) -> Result<()> {
    start(layout, name, false)?;
    let (paths, _, secrets) = profile::load(layout, name)?;
    let mut state = None;
    for _ in 0..50 {
        if let Ok(candidate) = profile::read_json::<State>(&paths.state)
            && http::healthy(&candidate.endpoint, &secrets.auth_token)
        {
            state = Some(candidate);
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    let state = state.ok_or_else(|| CliError::Control("Forge did not become ready".into()))?;
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
    let origin = validate_origin(origin.unwrap_or("http://artisan-editor.localhost"))?;
    launch_url(&format!("{origin}/#pair={}", pair.code))
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
    fn removal_of_data_is_explicit() {
        let cli = Cli::try_parse_from(["ae", "uninstall"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Commands::Uninstall { remove_data: false })
        ));
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
}
