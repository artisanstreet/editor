//! WSL from Windows: which distributions have Forge, and running `ae`
//! inside one. On Windows, Forge commands go to the distribution that has
//! Forge when this PC has none.

use crate::{sys, ui};
use anyhow::{Context, Result, bail};
use artisan_paths::App;
use std::process::Command;

/// The installed distributions, without Docker's.
pub fn distros() -> Result<Vec<String>> {
    let output = sys::quiet(Command::new("wsl.exe").args(["--list", "--quiet"]))
        .stdin(std::process::Stdio::null())
        .output()
        .context("run wsl.exe")?;
    // wsl.exe writes UTF-16LE.
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

/// `ae` inside `distro`, where `ae install` links it.
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

fn has_forge(distro: &str) -> bool {
    sys::quiet(Command::new("wsl.exe").args([
        "-d",
        distro,
        "--exec",
        "sh",
        "-c",
        r#"test -x "${XDG_DATA_HOME:-$HOME/.local/share}/artisan-street/forge/forge""#,
    ]))
    .stdin(std::process::Stdio::null())
    .stdout(std::process::Stdio::null())
    .stderr(std::process::Stdio::null())
    .status()
    .is_ok_and(|status| status.success())
}

/// The distributions with Forge installed (starts stopped ones to look).
pub fn distros_with_forge() -> Vec<String> {
    distros()
        .unwrap_or_default()
        .into_iter()
        .filter(|distro| has_forge(distro))
        .collect()
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
