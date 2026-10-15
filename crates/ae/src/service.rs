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

fn uses_systemd(data_dir: &Path) -> bool {
    if !cfg!(target_os = "linux") {
        return false;
    }
    let Some(text) = unit_path()
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
    else {
        return false;
    };
    unit_matches(&text, data_dir)
}

fn unit_matches(text: &str, data_dir: &Path) -> bool {
    text.lines()
        .find_map(|line| {
            let quoted = line.strip_prefix("Environment=")?;
            let field: String = serde_json::from_str(quoted).ok()?;
            let data = field.strip_prefix("ARTISAN_FORGE_DATA=")?;
            let mut path = String::new();
            let mut characters = data.chars();
            while let Some(character) = characters.next() {
                if character == '%' && characters.next() != Some('%') {
                    return None;
                }
                path.push(character);
            }
            Some(PathBuf::from(path))
        })
        .is_some_and(|configured| same_path(&configured, data_dir))
}

fn unit_quote(value: &str) -> String {
    // JSON quoting is the subset of systemd double-quoted strings we emit.
    // Escape its percent specifiers as well; no shell evaluates these fields.
    serde_json::to_string(&value.replace('%', "%%")).expect("strings serialize")
}

fn same_path(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

/// Whether a systemd user manager runs here (not on WSL without systemd).
pub fn systemd_available() -> bool {
    cfg!(target_os = "linux") && output("systemctl", &["--user", "show-environment"]).is_ok()
}

/// The unit `ae install forge` writes. `PATH` is the installing shell's, so
/// engines find git and the user's tools.
pub fn unit_text(ae: &Path, data_dir: &Path) -> String {
    let path = unit_quote(&format!(
        "PATH={}",
        std::env::var("PATH").unwrap_or_default()
    ));
    let data = unit_quote(&format!("ARTISAN_FORGE_DATA={}", data_dir.display()));
    let executable = unit_quote(&ae.to_string_lossy());
    format!(
        "[Unit]\nDescription=Artisan Forge\nAfter=network-online.target\n\n\
         [Service]\nType=simple\nExecStart={executable} forge serve\nEnvironment={path}\nEnvironment={data}\n\
         Restart=on-failure\nRestartSec=5\nTimeoutStopSec=30\n\n[Install]\nWantedBy=default.target\n"
    )
}

/// Installs and starts Forge as a service. Returns whether it is a systemd
/// unit (else it was started detached, and starts again on `ae forge start`).
pub fn install(sys: Sys, ae: &Path, data_dir: &Path) -> Result<bool> {
    if !systemd_available() {
        start(data_dir, sys.dry_run)?;
        return Ok(false);
    }
    let instance_dir = std::path::absolute(data_dir).context("resolve Forge data directory")?;
    sys.write(&unit_path()?, &unit_text(ae, &instance_dir))?;
    sys.run("systemctl", &["--user", "daemon-reload"])?;
    sys.run("systemctl", &["--user", "enable", UNIT])?;
    sys.run("systemctl", &["--user", "restart", UNIT])?;
    Ok(true)
}

pub fn uninstall(sys: Sys, data_dir: &Path) -> Result<()> {
    if uses_systemd(data_dir) {
        sys.run("systemctl", &["--user", "disable", "--now", UNIT])?;
        sys.remove(&unit_path()?)?;
        sys.run("systemctl", &["--user", "daemon-reload"])
    } else {
        stop(data_dir, sys.dry_run)
    }
}

fn configured_listen() -> Result<SocketAddr> {
    match std::env::var("ARTISAN_FORGE_LISTEN") {
        Ok(value) => value.parse().context("invalid ARTISAN_FORGE_LISTEN"),
        Err(std::env::VarError::NotPresent) => Ok(crate::forge::default_listen()),
        Err(error) => Err(error).context("invalid ARTISAN_FORGE_LISTEN"),
    }
}

pub fn start(data_dir: &Path, dry_run: bool) -> Result<()> {
    let sys = Sys { dry_run };
    if uses_systemd(data_dir) {
        return sys.run("systemctl", &["--user", "start", UNIT]);
    }
    if let Some(pid) = running_pid(data_dir) {
        println!("Forge is already running (pid {pid}).");
        return Ok(());
    }
    let address = configured_listen()?;
    let exe = std::env::current_exe().context("find ae")?;
    if dry_run {
        println!(
            "would start {} forge serve (data {})",
            exe.display(),
            data_dir.display()
        );
        return Ok(());
    }
    anyhow::ensure!(
        !listening(address),
        "{address} is already occupied by another service"
    );
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
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        anyhow::ensure!(
            running_pid(data_dir) == Some(pid),
            "Forge exited while starting; log: {}",
            log.display()
        );
        if listening(address) {
            break;
        }
        if std::time::Instant::now() >= deadline {
            stop(data_dir, false)?;
            bail!(
                "Forge did not listen within 15 seconds; log: {}",
                log.display()
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    println!("Forge started (pid {pid}); log: {}", log.display());
    Ok(())
}

pub fn stop(data_dir: &Path, dry_run: bool) -> Result<()> {
    let sys = Sys { dry_run };
    if uses_systemd(data_dir) {
        return sys.run("systemctl", &["--user", "stop", UNIT]);
    }
    let Some(pid) = running_pid(data_dir) else {
        println!("Forge is not running.");
        return Ok(());
    };
    let numeric_pid = pid;
    let pid = pid.to_string();
    if cfg!(windows) {
        sys.run("taskkill.exe", &["/PID", &pid, "/T", "/F"])?;
    } else {
        sys.run("kill", &["-TERM", &pid])?;
    }
    if !dry_run {
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        while running_pid(data_dir) == Some(numeric_pid) {
            if std::time::Instant::now() >= deadline {
                bail!("Forge pid {pid} did not stop within 30 seconds");
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = std::fs::remove_file(pid_file(data_dir));
    }
    Ok(())
}

pub fn restart(data_dir: &Path, dry_run: bool) -> Result<()> {
    if uses_systemd(data_dir) {
        return Sys { dry_run }.run("systemctl", &["--user", "restart", UNIT]);
    }
    stop(data_dir, dry_run)?;
    start(data_dir, dry_run)
}

pub fn is_running(data_dir: &Path) -> bool {
    if uses_systemd(data_dir) {
        output("systemctl", &["--user", "is-active", "--quiet", UNIT]).is_ok()
    } else {
        running_pid(data_dir).is_some()
    }
}

pub fn status(data_dir: &Path) {
    if uses_systemd(data_dir) {
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
    let address = configured_listen().unwrap_or_else(|_| crate::forge::default_listen());
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
    let mut command = if uses_systemd(data_dir) {
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
    let mut system = sysinfo::System::new();
    let process_id = sysinfo::Pid::from_u32(pid);
    system.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::Some(&[process_id]),
        sysinfo::ProcessRefreshKind::new()
            .with_exe(sysinfo::UpdateKind::Always)
            .with_cmd(sysinfo::UpdateKind::Always)
            .with_environ(sysinfo::UpdateKind::Always),
    );
    let process = system.process(process_id)?;
    let expected = std::env::current_exe().ok()?;
    let executable = process.exe()?;
    let executable = executable.to_string_lossy();
    let executable = Path::new(executable.strip_suffix(" (deleted)").unwrap_or(&executable));
    let data = process
        .environ()
        .iter()
        .find_map(|value| value.to_str()?.strip_prefix("ARTISAN_FORGE_DATA="));
    matches_process(executable, &expected, process.cmd(), data, data_dir).then_some(pid)
}

fn matches_process(
    executable: &Path,
    expected: &Path,
    command: &[std::ffi::OsString],
    data: Option<&str>,
    data_dir: &Path,
) -> bool {
    same_path(executable, expected)
        && command
            .windows(2)
            .any(|args| args[0] == "forge" && args[1] == "serve")
        && data.is_some_and(|data| same_path(Path::new(data), data_dir))
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

#[cfg(test)]
mod instance_tests {
    use super::*;
    #[test]
    fn unit_selection_is_explicit_and_round_trips_quoted_paths() {
        let first = Path::new("/tmp/forge first");
        let second = Path::new("/tmp/forge second");
        assert!(unit_matches(&unit_text(Path::new("/bin/ae"), first), first));
        assert!(!unit_matches(
            &unit_text(Path::new("/bin/ae"), first),
            second
        ));
        assert!(!unit_matches(
            "[Service]\nExecStart=/bin/ae forge serve\n",
            first
        ));
        let quoted = Path::new("/tmp/quoted \"percent%h\" forge");
        assert!(unit_matches(
            &unit_text(Path::new("/bin/ae"), quoted),
            quoted
        ));
        assert!(!unit_matches(
            "Environment=\"ARTISAN_FORGE_DATA=/tmp/%h\"",
            Path::new("/tmp/%h")
        ));
    }
    #[test]
    fn a_recycled_pid_must_still_match_executable_role_and_instance() {
        let expected = Path::new("/bin/ae");
        let data = Path::new("/tmp/selected-forge");
        let command: Vec<_> = ["ae", "forge", "serve"]
            .map(std::ffi::OsString::from)
            .into();
        assert!(matches_process(
            expected,
            expected,
            &command,
            Some("/tmp/selected-forge"),
            data
        ));
        assert!(!matches_process(
            Path::new("/bin/something-else"),
            expected,
            &command,
            Some("/tmp/selected-forge"),
            data
        ));
        assert!(!matches_process(
            expected,
            expected,
            &command,
            Some("/tmp/other-forge"),
            data
        ));
        assert!(!matches_process(
            expected,
            expected,
            &["ae".into(), "update".into()],
            Some("/tmp/selected-forge"),
            data
        ));
    }
}
