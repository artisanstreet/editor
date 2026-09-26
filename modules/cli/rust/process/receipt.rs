use std::{
    env,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use crate::{
    CliError, Result,
    error::{ForgeTermination, io},
};

use super::{
    FORGE_READY_INTERVAL, MAX_READINESS_BYTES,
    spec::{ForgeReadiness, ForgeReadinessStatus, StartResult},
};

mod stale;

pub use stale::{ReadinessReconcile, reconcile_stale_readiness};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ReadinessFileSnapshot {
    pub(super) identity: Option<ReadinessFileIdentity>,
    pub(super) bytes: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ReadinessFileIdentity {
    pub(super) first: u64,
    pub(super) second: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum ReadinessFileRead {
    Missing,
    Invalid,
    Present(ReadinessFileSnapshot),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum BackgroundStartDecision {
    AlreadyRunning,
    Spawn {
        prior_readiness: Option<ReadinessFileSnapshot>,
    },
}

pub(super) fn background_start_decision<F>(
    existing: ReadinessFileRead,
    expected_executable: &Path,
    resolve_executable: F,
) -> BackgroundStartDecision
where
    F: FnOnce(u32) -> Option<PathBuf>,
{
    match existing {
        ReadinessFileRead::Present(snapshot) => {
            let matches_live_forge =
                ForgeReadiness::from_json(&snapshot.bytes).is_ok_and(|readiness| {
                    readiness_matches_process(&readiness, expected_executable, resolve_executable)
                });
            if matches_live_forge {
                BackgroundStartDecision::AlreadyRunning
            } else {
                BackgroundStartDecision::Spawn {
                    prior_readiness: Some(snapshot),
                }
            }
        }
        ReadinessFileRead::Missing | ReadinessFileRead::Invalid => BackgroundStartDecision::Spawn {
            prior_readiness: None,
        },
    }
}

pub(super) fn read_readiness_file(path: &Path) -> ReadinessFileRead {
    let path_metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return ReadinessFileRead::Missing;
        }
        Err(_) => return ReadinessFileRead::Invalid,
    };
    if !is_safe_readiness_file(&path_metadata) {
        return ReadinessFileRead::Invalid;
    }

    let Ok(before_file) = File::open(path) else {
        return ReadinessFileRead::Invalid;
    };
    let Ok(before_metadata) = before_file.metadata() else {
        return ReadinessFileRead::Invalid;
    };
    if !is_safe_readiness_file(&before_metadata) {
        return ReadinessFileRead::Invalid;
    }
    let Some(before_identity) = readiness_file_identity(&before_file) else {
        return ReadinessFileRead::Invalid;
    };
    drop(before_file);

    let Ok(mut file) = File::open(path) else {
        return ReadinessFileRead::Invalid;
    };
    let Ok(opened_metadata) = file.metadata() else {
        return ReadinessFileRead::Invalid;
    };
    let Some(opened_identity) = readiness_file_identity(&file) else {
        return ReadinessFileRead::Invalid;
    };
    if before_identity != opened_identity || !is_safe_readiness_file(&opened_metadata) {
        return ReadinessFileRead::Invalid;
    }

    let mut bytes = Vec::with_capacity(MAX_READINESS_BYTES.min(256));
    if file
        .by_ref()
        .take((MAX_READINESS_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return ReadinessFileRead::Invalid;
    }
    let Ok(final_metadata) = file.metadata() else {
        return ReadinessFileRead::Invalid;
    };
    let Some(final_identity) = readiness_file_identity(&file) else {
        return ReadinessFileRead::Invalid;
    };
    if opened_identity != final_identity || !is_safe_readiness_file(&final_metadata) {
        return ReadinessFileRead::Invalid;
    }
    let Ok(final_path_metadata) = fs::symlink_metadata(path) else {
        return ReadinessFileRead::Invalid;
    };
    if !is_safe_readiness_file(&final_path_metadata) {
        return ReadinessFileRead::Invalid;
    }
    let Ok(final_file) = File::open(path) else {
        return ReadinessFileRead::Invalid;
    };
    let Ok(final_file_metadata) = final_file.metadata() else {
        return ReadinessFileRead::Invalid;
    };
    if !is_safe_readiness_file(&final_file_metadata)
        || readiness_file_identity(&final_file) != Some(final_identity)
    {
        return ReadinessFileRead::Invalid;
    }

    ReadinessFileRead::Present(ReadinessFileSnapshot {
        identity: Some(final_identity),
        bytes,
    })
}

fn is_safe_readiness_file(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return false;
        }
    }
    true
}

fn readiness_file_identity(file: &File) -> Option<ReadinessFileIdentity> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        let metadata = file.metadata().ok()?;
        Some(ReadinessFileIdentity {
            first: metadata.dev(),
            second: metadata.ino(),
        })
    }
    #[cfg(windows)]
    {
        let information =
            winapi_util::file::information(winapi_util::HandleRef::from_file(file)).ok()?;
        let volume = information.volume_serial_number();
        let index = information.file_index();
        if volume == 0 && index == 0 {
            return None;
        }
        Some(ReadinessFileIdentity {
            first: volume,
            second: index,
        })
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = file;
        None
    }
}

pub(super) fn readiness_file_replaced(
    prior: Option<&ReadinessFileSnapshot>,
    current: &ReadinessFileSnapshot,
) -> bool {
    let Some(current_identity) = current.identity else {
        return false;
    };
    match prior {
        None => true,
        Some(prior) => prior
            .identity
            .is_some_and(|prior_identity| prior_identity != current_identity),
    }
}

pub(super) fn readiness_matches_process<F>(
    readiness: &ForgeReadiness,
    expected_executable: &Path,
    resolve_executable: F,
) -> bool
where
    F: FnOnce(u32) -> Option<PathBuf>,
{
    readiness_matches_child(
        readiness,
        readiness.pid(),
        expected_executable,
        resolve_executable,
    )
}

pub(super) fn readiness_matches_child<F>(
    readiness: &ForgeReadiness,
    child_pid: u32,
    expected_executable: &Path,
    resolve_executable: F,
) -> bool
where
    F: FnOnce(u32) -> Option<PathBuf>,
{
    readiness.pid() == child_pid
        && resolve_executable(child_pid)
            .is_some_and(|actual| same_executable(&actual, expected_executable))
}

fn same_executable(left: &Path, right: &Path) -> bool {
    #[cfg(windows)]
    {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

#[cfg(windows)]
pub(super) fn process_executable(pid: u32) -> Option<PathBuf> {
    let system_root = env::var_os("SystemRoot")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())?;
    let powershell = system_root
        .join("System32")
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe");
    if !powershell.is_file() {
        return None;
    }

    let filter = format!("ProcessId = {pid}");
    let output = Command::new(powershell)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
        ])
        .arg(format!(
            "(Get-CimInstance -ClassName Win32_Process -Filter '{filter}').ExecutablePath"
        ))
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    let path = stdout
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    let path = PathBuf::from(path);
    path.is_absolute().then_some(path)
}

#[cfg(unix)]
pub(super) fn process_executable(pid: u32) -> Option<PathBuf> {
    fs::read_link(format!("/proc/{pid}/exe")).ok()
}

#[cfg(not(any(unix, windows)))]
pub(super) fn process_executable(_: u32) -> Option<PathBuf> {
    None
}

pub(super) trait ChildProbe {
    fn pid(&self) -> u32;
    fn poll_termination(&mut self) -> Result<Option<ForgeTermination>>;
}

impl ChildProbe for Child {
    fn pid(&self) -> u32 {
        self.id()
    }

    fn poll_termination(&mut self) -> Result<Option<ForgeTermination>> {
        self.try_wait()
            .map(|status| status.map(|status| ForgeTermination::from_exit_status(&status)))
            .map_err(io("poll Forge startup"))
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn wait_for_readiness_with<C, ReadReceipt, ResolveExecutable, Sleep>(
    child: &mut C,
    expected_executable: &Path,
    readiness_path: &Path,
    prior_readiness: Option<&ReadinessFileSnapshot>,
    deadline: Instant,
    mut read_receipt: ReadReceipt,
    resolve_executable: ResolveExecutable,
    mut sleep: Sleep,
) -> Result<StartResult>
where
    C: ChildProbe,
    ReadReceipt: FnMut(&Path) -> ReadinessFileRead,
    ResolveExecutable: Fn(u32) -> Option<PathBuf>,
    Sleep: FnMut(Instant, Duration),
{
    let child_pid = child.pid();
    loop {
        if let Some(termination) = child.poll_termination()? {
            return Err(CliError::ForgeTerminated { termination });
        }
        if Instant::now() >= deadline {
            return readiness_timeout_or_child_exit(child);
        }

        if let ReadinessFileRead::Present(snapshot) = read_receipt(readiness_path)
            && readiness_file_replaced(prior_readiness, &snapshot)
            && let Ok(readiness) = ForgeReadiness::from_json(&snapshot.bytes)
            && readiness_matches_child(
                &readiness,
                child_pid,
                expected_executable,
                &resolve_executable,
            )
        {
            if Instant::now() >= deadline {
                return readiness_timeout_or_child_exit(child);
            }
            if let Some(termination) = child.poll_termination()? {
                return Err(CliError::ForgeTerminated { termination });
            }
            return Ok(StartResult::Spawned { pid: child_pid });
        }

        if Instant::now() >= deadline {
            return readiness_timeout_or_child_exit(child);
        }
        sleep(deadline, FORGE_READY_INTERVAL);
    }
}

fn readiness_timeout_or_child_exit<C: ChildProbe>(child: &mut C) -> Result<StartResult> {
    if let Some(termination) = child.poll_termination()? {
        Err(CliError::ForgeTerminated { termination })
    } else {
        Err(CliError::ForgeReadinessTimeout)
    }
}

pub fn readiness_status(readiness_path: &Path, expected_executable: &Path) -> ForgeReadinessStatus {
    match read_readiness_file(readiness_path) {
        ReadinessFileRead::Missing => ForgeReadinessStatus::Missing,
        ReadinessFileRead::Invalid => ForgeReadinessStatus::Invalid,
        ReadinessFileRead::Present(snapshot) => {
            let Ok(readiness) = ForgeReadiness::from_json(&snapshot.bytes) else {
                return ForgeReadinessStatus::Invalid;
            };
            if readiness_matches_process(&readiness, expected_executable, process_executable) {
                ForgeReadinessStatus::Ready(readiness)
            } else {
                ForgeReadinessStatus::Invalid
            }
        }
    }
}

pub(super) fn poll_delay(deadline: Instant, interval: Duration, now: Instant) -> Option<Duration> {
    deadline
        .checked_duration_since(now)
        .filter(|remaining| !remaining.is_zero())
        .map(|remaining| remaining.min(interval))
}

pub(super) fn sleep_until(deadline: Instant, interval: Duration) {
    if let Some(delay) = poll_delay(deadline, interval, Instant::now()) {
        thread::sleep(delay);
    }
}

#[cfg(target_os = "windows")]
pub(super) fn detach(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    // A hidden console rather than none (`DETACHED_PROCESS`). The Forge runs
    // console children constantly — git for every project reading, PowerShell
    // for file ACLs — and a child of a console-less parent allocates its own
    // visible console, which flashed a terminal window over the editor per
    // call. Children inherit this hidden console instead, so nothing paints.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
}

#[cfg(not(target_os = "windows"))]
pub(super) fn detach(_: &mut Command) {
    // std has no portable daemon/session API. Redirected stdio still makes the
    // child independent of this terminal; installers may add a service manager.
}
