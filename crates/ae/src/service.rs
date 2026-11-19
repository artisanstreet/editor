use crate::paths::{self, LocateError};
use crate::sys::{self, RemoveError, RunError, SpawnError, WriteError};
use artisan_paths::{App, EnvPathError};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

pub(crate) const UNIT: &str = "artisan-forge.service";
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "Artisan Forge";
const STOP_FILE: &str = "forge.stop";
const STALL_DEADLINE: Duration = Duration::from_secs(60);
const STOP_DEADLINE: Duration = Duration::from_secs(30);
const FORCE_DEADLINE: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(100);

#[derive(Debug, thiserror::Error)]
pub(crate) enum AddressError {
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
pub(crate) enum UnitError {
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
pub(crate) enum AutostartError {
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
pub(crate) enum OperationLockError {
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
pub(crate) enum NotReadyError {
    #[error("Forge (pid {pid}) exited while starting{log}")]
    Exited { pid: u32, log: String },
    #[error("check whether Forge (pid {pid}) recorded itself as ready")]
    Recorded {
        pid: u32,
        #[source]
        source: PidError,
    },
    #[error("measure how far Forge's log {} has grown", path.display())]
    Log {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("Forge (pid {pid}) made no progress starting for {}{log}", crate::ui::count(STALL_DEADLINE.as_secs(), "second", "seconds"))]
    Stalled { pid: u32, log: String },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum PidError {
    #[error(transparent)]
    Locate(#[from] LocateError),
    #[error("read Forge's pid file {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("Forge's pid file {} holds {text:?}, not a process id", path.display())]
    Parse {
        path: PathBuf,
        text: String,
        #[source]
        source: std::num::ParseIntError,
    },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum RunningError {
    #[error(transparent)]
    Locate(#[from] LocateError),
    #[error(transparent)]
    Unit(#[from] UnitError),
    #[error(transparent)]
    Pid(#[from] PidError),
    #[error("ask systemd whether Forge's service is active")]
    Systemd(#[source] RunError),
    #[error("find Forge's program")]
    Program(#[source] artisan_paths::RootError),
    #[cfg(unix)]
    #[error("read which Forge serves the upgrade gateway")]
    Gateway(#[source] forge_http::upgrade::GatewayRecordError),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum StartError {
    #[error(transparent)]
    Lock(#[from] OperationLockError),
    #[error(transparent)]
    Locate(#[from] LocateError),
    #[error(transparent)]
    Unit(#[from] UnitError),
    #[error("start Forge's service")]
    Systemctl(#[source] RunError),
    #[error("Forge's service isn't active after starting{log}")]
    Inactive { log: String },
    #[error("Forge is already running (pid {pid})")]
    AlreadyRunning { pid: u32 },
    #[error("check whether Forge is already running")]
    Running(#[source] RunningError),
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
    #[error("record the pid of the Forge serving at {}", path.display())]
    RecordPid {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("stopping the Forge that didn't start failed too: {stopping}")]
    NotReadyNorStopped {
        #[source]
        source: NotReadyError,
        stopping: String,
    },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum StopError {
    #[error(transparent)]
    Lock(#[from] OperationLockError),
    #[error(transparent)]
    Unit(#[from] UnitError),
    #[error("check whether Forge is running")]
    Running(#[source] RunningError),
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
    #[error("check whether Forge (pid {pid}) let go of its data folder")]
    Release {
        pid: u32,
        #[source]
        source: forge_http::LockDataError,
    },
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum RestartError {
    #[error(transparent)]
    Lock(#[from] OperationLockError),
    #[error(transparent)]
    Unit(#[from] UnitError),
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

pub(crate) fn address() -> Result<SocketAddr, AddressError> {
    let data = data()?;
    forge_http::configured_listen(&data).map_err(|source| AddressError::Read { data, source })
}

pub(crate) fn log_file() -> Result<PathBuf, LocateError> {
    Ok(data()?.join("forge.log"))
}

pub(crate) fn log_hint() -> Result<String, UnitError> {
    if uses_systemd()? {
        Ok(format!("journalctl --user -u {UNIT}"))
    } else {
        Ok(log_file()?.display().to_string())
    }
}

pub(crate) fn unit_path() -> Result<PathBuf, EnvPathError> {
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

fn uses_systemd() -> Result<bool, UnitError> {
    Ok(unit_is_ours()? && systemd_available())
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

#[derive(Debug, PartialEq, Eq)]
struct ServicePath {
    kept: String,
    unrepresentable: Vec<PathBuf>,
}

fn service_path(installer: &std::ffi::OsStr, temporary: &Path) -> ServicePath {
    let mut kept: Vec<String> = Vec::new();
    let mut unrepresentable = Vec::new();
    for entry in std::env::split_paths(installer) {
        match entry.to_str() {
            Some(text) => {
                let transient = text.is_empty()
                    || text.starts_with("/nix/store/")
                    || entry.starts_with(temporary);
                if !transient && !kept.iter().any(|known| known == text) {
                    kept.push(text.to_owned());
                }
            }
            None => unrepresentable.push(entry),
        }
    }
    ServicePath {
        kept: kept.join(":"),
        unrepresentable,
    }
}

fn unit_text(forge: &Path, mode: StopMode, path: Option<&str>) -> String {
    let kill_mode = match mode {
        StopMode::Gateway => "mixed",
        StopMode::ProcessGroup => "control-group",
    };
    let path = path.map_or_else(String::new, |path| {
        format!("Environment={}\n", unit_quote(&format!("PATH={path}")))
    });
    format!(
        "[Unit]\nDescription=Artisan Forge\nAfter=network-online.target\n\n\
         [Service]\nType=notify\nNotifyAccess=all\nDelegate=yes\nDelegateSubgroup=forge\nExecStart={}\n{path}\
         Environment=GLIBC_TUNABLES=glibc.malloc.arena_max=2\n\
         Restart=on-failure\nRestartSec=5\nTimeoutStartSec=180\nTimeoutStopSec=30\nKillMode={kill_mode}\nSendSIGKILL=no\n\n[Install]\nWantedBy=default.target\n",
        unit_quote(&forge.to_string_lossy())
    )
}

fn this_unit(mode: StopMode) -> Result<String, LocateError> {
    let path =
        std::env::var_os("PATH").map(|installer| service_path(&installer, &std::env::temp_dir()));
    for left_out in path.iter().flat_map(|path| &path.unrepresentable) {
        crate::ui::warning(format!(
            "Forge's service leaves {} off its PATH because systemd can't hold a folder name that isn't Unicode",
            left_out.to_string_lossy()
        ));
    }
    Ok(unit_text(
        &paths::binary(App::Forge)?,
        mode,
        path.as_ref().map(|path| path.kept.as_str()),
    ))
}

fn write_unit(mode: StopMode) -> Result<(), AutostartError> {
    let unit = unit_path().map_err(UnitError::Config)?;
    sys::write(&unit, &this_unit(mode)?).map_err(AutostartError::Write)
}

pub(crate) fn prepare_bootstrap_stop() -> Result<(), AutostartError> {
    if uses_systemd()? {
        write_unit(StopMode::ProcessGroup)?;
        sys::run("systemctl", &["--user", "daemon-reload"])?;
    }
    Ok(())
}

fn run_command(forge: &Path) -> String {
    format!("\"{}\"", forge.display())
}

pub(crate) fn enable_autostart() -> Result<&'static str, AutostartError> {
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

pub(crate) fn disable_autostart() -> Result<(), AutostartError> {
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

pub(crate) fn start() -> Result<Option<u32>, StartError> {
    let _operation = operation_lock()?;
    start_unlocked()
}

fn start_unlocked() -> Result<Option<u32>, StartError> {
    if uses_systemd()? {
        sys::run("systemctl", &["--user", "start", UNIT]).map_err(StartError::Systemctl)?;
        if !systemd_active().map_err(StartError::Systemctl)? {
            return Err(StartError::Inactive { log: recent_log() });
        }
        return Ok(None);
    }
    if let Some(pid) = running_pid().map_err(StartError::Running)? {
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
    match wait_ready(pid, address, &log) {
        Ok(Ready::Recorded) => {}
        Ok(Ready::Serving) => {
            let recorded = pid_file()?;
            std::fs::write(&recorded, pid.to_string()).map_err(|source| StartError::RecordPid {
                path: recorded,
                source,
            })?;
        }
        Err(error) => {
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
    }
    Ok(Some(pid))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Startup {
    Ready,
    Serving,
    Starting,
    Exited,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ready {
    Recorded,
    Serving,
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
            Startup::Ready | Startup::Serving => return Verdict::Ready,
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

fn startup(pid: u32, address: SocketAddr) -> Result<Startup, PidError> {
    Ok(if recorded_pid()? == Some(pid) {
        Startup::Ready
    } else if !process_exists(pid) {
        Startup::Exited
    } else if listening(address) {
        Startup::Serving
    } else {
        Startup::Starting
    })
}

fn log_length(log: &Path) -> Result<u64, NotReadyError> {
    match std::fs::metadata(log) {
        Ok(metadata) => Ok(metadata.len()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(source) => Err(NotReadyError::Log {
            path: log.to_owned(),
            source,
        }),
    }
}

fn wait_ready(pid: u32, address: SocketAddr, log: &Path) -> Result<Ready, NotReadyError> {
    let mut watch = StartWatch::new(log_length(log)?, Instant::now());
    loop {
        let now =
            startup(pid, address).map_err(|source| NotReadyError::Recorded { pid, source })?;
        match watch.observe(now, log_length(log)?, Instant::now()) {
            Verdict::Ready if now == Startup::Serving => return Ok(Ready::Serving),
            Verdict::Ready => return Ok(Ready::Recorded),
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

fn systemd_active() -> Result<bool, RunError> {
    match sys::output("systemctl", &["--user", "is-active", "--quiet", UNIT]) {
        Ok(_active) => Ok(true),
        Err(RunError::Failed { .. }) => Ok(false),
        Err(error) => Err(error),
    }
}

fn recent_log() -> String {
    let text = match uses_systemd() {
        Err(error) => format!(
            "(finding out whether systemd runs Forge failed: {})",
            diagnostics::chain(&error)
        ),
        Ok(true) => sys::output(
            "journalctl",
            &["--user", "-u", UNIT, "-n", "15", "--no-pager", "-o", "cat"],
        )
        .unwrap_or_else(|error| format!("(journalctl failed: {})", diagnostics::chain(&error))),
        Ok(false) => match log_file().map(std::fs::read_to_string) {
            Ok(Ok(text)) => {
                let lines: Vec<&str> = text.lines().collect();
                lines[lines.len().saturating_sub(15)..].join("\n")
            }
            Ok(Err(error)) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Ok(Err(error)) => format!("(reading the log failed: {error})"),
            Err(error) => format!("(finding the log failed: {})", diagnostics::chain(&error)),
        },
    };
    if text.trim().is_empty() {
        String::new()
    } else {
        format!("; its log ends with:\n{}", text.trim_end())
    }
}

pub(crate) fn stop() -> Result<bool, StopError> {
    let _operation = operation_lock()?;
    stop_unlocked()
}

fn stop_unlocked() -> Result<bool, StopError> {
    if uses_systemd()? {
        let was = is_running().map_err(StopError::Running)?;
        sys::run("systemctl", &["--user", "stop", UNIT]).map_err(StopError::Systemctl)?;
        return Ok(was);
    }
    let Some(pid) = running_pid().map_err(StopError::Running)? else {
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
    let stopped = wait_stopped(pid, data, STOP_DEADLINE)?;
    if !stopped && cfg!(windows) {
        sys::run("taskkill.exe", &["/PID", &pid.to_string(), "/T", "/F"])
            .map_err(|source| StopError::Force { pid, source })?;
        if !wait_stopped(pid, data, FORCE_DEADLINE)? {
            return Err(StopError::Survived { pid });
        }
    } else if !stopped {
        return Err(StopError::TimedOut { pid });
    }
    sys::remove(&pid_file()?).map_err(StopError::Clear)?;
    sys::remove(&data.join(STOP_FILE)).map_err(StopError::Clear)
}

fn data_released(pid: u32, data: &Path) -> Result<bool, StopError> {
    match forge_http::lock_data_dir(data) {
        Ok(_released) => Ok(true),
        Err(forge_http::LockDataError::Held { .. }) => Ok(false),
        Err(
            source @ (forge_http::LockDataError::Open { .. }
            | forge_http::LockDataError::Lock { .. }),
        ) => Err(StopError::Release { pid, source }),
    }
}

fn wait_stopped(pid: u32, data: &Path, deadline: Duration) -> Result<bool, StopError> {
    let started = Instant::now();
    loop {
        if !process_exists(pid) && data_released(pid, data)? {
            return Ok(true);
        }
        if started.elapsed() >= deadline {
            return Ok(false);
        }
        std::thread::sleep(POLL);
    }
}

pub(crate) fn restart() -> Result<Option<u32>, RestartError> {
    let _operation = operation_lock()?;
    if uses_systemd()? {
        sys::run("systemctl", &["--user", "restart", UNIT]).map_err(RestartError::Systemctl)?;
        if !systemd_active().map_err(RestartError::Systemctl)? {
            return Err(RestartError::Inactive { log: recent_log() });
        }
        return Ok(None);
    }
    stop_unlocked()?;
    Ok(start_unlocked()?)
}

pub(crate) fn is_running() -> Result<bool, RunningError> {
    if uses_systemd()? {
        systemd_active().map_err(RunningError::Systemd)
    } else {
        Ok(running_pid()?.is_some())
    }
}

fn pid_file() -> Result<PathBuf, LocateError> {
    Ok(data()?.join("forge.pid"))
}

fn recorded_pid() -> Result<Option<u32>, PidError> {
    let path = pid_file()?;
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(PidError::Read { path, source }),
    };
    let recorded = text.trim();
    if recorded.is_empty() {
        return Ok(None);
    }
    recorded
        .parse()
        .map(Some)
        .map_err(|source| PidError::Parse {
            path,
            text: recorded.to_owned(),
            source,
        })
}

pub(crate) fn running_pid() -> Result<Option<u32>, RunningError> {
    let Some(pid) = recorded_pid()? else {
        return Ok(None);
    };
    let Some(process) = crate::platform::running(pid) else {
        return Ok(None);
    };
    #[cfg(unix)]
    if let Some(gateway) = forge_http::upgrade::gateway(&data()?).map_err(RunningError::Gateway)?
        && gateway.pid == pid
        && same_path(&process.executable, &gateway.binary)
    {
        return Ok(Some(pid));
    }
    let forge = App::Forge.binary().map_err(RunningError::Program)?;
    Ok(same_path(&process.executable, &forge).then_some(pid))
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

pub(crate) fn listening(address: SocketAddr) -> bool {
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
