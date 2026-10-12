//! The Forge service: a systemd user unit where `ae install forge` could
//! write one, otherwise a detached `ae forge serve` with a pid file and a
//! log file in the data directory (Windows, macOS, WSL without systemd).

use crate::install::platform::spawn_detached;
use crate::install::sys::{Sys, output};
use anyhow::{Context, Result, bail};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

pub const UNIT: &str = "artisan-forge.service";
const LOG_ROTATE_BYTES: u64 = 16 * 1024 * 1024;

pub fn unit_path() -> Result<PathBuf> {
    Ok(crate::paths::xdg_config()?
        .join("systemd")
        .join("user")
        .join(UNIT))
}

fn uses_systemd() -> bool {
    cfg!(target_os = "linux") && unit_path().is_ok_and(|path| path.exists())
}

/// Whether a systemd user manager runs here (not on WSL without systemd).
pub fn systemd_available() -> bool {
    cfg!(target_os = "linux") && output("systemctl", &["--user", "show-environment"]).is_ok()
}

/// The unit `ae install forge` writes. `PATH` is the installing shell's, so
/// engines find git and the user's tools.
pub fn unit_text(ae: &Path, data_dir: Option<&Path>) -> String {
    let path = std::env::var("PATH").unwrap_or_default();
    let data = data_dir
        .map(|dir| format!("Environment=\"ARTISAN_FORGE_DATA={}\"\n", dir.display()))
        .unwrap_or_default();
    format!(
        "[Unit]\nDescription=Artisan Forge\nAfter=network-online.target\n\n\
         [Service]\nType=simple\nExecStart=\"{}\" forge serve\nEnvironment=\"PATH={path}\"\n{data}\
         Restart=on-failure\nRestartSec=5\nTimeoutStopSec=30\n\n[Install]\nWantedBy=default.target\n",
        ae.display()
    )
}

/// Installs and starts Forge as a service. Returns whether it is a systemd
/// unit (else it was started detached, and starts again on `ae forge start`).
pub fn install(sys: Sys, ae: &Path, data_dir: &Path, custom_data_dir: bool) -> Result<bool> {
    if !systemd_available() {
        start(data_dir, sys.dry_run)?;
        return Ok(false);
    }
    sys.write(
        &unit_path()?,
        &unit_text(ae, custom_data_dir.then_some(data_dir)),
    )?;
    sys.run("systemctl", &["--user", "daemon-reload"])?;
    sys.run("systemctl", &["--user", "enable", UNIT])?;
    sys.run("systemctl", &["--user", "restart", UNIT])?;
    Ok(true)
}

pub fn uninstall(sys: Sys, data_dir: &Path) -> Result<()> {
    if uses_systemd() {
        sys.run("systemctl", &["--user", "disable", "--now", UNIT])?;
        sys.remove(&unit_path()?)?;
        sys.run("systemctl", &["--user", "daemon-reload"])
    } else {
        stop(data_dir, sys.dry_run)
    }
}

pub fn start(data_dir: &Path, dry_run: bool) -> Result<()> {
    let sys = Sys { dry_run };
    if uses_systemd() {
        return sys.run("systemctl", &["--user", "start", UNIT]);
    }
    if let Some(pid) = running_pid(data_dir) {
        println!("Forge is already running (pid {pid}).");
        return Ok(());
    }
    let exe = std::env::current_exe().context("find ae")?;
    if dry_run {
        println!(
            "would start {} forge serve (data {})",
            exe.display(),
            data_dir.display()
        );
        return Ok(());
    }
    std::fs::create_dir_all(data_dir).with_context(|| format!("create {}", data_dir.display()))?;
    let log = data_dir.join("forge.log");
    if std::fs::metadata(&log).is_ok_and(|metadata| metadata.len() > LOG_ROTATE_BYTES) {
        let _ = std::fs::rename(&log, data_dir.join("forge.log.1"));
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)
        .with_context(|| format!("open {}", log.display()))?;
    let pid = spawn_detached(
        Command::new(exe)
            .args(["forge", "serve"])
            .env("ARTISAN_FORGE_DATA", data_dir)
            .stdout(file.try_clone()?)
            .stderr(file),
    )?;
    std::fs::write(pid_file(data_dir), pid.to_string()).context("write forge.pid")?;
    println!("Forge started (pid {pid}); log: {}", log.display());
    Ok(())
}

pub fn stop(data_dir: &Path, dry_run: bool) -> Result<()> {
    let sys = Sys { dry_run };
    if uses_systemd() {
        return sys.run("systemctl", &["--user", "stop", UNIT]);
    }
    let Some(pid) = running_pid(data_dir) else {
        println!("Forge is not running.");
        return Ok(());
    };
    let pid = pid.to_string();
    if cfg!(windows) {
        sys.run("taskkill.exe", &["/PID", &pid, "/T", "/F"])?;
    } else {
        sys.run("kill", &["-TERM", &pid])?;
    }
    if !dry_run {
        let _ = std::fs::remove_file(pid_file(data_dir));
    }
    Ok(())
}

pub fn restart(data_dir: &Path, dry_run: bool) -> Result<()> {
    if uses_systemd() {
        return Sys { dry_run }.run("systemctl", &["--user", "restart", UNIT]);
    }
    stop(data_dir, dry_run)?;
    start(data_dir, dry_run)
}

pub fn is_running(data_dir: &Path) -> bool {
    if uses_systemd() {
        output("systemctl", &["--user", "is-active", "--quiet", UNIT]).is_ok()
    } else {
        running_pid(data_dir).is_some()
    }
}

pub fn status(data_dir: &Path) {
    if uses_systemd() {
        let _ = Command::new("systemctl")
            .args(["--user", "status", UNIT, "--no-pager", "--lines=0"])
            .status();
    } else {
        match running_pid(data_dir) {
            Some(pid) => println!(
                "Forge is running (pid {pid}); log: {}",
                data_dir.join("forge.log").display()
            ),
            None => println!("Forge is not running. Start it with `ae forge start`."),
        }
    }
    let address = crate::forge::default_listen();
    println!("Data: {}", data_dir.display());
    println!(
        "{address}: {}",
        if listening(address) {
            "accepting connections"
        } else {
            "nothing listening"
        }
    );
}

pub fn logs(data_dir: &Path, follow: bool) -> Result<()> {
    let mut command = if uses_systemd() {
        let mut command = Command::new("journalctl");
        command.args(["--user", "-u", UNIT, "-n", "200", "--no-pager"]);
        if follow {
            command.arg("-f");
        }
        command
    } else {
        let log = data_dir.join("forge.log");
        if cfg!(windows) {
            let wait = if follow { " -Wait" } else { "" };
            crate::install::sys::powershell(&format!(
                "Get-Content -Tail 200{wait} -Path {}",
                crate::install::sys::ps_quote(&log)
            ))
        } else {
            let mut command = Command::new("tail");
            command.args(["-n", "200"]);
            if follow {
                command.arg("-F");
            }
            command.arg(log);
            command
        }
    };
    let status = command.status().context("show the log")?;
    if !status.success() {
        bail!("showing the log failed ({status})");
    }
    Ok(())
}

fn pid_file(data_dir: &Path) -> PathBuf {
    data_dir.join("forge.pid")
}

fn running_pid(data_dir: &Path) -> Option<u32> {
    let pid: u32 = std::fs::read_to_string(pid_file(data_dir))
        .ok()?
        .trim()
        .parse()
        .ok()?;
    let alive = if cfg!(windows) {
        output(
            "tasklist.exe",
            &["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"],
        )
        .is_ok_and(|out| out.contains(&format!("\"{pid}\"")))
    } else {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    };
    alive.then_some(pid)
}

/// `ae … --on wsl:<distro>`: runs the same command with the `ae` installed
/// in that WSL distribution.
pub fn forward(host: &str) -> Result<()> {
    let Some(distro) = host.strip_prefix("wsl:") else {
        bail!("unknown host {host:?}; use wsl:<distro>");
    };
    let mut args = Vec::new();
    let mut skip = false;
    for arg in std::env::args_os().skip(1) {
        if std::mem::take(&mut skip) {
            continue;
        }
        if arg == "--on" {
            skip = true;
        } else if !arg.to_string_lossy().starts_with("--on=") {
            args.push(arg);
        }
    }
    let status = wsl_ae(distro).args(args).status().context("run wsl.exe")?;
    std::process::exit(status.code().unwrap_or(1));
}

/// `ae` inside a WSL distribution, where `ae install` links it.
pub fn wsl_ae(distro: &str) -> Command {
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

/// Whether something accepts connections on `address` (`ae doctor`).
pub fn listening(address: SocketAddr) -> bool {
    TcpStream::connect_timeout(&address, Duration::from_secs(1)).is_ok()
}
