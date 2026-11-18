use crate::paths::{self, LocateError};
use crate::sys::{self, BoundedRunError};
use crate::ui::{self, AskError};
use artisan_paths::App;
use std::process::{Command, ExitStatus};
use std::time::Duration;

const LIST_DEADLINE: Duration = Duration::from_secs(20);
const PROBE_DEADLINE: Duration = Duration::from_secs(60);

#[derive(Debug, thiserror::Error)]
pub enum DistrosError {
    #[error("list WSL distributions")]
    Run(#[source] BoundedRunError),
    #[error("wsl.exe --list failed ({status})")]
    Failed { status: ExitStatus },
}

#[derive(Debug, thiserror::Error)]
pub enum StatusInError {
    #[error("run `ae status --json` in {distro}")]
    Run {
        distro: String,
        #[source]
        source: BoundedRunError,
    },
    #[error("`ae status` in {distro} failed ({status}): {stderr}")]
    Failed {
        distro: String,
        status: ExitStatus,
        stderr: String,
    },
    #[error("read `ae status --json` from {distro}")]
    Parse {
        distro: String,
        #[source]
        source: serde_json::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ForgeTargetError {
    #[error("--distro only applies on Windows")]
    DistroOffWindows,
    #[error(transparent)]
    Locate(#[from] LocateError),
    #[error("look for Forge in WSL")]
    Distros(#[source] DistrosError),
    #[error("Forge isn't installed on this PC or in WSL; run `ae install`")]
    NotInstalled,
    #[error(transparent)]
    Ask(#[from] AskError),
}

#[derive(Debug, thiserror::Error)]
#[error("run ae in {distro} (WSL)")]
pub struct ForwardError {
    distro: String,
    #[source]
    source: std::io::Error,
}

pub fn distros() -> Result<Vec<String>, DistrosError> {
    let output = sys::output_within(
        Command::new("wsl.exe").args(["--list", "--quiet"]),
        LIST_DEADLINE,
    )
    .map_err(DistrosError::Run)?;
    if !output.status.success() {
        return Err(DistrosError::Failed {
            status: output.status,
        });
    }
    let units: Vec<u16> = output
        .stdout
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
    Ok(String::from_utf16_lossy(&units)
        .lines()
        .map(|line| {
            line.trim_matches(|c: char| c.is_whitespace() || c == '\0')
                .to_owned()
        })
        .filter(|line| !line.is_empty() && !line.starts_with("docker-desktop"))
        .collect())
}

pub fn ae(distro: &str) -> Command {
    let mut command = Command::new("wsl.exe");
    command.args([
        "-d",
        distro,
        "--exec",
        "sh",
        "-c",
        r#"exec "$HOME/.local/bin/ae" "$@""#,
        "ae",
    ]);
    command
}

pub struct Probe {
    pub distro: String,
    pub report: Result<serde_json::Value, StatusInError>,
}

impl Probe {
    pub fn has_forge(&self) -> bool {
        self.report.as_ref().is_ok_and(|report| {
            report["apps"]
                .as_array()
                .is_some_and(|apps| apps.iter().any(|app| app["app"] == App::Forge.id()))
        })
    }
}

fn status_in(distro: &str) -> Result<serde_json::Value, StatusInError> {
    let output = sys::output_within(ae(distro).args(["status", "--json"]), PROBE_DEADLINE)
        .map_err(|source| StatusInError::Run {
            distro: distro.to_owned(),
            source,
        })?;
    if !output.status.success() {
        return Err(StatusInError::Failed {
            distro: distro.to_owned(),
            status: output.status,
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        });
    }
    serde_json::from_slice(&output.stdout).map_err(|source| StatusInError::Parse {
        distro: distro.to_owned(),
        source,
    })
}

pub fn probe_all() -> Result<Vec<Probe>, DistrosError> {
    Ok(distros()?
        .into_iter()
        .map(|distro| {
            let report = status_in(&distro);
            Probe { distro, report }
        })
        .collect())
}

pub fn distros_with_forge() -> Result<Vec<String>, DistrosError> {
    let mut found = Vec::new();
    for probe in probe_all()? {
        match &probe.report {
            Ok(_) if probe.has_forge() => found.push(probe.distro),
            Ok(_) => {}
            Err(error) => ui::warning(format!(
                "Couldn't check {} (WSL) for Forge: {}",
                probe.distro,
                diagnostics::chain(error)
            )),
        }
    }
    Ok(found)
}

pub fn forge_target(distro: Option<&str>) -> Result<Option<String>, ForgeTargetError> {
    if let Some(distro) = distro {
        if !cfg!(windows) {
            return Err(ForgeTargetError::DistroOffWindows);
        }
        return Ok(Some(distro.to_owned()));
    }
    if !cfg!(windows) || paths::binary(App::Forge)?.is_file() {
        return Ok(None);
    }
    let progress = ui::Progress::spinner("Looking for Forge in WSL");
    let found = distros_with_forge();
    progress.clear();
    let found = found.map_err(ForgeTargetError::Distros)?;
    match found.as_slice() {
        [] => Err(ForgeTargetError::NotInstalled),
        [distro] => Ok(Some(distro.clone())),
        _ if ui::interactive() => {
            let mut select = cliclack::select("Which Forge?");
            for distro in &found {
                select = select.item(distro.clone(), format!("{distro} (WSL)"), "");
            }
            Ok(select.interact().map(Some).map_err(ui::prompt_error)?)
        }
        _ => Err(ui::needs_flag(
            "which WSL distribution's Forge to use",
            &format!("--distro {}", found.join("|")),
        )
        .into()),
    }
}

pub fn forward(distro: &str) -> Result<(), ForwardError> {
    let mut args = Vec::new();
    let mut skip = false;
    for arg in std::env::args_os().skip(1) {
        if std::mem::take(&mut skip) {
            continue;
        }
        if arg == "--distro" {
            skip = true;
        } else if !arg.to_string_lossy().starts_with("--distro=") {
            args.push(arg);
        }
    }
    let status = ae(distro)
        .args(args)
        .status()
        .map_err(|source| ForwardError {
            distro: distro.to_owned(),
            source,
        })?;
    std::process::exit(status.code().unwrap_or(1));
}
