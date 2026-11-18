use crate::paths::{self, LocateError};
use crate::sys::{self, RemoveError, RunError, SpawnError, WriteError};
use artisan_paths::{App, EnvPathError};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

pub const UNIT: &str = "artisan-forge.service";
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "Artisan Forge";
const STOP_FILE: &str = "forge.stop";
const STALL_DEADLINE: Duration = Duration::from_secs(60);
const STOP_DEADLINE: Duration = Duration::from_secs(30);
const FORCE_DEADLINE: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(100);

#[derive(Debug, thiserror::Error)]
pub enum AddressError {
    #[error(transparent)]
    Locate(#[from] LocateError),
    #[error("read where Forge listens from {}", data.display())]
    Read {
        data: PathBuf,
        #[source]
        source: forge_http::ConfiguredListenError,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum UnitError {
    #[error("find the systemd user folder")]
    Config(#[source] EnvPathError),
    #[error(transparent)]
    Locate(#[from] LocateError),
    #[error("read {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum AutostartError {
    #[error(transparent)]
    Unit(#[from] UnitError),
    #[error(transparent)]
    Locate(#[from] LocateError),
    #[error("write Forge's systemd unit")]
    Write(#[source] WriteError),
    #[error(transparent)]
    Run(#[from] RunError),
    #[error("remove Forge's systemd unit")]
    Remove(#[source] RemoveError),
}

#[derive(Debug, thiserror::Error)]
pub enum OperationLockError {
    #[error(transparent)]
    Locate(#[from] LocateError),
    #[error("lock Forge service operations with {}", path.display())]
    Lock {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum NotReadyError {
    #[error("Forge (pid {pid}) exited while starting{log}")]
    Exited { pid: u32, log: String },
    #[error("Forge (pid {pid}) made no progress starting for {}{log}", crate::ui::count(STALL_DEADLINE.as_secs(), "second", "seconds"))]
    Stalled { pid: u32, log: String },
}

#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error(transparent)]
    Lock(#[from] OperationLockError),
    #[error(transparent)]
    Locate(#[from] LocateError),
    #[error("start Forge's service")]
    Systemctl(#[source] RunError),
    #[error("Forge's service isn't active after starting{log}")]
    Inactive { log: String },
    #[error("Forge is already running (pid {pid})")]
    AlreadyRunning { pid: u32 },
    #[error("Forge isn't installed here; run `ae install`")]
    NotInstalled,
    #[error(transparent)]
    Address(#[from] AddressError),
    #[error("{address} is already occupied by another service")]
    Occupied { address: SocketAddr },
    #[error("create {}", path.display())]
    CreateData {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("clear what the last Forge left")]
    Clear(#[source] RemoveError),
    #[error(transparent)]
    Spawn(#[from] SpawnError),
    #[error(transparent)]
    NotReady(#[from] NotReadyError),
    #[error("stopping the Forge that didn't start failed too: {stopping}")]
    NotReadyNorStopped {
        #[source]
        source: NotReadyError,
        stopping: String,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum StopError {
    #[error(transparent)]
    Lock(#[from] OperationLockError),
    #[error(transparent)]
    Locate(#[from] LocateError),
    #[error("stop Forge's service")]
    Systemctl(#[source] RunError),
    #[error("ask Forge (pid {pid}) to stop")]
    Ask {
        pid: u32,
        #[source]
        source: std::io::Error,
    },
    #[error("signal Forge (pid {pid}) to stop")]
    Signal {
        pid: u32,
        #[source]
        source: RunError,
    },
    #[error("force Forge (pid {pid}) to stop")]
    Force {
        pid: u32,
        #[source]
        source: RunError,
    },
    #[error("Forge (pid {pid}) didn't stop even when forced")]
    Survived { pid: u32 },
    #[error("Forge (pid {pid}) didn't stop within {}", crate::ui::count(STOP_DEADLINE.as_secs(), "second", "seconds"))]
    TimedOut { pid: u32 },
    #[error("clear what the stopped Forge left")]
    Clear(#[source] RemoveError),
}

#[derive(Debug, thiserror::Error)]
pub enum RestartError {
    #[error(transparent)]
    Lock(#[from] OperationLockError),
    #[error("restart Forge's service")]
    Systemctl(#[source] RunError),
    #[error("Forge's service isn't active after restarting{log}")]
    Inactive { log: String },
    #[error(transparent)]
    Stop(#[from] StopError),
    #[error(transparent)]
    Start(#[from] StartError),
}

fn data() -> Result<PathBuf, LocateError> {
    paths::data(App::Forge)
}

pub fn address() -> Result<SocketAddr, AddressError> {
    let data = data()?;
    forge_http::configured_listen(&data).map_err(|source| AddressError::Read { data, source })
}

pub fn log_file() -> Result<PathBuf, LocateError> {
    Ok(data()?.join("forge.log"))
}

pub fn log_hint() -> Result<String, LocateError> {
    if uses_systemd() {
        Ok(format!("journalctl --user -u {UNIT}"))
    } else {
        Ok(log_file()?.display().to_string())
    }
}

pub fn unit_path() -> Result<PathBuf, EnvPathError> {
    Ok(artisan_paths::xdg_config()?.join("systemd/user").join(UNIT))
}

fn wants_link() -> Result<PathBuf, EnvPathError> {
    Ok(artisan_paths::xdg_config()?
        .join("systemd/user/default.target.wants")
        .join(UNIT))
}

fn systemd_available() -> bool {
    cfg!(target_os = "linux") && sys::output("systemctl", &["--user", "show-environment"]).is_ok()
}

fn unit_is_ours() -> Result<bool, UnitError> {
    if !cfg!(target_os = "linux") {
        return Ok(false);
    }
    let unit = unit_path().map_err(UnitError::Config)?;
    match std::fs::read_to_string(&unit) {
        Ok(text) => Ok(unit_runs(&text, &paths::binary(App::Forge)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(UnitError::Read { path: unit, source }),
    }
}

fn uses_systemd() -> bool {
    unit_is_ours().is_ok_and(|ours| ours) && systemd_available()
}

fn unit_runs(text: &str, forge: &Path) -> bool {
    text.lines()
        .find_map(|line| line.strip_prefix("ExecStart="))
        .is_some_and(|exec| exec == unit_quote(&forge.to_string_lossy()))
}

fn unit_quote(value: &str) -> String {
    serde_json::to_string(&value.replace('%', "%%"))
        .expect("invariant: a string always serializes as JSON")
}

#[derive(Clone, Copy)]
enum StopMode {
    Gateway,
    ProcessGroup,
}

fn service_path(installer: &str, temporary: &Path) -> String {
    let mut kept: Vec<&str> = Vec::new();
    for entry in installer.split(':') {
        let transient = entry.is_empty()
            || entry.starts_with("/nix/store/")
            || Path::new(entry).starts_with(temporary);
        if !transient && !kept.contains(&entry) {
            kept.push(entry);
        }
    }
    kept.join(":")
}

fn unit_text(forge: &Path, mode: StopMode, path: &str) -> String {
    let kill_mode = match mode {
        StopMode::Gateway => "mixed",
        StopMode::ProcessGroup => "control-group",
    };
    let path = unit_quote(&format!("PATH={path}"));
    format!(
        "[Unit]\nDescription=Artisan Forge\nAfter=network-online.target\n\n\
         [Service]\nType=notify\nNotifyAccess=all\nDelegate=yes\nDelegateSubgroup=forge\nExecStart={}\nEnvironment={path}\n\
         Environment=GLIBC_TUNABLES=glibc.malloc.arena_max=2\n\
         Restart=on-failure\nRestartSec=5\nTimeoutStartSec=180\nTimeoutStopSec=30\nKillMode={kill_mode}\nSendSIGKILL=no\n\n[Install]\nWantedBy=default.target\n",
        unit_quote(&forge.to_string_lossy())
    )
}

fn this_unit(mode: StopMode) -> Result<String, LocateError> {
    let installer = std::env::var("PATH").unwrap_or_default();
    Ok(unit_text(
        &paths::binary(App::Forge)?,
        mode,
        &service_path(&installer, &std::env::temp_dir()),
    ))
}

fn write_unit(mode: StopMode) -> Result<(), AutostartError> {
    let unit = unit_path().map_err(UnitError::Config)?;
    sys::write(&unit, &this_unit(mode)?).map_err(AutostartError::Write)
}

pub fn prepare_bootstrap_stop() -> Result<(), AutostartError> {
    if uses_systemd() {
        write_unit(StopMode::ProcessGroup)?;
        sys::run("systemctl", &["--user", "daemon-reload"])?;
    }
    Ok(())
}

fn run_command(forge: &Path) -> String {
    format!("\"{}\"", forge.display())
}

pub fn enable_autostart() -> Result<&'static str, AutostartError> {
    let forge = paths::binary(App::Forge)?;
    if systemd_available() {
        write_unit(StopMode::Gateway)?;
        sys::run("systemctl", &["--user", "daemon-reload"])?;
        sys::run("systemctl", &["--user", "enable", UNIT])?;
        Ok("starts at login (systemd)")
    } else if cfg!(windows) {
        sys::run(
            "reg.exe",
            &[
                "add",
                RUN_KEY,
                "/v",
                RUN_VALUE,
                "/d",
                &run_command(&forge),
                "/f",
            ],
        )?;
        Ok("starts at sign-in")
    } else if cfg!(target_os = "linux") {
        Ok("start it with `ae forge start` (systemd isn't running here)")
    } else {
        Ok("start it with `ae forge start`")
    }
}

pub fn disable_autostart() -> Result<(), AutostartError> {
    if unit_is_ours()? {
        let manager = systemd_available();
        if manager {
            sys::run("systemctl", &["--user", "disable", UNIT])?;
        }
        sys::remove(&wants_link().map_err(UnitError::Config)?).map_err(AutostartError::Remove)?;
        sys::remove(&unit_path().map_err(UnitError::Config)?).map_err(AutostartError::Remove)?;
        if manager {
            sys::run("systemctl", &["--user", "daemon-reload"])?;
        }
    }
    if cfg!(windows) && run_entry()?.is_some_and(|command| command == run_command_of_forge()) {
        sys::run("reg.exe", &["delete", RUN_KEY, "/v", RUN_VALUE, "/f"])?;
    }
    Ok(())
}

fn run_command_of_forge() -> String {
    App::Forge
        .binary()
        .map_or_else(|_| String::new(), |forge| run_command(&forge))
}

fn run_entry() -> Result<Option<String>, RunError> {
    let value = sys::powershell(&format!(
        "$v = (Get-ItemProperty -Path {key} -Name {name} -ErrorAction SilentlyContinue).{name}; if ($v) {{ $v }}",
        key = sys::ps_string(r"HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"),
        name = sys::ps_string(RUN_VALUE),
    ))?;
    let value = value.trim();
    Ok((!value.is_empty()).then(|| value.to_owned()))
}

fn operation_lock() -> Result<std::fs::File, OperationLockError> {
    let directory = data()?;
    let path = directory.join("service.lock");
    let lock = |source| OperationLockError::Lock {
        path: path.clone(),
        source,
    };
    std::fs::create_dir_all(&directory).map_err(lock)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(lock)?;
    file.lock().map_err(lock)?;
    Ok(file)
}

pub fn start() -> Result<Option<u32>, StartError> {
    let _operation = operation_lock()?;
    start_unlocked()
}

fn start_unlocked() -> Result<Option<u32>, StartError> {
    if uses_systemd() {
        sys::run("systemctl", &["--user", "start", UNIT]).map_err(StartError::Systemctl)?;
        if !systemd_active() {
            return Err(StartError::Inactive { log: recent_log() });
        }
        return Ok(None);
    }
    if let Some(pid) = running_pid() {
        return Err(StartError::AlreadyRunning { pid });
    }
    let forge = paths::binary(App::Forge)?;
    if !forge.is_file() {
        return Err(StartError::NotInstalled);
    }
    let address = address()?;
    if listening(address) {
        return Err(StartError::Occupied { address });
    }
    let data = data()?;
    std::fs::create_dir_all(&data).map_err(|source| StartError::CreateData {
        path: data.clone(),
        source,
    })?;
    sys::remove(&pid_file()?).map_err(StartError::Clear)?;
    sys::remove(&data.join(STOP_FILE)).map_err(StartError::Clear)?;
    let log = log_file()?;
    let pid = sys::spawn_detached(
        Command::new(&forge)
            .env_remove("LD_LIBRARY_PATH")
            .arg("--log")
            .arg(&log)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null()),
    )?;
    if let Err(error) = wait_ready(pid, &log) {
        if process_exists(pid)
            && let Err(stopping) = stop_process(pid, &data)
        {
            return Err(StartError::NotReadyNorStopped {
                source: error,
                stopping: diagnostics::chain(&stopping),
            });
        }
        return Err(error.into());
    }
    Ok(Some(pid))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Startup {
    Ready,
    Starting,
    Exited,
}

#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Ready,
    Waiting,
    Exited,
    Stalled,
}

struct StartWatch {
    logged: u64,
    since: Instant,
}

impl StartWatch {
    const fn new(logged: u64, now: Instant) -> Self {
        Self { logged, since: now }
    }

    fn observe(&mut self, startup: Startup, logged: u64, now: Instant) -> Verdict {
        match startup {
            Startup::Ready => return Verdict::Ready,
            Startup::Exited => return Verdict::Exited,
            Startup::Starting => {}
        }
        if logged != self.logged {
            self.logged = logged;
            self.since = now;
            Verdict::Waiting
        } else if now.duration_since(self.since) >= STALL_DEADLINE {
            Verdict::Stalled
        } else {
            Verdict::Waiting
        }
    }
}

fn startup(pid: u32) -> Startup {
    if pid_file().ok().and_then(|path| recorded_pid(&path)) == Some(pid) {
        Startup::Ready
    } else if process_exists(pid) {
        Startup::Starting
    } else {
        Startup::Exited
    }
}

fn log_length(log: &Path) -> u64 {
    std::fs::metadata(log).map_or(0, |metadata| metadata.len())
}

fn wait_ready(pid: u32, log: &Path) -> Result<(), NotReadyError> {
    let mut watch = StartWatch::new(log_length(log), Instant::now());
    loop {
        match watch.observe(startup(pid), log_length(log), Instant::now()) {
            Verdict::Ready => return Ok(()),
            Verdict::Exited => {
                return Err(NotReadyError::Exited {
                    pid,
                    log: recent_log(),
                });
            }
            Verdict::Stalled => {
                return Err(NotReadyError::Stalled {
                    pid,
                    log: recent_log(),
                });
            }
            Verdict::Waiting => std::thread::sleep(POLL),
        }
    }
}

fn systemd_active() -> bool {
    sys::output("systemctl", &["--user", "is-active", "--quiet", UNIT]).is_ok()
}

fn recent_log() -> String {
    let text = if uses_systemd() {
        sys::output(
            "journalctl",
            &["--user", "-u", UNIT, "-n", "15", "--no-pager", "-o", "cat"],
        )
        .unwrap_or_else(|error| format!("(journalctl failed: {})", diagnostics::chain(&error)))
    } else {
        match log_file().map(std::fs::read_to_string) {
            Ok(Ok(text)) => {
                let lines: Vec<&str> = text.lines().collect();
                lines[lines.len().saturating_sub(15)..].join("\n")
            }
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Ok(Err(error)) => format!("(reading the log failed: {error})"),
            Err(error) => format!("(finding the log failed: {})", diagnostics::chain(&error)),
        }
    };
    if text.trim().is_empty() {
        String::new()
    } else {
        format!("; its log ends with:\n{}", text.trim_end())
    }
}

pub fn stop() -> Result<bool, StopError> {
    let _operation = operation_lock()?;
    stop_unlocked()
}

fn stop_unlocked() -> Result<bool, StopError> {
    if uses_systemd() {
        let was = is_running();
        sys::run("systemctl", &["--user", "stop", UNIT]).map_err(StopError::Systemctl)?;
        return Ok(was);
    }
    let Some(pid) = running_pid() else {
        return Ok(false);
    };
    stop_process(pid, &data()?)?;
    Ok(true)
}

fn stop_process(pid: u32, data: &Path) -> Result<(), StopError> {
    if cfg!(windows) {
        std::fs::write(data.join(STOP_FILE), pid.to_string())
            .map_err(|source| StopError::Ask { pid, source })?;
    } else {
        sys::run("kill", &["-TERM", &pid.to_string()])
            .map_err(|source| StopError::Signal { pid, source })?;
    }
    let stopped = wait_stopped(pid, data, STOP_DEADLINE);
    if !stopped && cfg!(windows) {
        sys::run("taskkill.exe", &["/PID", &pid.to_string(), "/T", "/F"])
            .map_err(|source| StopError::Force { pid, source })?;
        if !wait_stopped(pid, data, FORCE_DEADLINE) {
            return Err(StopError::Survived { pid });
        }
    } else if !stopped {
        return Err(StopError::TimedOut { pid });
    }
    sys::remove(&pid_file()?).map_err(StopError::Clear)?;
    sys::remove(&data.join(STOP_FILE)).map_err(StopError::Clear)
}

fn wait_stopped(pid: u32, data: &Path, deadline: Duration) -> bool {
    let started = Instant::now();
    loop {
        if !process_exists(pid) && forge_http::lock_data_dir(data).is_ok() {
            return true;
        }
        if started.elapsed() >= deadline {
            return false;
        }
        std::thread::sleep(POLL);
    }
}

pub fn restart() -> Result<Option<u32>, RestartError> {
    let _operation = operation_lock()?;
    if uses_systemd() {
        sys::run("systemctl", &["--user", "restart", UNIT]).map_err(RestartError::Systemctl)?;
        if !systemd_active() {
            return Err(RestartError::Inactive { log: recent_log() });
        }
        return Ok(None);
    }
    stop_unlocked()?;
    Ok(start_unlocked()?)
}

pub fn is_running() -> bool {
    if uses_systemd() {
        systemd_active()
    } else {
        running_pid().is_some()
    }
}

fn pid_file() -> Result<PathBuf, LocateError> {
    Ok(data()?.join("forge.pid"))
}

fn recorded_pid(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

pub fn running_pid() -> Option<u32> {
    let pid = recorded_pid(&pid_file().ok()?)?;
    let process = crate::platform::running(pid)?;
    #[cfg(unix)]
    if let Some(gateway) = forge_http::upgrade::gateway(&data().ok()?).ok().flatten()
        && gateway.pid == pid
        && same_path(&process.executable, &gateway.binary)
    {
        return Some(pid);
    }
    same_path(&process.executable, &App::Forge.binary().ok()?).then_some(pid)
}

fn same_path(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => crate::platform::same_executable(left, right),
    }
}

fn process_exists(pid: u32) -> bool {
    crate::platform::running(pid).is_some()
}

pub fn listening(address: SocketAddr) -> bool {
    let address = if address.ip().is_unspecified() {
        SocketAddr::from(([127, 0, 0, 1], address.port()))
    } else {
        address
    };
    TcpStream::connect_timeout(&address, Duration::from_secs(1)).is_ok()
}

#[cfg(test)]
#[path = "../tests/service/unit.rs"]
mod tests;
