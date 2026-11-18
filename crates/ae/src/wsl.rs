use crate::{sys, ui};
use anyhow::{Context, Result, bail};
use artisan_paths::App;
use std::process::Command;
use std::time::Duration;

const LIST_DEADLINE: Duration = Duration::from_secs(20);
const PROBE_DEADLINE: Duration = Duration::from_secs(60);

pub fn distros() -> Result<Vec<String>> {
    let output = sys::output_within(
        Command::new("wsl.exe").args(["--list", "--quiet"]),
        LIST_DEADLINE,
    )?;
    anyhow::ensure!(
        output.status.success(),
        "wsl.exe --list failed ({})",
        output.status
    );
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
    pub report: Result<serde_json::Value>,
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

fn status_in(distro: &str) -> Result<serde_json::Value> {
    let output = sys::output_within(ae(distro).args(["status", "--json"]), PROBE_DEADLINE)?;
    anyhow::ensure!(
        output.status.success(),
        "`ae status` in {distro} failed ({}): {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    );
    serde_json::from_slice(&output.stdout)
        .with_context(|| format!("read `ae status --json` from {distro}"))
}

pub fn probe_all() -> Result<Vec<Probe>> {
    Ok(distros()?
        .into_iter()
        .map(|distro| {
            let report = status_in(&distro);
            Probe { distro, report }
        })
        .collect())
}

pub fn distros_with_forge() -> Result<Vec<String>> {
    let mut found = Vec::new();
    for probe in probe_all()? {
        match &probe.report {
            Ok(_) if probe.has_forge() => found.push(probe.distro),
            Ok(_) => {}
            Err(error) => ui::warning(format!(
                "Couldn't check {} (WSL) for Forge: {error:#}",
                probe.distro
            )),
        }
    }
    Ok(found)
}

/// Where a Forge command runs: `None` for this machine, else the WSL
/// distribution (on Windows, when this PC has no Forge).
pub fn forge_target(distro: Option<&str>) -> Result<Option<String>> {
    if let Some(distro) = distro {
        anyhow::ensure!(cfg!(windows), "--distro only applies on Windows");
        return Ok(Some(distro.to_owned()));
    }
    if !cfg!(windows) || App::Forge.binary()?.is_file() {
        return Ok(None);
    }
    let progress = ui::Progress::spinner("Looking for Forge in WSL");
    let found = distros_with_forge();
    progress.clear();
    let found = found?;
    match found.as_slice() {
        [] => bail!("Forge isn't installed on this PC or in WSL; run `ae install`"),
        [distro] => Ok(Some(distro.clone())),
        _ if ui::interactive() => {
            let mut select = cliclack::select("Which Forge?");
            for distro in &found {
                select = select.item(distro.clone(), format!("{distro} (WSL)"), "");
            }
            select.interact().map(Some).map_err(ui::prompt_error)
        }
        _ => Err(ui::needs_flag(
            "which WSL distribution's Forge to use",
            &format!("--distro {}", found.join("|")),
        )),
    }
}

/// Runs this same command with the `ae` in `distro` (minus `--distro`) and
/// exits with its status.
pub fn forward(distro: &str) -> Result<()> {
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
    let status = ae(distro).args(args).status().context("run wsl.exe")?;
    std::process::exit(status.code().unwrap_or(1));
}
