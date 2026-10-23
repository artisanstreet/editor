//! The Forge service. On Linux with systemd: a user unit running `forge`.
//! Elsewhere (Windows, macOS, WSL without systemd): `forge` started
//! detached, with a pid file and log in its data folder; on Windows a Run
//! entry starts it at sign-in.

use crate::sys;
use anyhow::{Context, Result, bail};
use artisan_paths::App;
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

pub const UNIT: &str = "artisan-forge.service";
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "Artisan Forge";
const LOG_ROTATE_BYTES: u64 = 16 * 1024 * 1024;

fn data() -> Result<PathBuf> {
    App::Forge.data().context("find Forge's data folder")
}

/// Where Forge listens.
pub fn address() -> SocketAddr {
    data()
        .and_then(|data| forge_http::configured_listen(&data))
        .unwrap_or_else(|_| SocketAddr::from(([127, 0, 0, 1], forge_http::DEFAULT_PORT)))
}

pub fn log_file() -> Result<PathBuf> {
    Ok(data()?.join("forge.log"))
}

/// Where to read Forge's log.
pub fn log_hint() -> String {
    if uses_systemd() {
        format!("journalctl --user -u {UNIT}")
    } else {
        log_file().map_or_else(|_| "forge.log".to_owned(), |log| log.display().to_string())
    }
}

pub fn unit_path() -> Result<PathBuf> {
    Ok(artisan_paths::xdg_config()?.join("systemd/user").join(UNIT))
}

fn systemd_available() -> bool {
    cfg!(target_os = "linux") && sys::output("systemctl", &["--user", "show-environment"]).is_ok()
}

/// Whether our unit is installed (one running this `forge`, not an older
/// Artisan's).
fn uses_systemd() -> bool {
    cfg!(target_os = "linux")
        && unit_path()
            .ok()
            .and_then(|path| std::fs::read_to_string(path).ok())
            .zip(App::Forge.binary().ok())
            .is_some_and(|(text, forge)| unit_runs(&text, &forge))
        && systemd_available()
}

fn unit_runs(text: &str, forge: &Path) -> bool {
    text.lines()
        .find_map(|line| line.strip_prefix("ExecStart="))
        .is_some_and(|exec| exec == unit_quote(&forge.to_string_lossy()))
}

fn unit_quote(value: &str) -> String {
    // JSON quoting is the subset of systemd double-quoted strings we emit;
    // percent specifiers are escaped too. A string always serializes.
    serde_json::to_string(&value.replace('%', "%%")).unwrap_or_default()
}

/// The unit. `PATH` is the installing shell's, so engines find git and the
/// user's tools.
///
/// glibc gives every thread that allocates its own malloc arena, and the
/// runtime's workers free what other workers allocated, so arenas fragment
/// and never shrink: Forge held ~85 MB of heap around ~6 MB of live data
/// after an hour and a half. Two arenas are plenty for its load.
fn unit_text(forge: &Path) -> String {
    let path = unit_quote(&format!(
        "PATH={}",
        std::env::var("PATH").unwrap_or_default()
    ));
    format!(
        "[Unit]\nDescription=Artisan Forge\nAfter=network-online.target\n\n\
         [Service]\nType=simple\nExecStart={}\nEnvironment={path}\n\
         Environment=GLIBC_TUNABLES=glibc.malloc.arena_max=2\n\
         Restart=on-failure\nRestartSec=5\nTimeoutStopSec=30\n\n[Install]\nWantedBy=default.target\n",
        unit_quote(&forge.to_string_lossy())
    )
}

/// Makes Forge start at login: the systemd unit, or the Windows Run entry.
/// Returns how it starts, for the summary.
pub fn enable_autostart() -> Result<&'static str> {
    let forge = App::Forge.binary()?;
    if systemd_available() {
        sys::write(&unit_path()?, &unit_text(&forge))?;
        sys::run("systemctl", &["--user", "daemon-reload"])?;
        sys::run("systemctl", &["--user", "enable", UNIT])?;
        Ok("starts at login (systemd)")
    } else if cfg!(windows) {
        let command = format!("\"{}\"", forge.display());
        sys::run(
            "reg.exe",
            &["add", RUN_KEY, "/v", RUN_VALUE, "/d", &command, "/f"],
        )?;
        Ok("starts at sign-in")
    } else if cfg!(target_os = "linux") {
        Ok("start it with `ae forge start` (systemd isn't running here)")
    } else {
        Ok("start it with `ae forge start`")
    }
}

pub fn disable_autostart() -> Result<()> {
    if uses_systemd() {
        sys::run("systemctl", &["--user", "disable", UNIT])?;
        sys::remove(&unit_path()?)?;
        sys::run("systemctl", &["--user", "daemon-reload"])?;
    }
    if cfg!(windows) && sys::output("reg.exe", &["query", RUN_KEY, "/v", RUN_VALUE]).is_ok() {
        sys::run("reg.exe", &["delete", RUN_KEY, "/v", RUN_VALUE, "/f"])?;
    }
    Ok(())
}

/// Starts Forge and waits until it accepts connections. Returns its pid
/// when it was started detached.
pub fn start() -> Result<Option<u32>> {
    let address = address();
    if uses_systemd() {
        sys::run("systemctl", &["--user", "start", UNIT])?;
        wait_listening(address, || {
            sys::output("systemctl", &["--user", "is-active", "--quiet", UNIT]).is_ok()
        })?;
        return Ok(None);
    }
    if let Some(pid) = running_pid() {
        bail!("Forge is already running (pid {pid})");
    }
    let forge = App::Forge.binary()?;
    anyhow::ensure!(
        forge.is_file(),
        "Forge isn't installed here; run `ae install`"
    );
    anyhow::ensure!(
        !listening(address),
        "{address} is already occupied by another service"
    );
    let data = data()?;
    std::fs::create_dir_all(&data).with_context(|| format!("create {}", data.display()))?;
    let log = log_file()?;
    if std::fs::metadata(&log).is_ok_and(|metadata| metadata.len() > LOG_ROTATE_BYTES) {
        std::fs::rename(&log, data.join("forge.log.1")).ok();
    }
    let pid = sys::spawn_detached(
        Command::new(&forge)
            // A Nix shell's libraries would reach the engines' tools.
            .env_remove("LD_LIBRARY_PATH")
            .arg("--log")
            .arg(&log)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null()),
    )?;
    std::fs::write(pid_file()?, pid.to_string()).context("write forge.pid")?;
    if let Err(error) = wait_listening(address, || running_pid() == Some(pid)) {
        // Best effort: the start failure is the error worth reporting.
        stop().ok();

        return Err(error);
    }
    Ok(Some(pid))
}

/// Waits for as long as Forge is alive: it binds only once its database is
/// upgraded, and a first start on a new schema can take a while.
fn wait_listening(address: SocketAddr, alive: impl Fn() -> bool) -> Result<()> {
    loop {
        if !alive() {
            bail!("Forge exited while starting{}", recent_log());
        }
        if listening(address) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The log's last lines, for a failed start.
fn recent_log() -> String {
    let text = if uses_systemd() {
        sys::output(
            "journalctl",
            &["--user", "-u", UNIT, "-n", "15", "--no-pager", "-o", "cat"],
        )
        .unwrap_or_default()
    } else {
        log_file()
            .ok()
            .and_then(|log| std::fs::read_to_string(log).ok())
            .map(|text| {
                let lines: Vec<&str> = text.lines().collect();
                lines[lines.len().saturating_sub(15)..].join("\n")
            })
            .unwrap_or_default()
    };
    if text.trim().is_empty() {
        String::new()
    } else {
        format!("; its log ends with:\n{}", text.trim_end())
    }
}

/// Stops Forge. Returns whether it was running.
pub fn stop() -> Result<bool> {
    if uses_systemd() {
        let was = is_running();
        sys::run("systemctl", &["--user", "stop", UNIT])?;
        return Ok(was);
    }
    let Some(pid) = running_pid() else {
        return Ok(false);
    };
    if cfg!(windows) {
        sys::run("taskkill.exe", &["/PID", &pid.to_string(), "/T", "/F"])?;
    } else {
        sys::run("kill", &["-TERM", &pid.to_string()])?;
    }
    // Wait for the process itself: a dying process loses its exe before the
    // kernel closes its files, so it still holds the port for a while.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while process_exists(pid) {
        if std::time::Instant::now() >= deadline {
            bail!("Forge (pid {pid}) didn't stop within 30 seconds");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    std::fs::remove_file(pid_file()?).ok();
    Ok(true)
}

pub fn restart() -> Result<Option<u32>> {
    if uses_systemd() {
        sys::run("systemctl", &["--user", "restart", UNIT])?;
        wait_listening(address(), || {
            sys::output("systemctl", &["--user", "is-active", "--quiet", UNIT]).is_ok()
        })?;
        return Ok(None);
    }
    stop()?;
    start()
}

pub fn is_running() -> bool {
    if uses_systemd() {
        sys::output("systemctl", &["--user", "is-active", "--quiet", UNIT]).is_ok()
    } else {
        running_pid().is_some()
    }
}

fn pid_file() -> Result<PathBuf> {
    Ok(data()?.join("forge.pid"))
}

/// The detached Forge's pid, if the pid file names a live `forge` from
/// this install.
pub fn running_pid() -> Option<u32> {
    let pid: u32 = std::fs::read_to_string(pid_file().ok()?)
        .ok()?
        .trim()
        .parse()
        .ok()?;
    let mut system = sysinfo::System::new();
    let process_id = sysinfo::Pid::from_u32(pid);
    system.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::Some(&[process_id]),
        sysinfo::ProcessRefreshKind::new().with_exe(sysinfo::UpdateKind::Always),
    );
    let process = system.process(process_id)?;
    if matches!(
        process.status(),
        sysinfo::ProcessStatus::Zombie | sysinfo::ProcessStatus::Dead
    ) {
        return None;
    }
    let executable = process.exe()?.to_string_lossy().into_owned();
    let executable = executable.strip_suffix(" (deleted)").unwrap_or(&executable);
    same_path(Path::new(executable), &App::Forge.binary().ok()?).then_some(pid)
}

fn same_path(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ if cfg!(windows) => left
            .to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy()),
        _ => left == right,
    }
}

/// Whether `pid` is still a live process; a zombie has closed its files.
fn process_exists(pid: u32) -> bool {
    let mut system = sysinfo::System::new();
    let process_id = sysinfo::Pid::from_u32(pid);
    system.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::Some(&[process_id]),
        sysinfo::ProcessRefreshKind::new(),
    );
    system.process(process_id).is_some_and(|process| {
        !matches!(
            process.status(),
            sysinfo::ProcessStatus::Zombie | sysinfo::ProcessStatus::Dead
        )
    })
}

/// Whether something accepts connections on `address`.
pub fn listening(address: SocketAddr) -> bool {
    let address = if address.ip().is_unspecified() {
        SocketAddr::from(([127, 0, 0, 1], address.port()))
    } else {
        address
    };
    TcpStream::connect_timeout(&address, Duration::from_secs(1)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_unit_running_this_forge_is_ours() {
        let forge = Path::new("/home/a/.local/share/artisan-street/forge/forge");
        assert!(unit_runs(&unit_text(forge), forge));
        let quoted = Path::new("/tmp/quoted \"percent%h\" forge");
        assert!(unit_runs(&unit_text(quoted), quoted));
        assert!(!unit_runs(
            "[Service]\nExecStart=\"/home/a/.local/share/artisan/bin/ae\" forge serve\n",
            forge
        ));
    }
}
