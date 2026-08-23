use std::{
    path::{Path, PathBuf},
    thread::sleep,
    time::{Duration, Instant},
};

use crate::{
    background_process::background_command,
    error::{InstallerError, Result},
};

/// How long a gracefully-asked process is given before it is judged stuck.
const GRACEFUL_WINDOW: Duration = Duration::from_secs(20);
/// Polling interval while waiting for a graceful exit.
const POLL_INTERVAL: Duration = Duration::from_millis(250);
/// The permanent CLI reserves this code for a Forge that reports active work.
const FORGE_BUSY_EXIT_CODE: i32 = 5;
/// An exact Forge process exists but cannot answer the authenticated activity
/// probe. It is broken rather than busy, so update retirement may end it.
const FORGE_ACTIVITY_UNAVAILABLE_EXIT_CODE: i32 = 6;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ForgeStopDisposition {
    Accepted,
    Busy,
    Unresponsive,
    Failed,
}

fn forge_stop_disposition(exit_code: Option<i32>) -> ForgeStopDisposition {
    match exit_code {
        Some(0) => ForgeStopDisposition::Accepted,
        Some(FORGE_BUSY_EXIT_CODE) => ForgeStopDisposition::Busy,
        Some(FORGE_ACTIVITY_UNAVAILABLE_EXIT_CODE) => ForgeStopDisposition::Unresponsive,
        _ => ForgeStopDisposition::Failed,
    }
}
/// One running process launched out of the installation's `versions` tree.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunningProcess {
    pub pid: u32,
    pub executable: PathBuf,
}

impl RunningProcess {
    /// Versioned payloads live at `versions/<version>/<component>/…`, so the
    /// component is read from the path rather than the executable name: the
    /// Forge host is a bundled runtime whose file name is not ours to depend on.
    fn component(&self, versions_root: &Path) -> Option<String> {
        self.executable
            .strip_prefix(versions_root)
            .ok()?
            .components()
            .nth(1)
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
    }
}

/// What retiring the superseded instances actually did.
#[derive(Clone, Copy, Debug, Default)]
pub struct Retirement {
    pub editors_closed: usize,
    pub forges_stopped: usize,
}

impl Retirement {
    pub const fn is_empty(self) -> bool {
        self.editors_closed == 0 && self.forges_stopped == 0
    }
}

/// Governs how far the installer may go to free a superseded instance.
#[derive(Clone, Copy, Debug)]
pub struct RetirementPolicy {
    /// Permits retirement even when an authenticated Forge reports active
    /// work. Without it, only that explicit busy report cancels the update;
    /// an unresponsive broken Forge is still retired.
    pub force: bool,
}

/// Retires every process still running a superseded version of this
/// installation, so the freshly activated release is what actually loads.
///
/// Electron holds a single-instance lock: launching the new editor while an
/// old one runs exits immediately and focuses the stale window, leaving a
/// user staring at the previous build with `active_version` already flipped —
/// an update that silently did nothing. The editor is a renderer, so it is
/// closed gracefully and then forced; Forge owns live agent runs and the
/// database lock, so an authenticated active-work report cancels retirement
/// unless `--force` was explicit. An unresponsive Forge is treated as broken
/// and retired so it cannot wedge every future build. Forge is gated first so
/// a busy refusal leaves the editor open around its work.
///
/// Instances already running the incoming release are deliberately left
/// alone: focusing those is correct behaviour, not staleness.
pub fn retire_superseded(
    install_root: &Path,
    release: &Path,
    stable_ae: &Path,
    policy: RetirementPolicy,
) -> Result<Retirement> {
    let versions_root = install_root.join("versions");
    let superseded: Vec<RunningProcess> = discover(&versions_root)?
        .into_iter()
        .filter(|process| !process.executable.starts_with(release))
        .collect();
    if superseded.is_empty() {
        return Ok(Retirement::default());
    }

    let mut retirement = Retirement::default();
    let (forges, others): (Vec<_>, Vec<_>) = superseded
        .iter()
        .partition(|process| process.component(&versions_root).as_deref() == Some("forge"));

    if !forges.is_empty() {
        // The product's own stop path: it releases the database lock and
        // settles running work rather than severing it mid-write.
        // Mutable state can be missing or stale while Forge's immutable,
        // authenticated instance card is still live. The permanent `ae`
        // lifecycle path repairs that split-brain view before stopping Forge;
        // capture its diagnostic because process identity below is the final
        // authority for whether retirement actually completed.
        let mut orderly_stop_accepted = false;
        let mut busy_refusal = false;
        let mut unresponsive = Vec::new();
        let mut stop_diagnostics = Vec::new();
        for process in &forges {
            let output = background_command(stable_ae)
                .args(exact_stop_arguments(process.pid, !policy.force))
                .env("ARTISAN_HOME", install_root)
                .output();
            let disposition = output
                .as_ref()
                .map_or(ForgeStopDisposition::Failed, |output| {
                    forge_stop_disposition(output.status.code())
                });
            match disposition {
                ForgeStopDisposition::Accepted => {
                    orderly_stop_accepted = true;
                    continue;
                }
                ForgeStopDisposition::Busy => busy_refusal = true,
                ForgeStopDisposition::Unresponsive if !policy.force => {
                    unresponsive.push(*process);
                }
                ForgeStopDisposition::Unresponsive | ForgeStopDisposition::Failed => {}
            }
            let diagnostic = output.as_ref().map_or_else(
                |error| format!("could not launch the lifecycle CLI: {error}"),
                |output| String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            );
            let diagnostic = if diagnostic.is_empty() {
                "refused shutdown without a diagnostic".to_owned()
            } else {
                diagnostic
            };
            let diagnostic = format!("pid {}: {diagnostic}", process.pid);
            stop_diagnostics.push(diagnostic);
        }
        if !policy.force && busy_refusal {
            return Err(InstallerError::InvalidInstallation(format!(
                "update cancelled before activation: {}. Forge and its active work were left running",
                stop_diagnostics.join("; ")
            )));
        }
        if !policy.force && !orderly_stop_accepted && unresponsive.is_empty() {
            return Err(InstallerError::InvalidInstallation(format!(
                "update cancelled before activation: no authenticated Forge accepted the idle-only shutdown ({})",
                stop_diagnostics.join("; ")
            )));
        }
        // The safe refusal above is reserved for an authenticated busy report.
        // A process that cannot answer the activity endpoint is a broken Forge,
        // not evidence of live work; retire its owned process tree so a stale
        // version cannot permanently wedge every subsequent build.
        for process in unresponsive {
            terminate(process.pid);
        }
        let stuck = await_exit(&versions_root, &forges, GRACEFUL_WINDOW)?;
        if !stuck.is_empty() {
            if !policy.force {
                return Err(InstallerError::InvalidInstallation(format!(
                    "Forge is still running from a superseded version (pid {}) and did not stop when asked. \
                     Finish or stop its work and retry, or pass --force to end it. The permanent ae \
                     lifecycle path could not establish an authenticated orderly shutdown.",
                    stuck
                        .iter()
                        .map(|process| process.pid.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )));
            }
            for process in &stuck {
                terminate(process.pid);
            }
        }
        retirement.forges_stopped = forges.len();
    }

    for process in &others {
        request_close(process.pid);
    }
    let editors_left = await_exit(&versions_root, &others, GRACEFUL_WINDOW)?;
    for process in &editors_left {
        terminate(process.pid);
    }
    retirement.editors_closed = others.len();

    Ok(retirement)
}

/// Returns the subset still running from the same versioned executable once
/// the window closes.
///
/// A PID is not an identity: Windows may reuse it immediately after Forge
/// exits. Re-running the existing executable-path discovery means a process
/// that inherited a stale Forge PID cannot turn a completed orderly shutdown
/// into an unnecessary `--force` requirement.
fn await_exit(
    versions_root: &Path,
    processes: &[&RunningProcess],
    window: Duration,
) -> Result<Vec<RunningProcess>> {
    let deadline = Instant::now() + window;
    loop {
        let discovered = discover(versions_root)?;
        let alive = still_running(processes, &discovered);
        if alive.is_empty() || Instant::now() >= deadline {
            return Ok(alive);
        }
        sleep(POLL_INTERVAL);
    }
}

fn still_running(
    expected: &[&RunningProcess],
    discovered: &[RunningProcess],
) -> Vec<RunningProcess> {
    expected
        .iter()
        .filter(|process| {
            discovered
                .iter()
                .any(|candidate| same_process(candidate, process))
        })
        .map(|process| (*process).clone())
        .collect()
}

/// The discovery snapshot identifies a process by both its PID and its loaded
/// versioned executable. On Windows executable paths are case-insensitive.
fn same_process(candidate: &RunningProcess, expected: &RunningProcess) -> bool {
    candidate.pid == expected.pid && same_executable(&candidate.executable, &expected.executable)
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

fn exact_stop_arguments(pid: u32, if_idle: bool) -> Vec<String> {
    let mut arguments = vec!["stop".into(), "--pid".into(), pid.to_string()];
    if if_idle {
        arguments.push("--if-idle".into());
    }
    arguments
}

#[cfg(windows)]
fn windows_discovery_script(versions_root: &Path) -> String {
    // Matching on the executable path rather than a process name catches every
    // top-level component of an old release regardless of what it is called.
    // Forge's embedded Windows process host deliberately runs from the same
    // executable, but it is an owned engine child rather than an authenticated
    // lifecycle controller. Stopping the controller retires that whole job;
    // addressing the child separately can only produce a false unauthenticated
    // Forge diagnostic while model work is live beneath it.
    format!(
        "Get-CimInstance Win32_Process | \
         Where-Object {{ $_.ExecutablePath -and \
           $_.ExecutablePath.StartsWith('{}', 'OrdinalIgnoreCase') -and \
           $_.CommandLine -notlike '*--artisan-internal-windows-process-host*' }} | \
         ForEach-Object {{ \"$($_.ProcessId)|$($_.ExecutablePath)\" }}",
        versions_root.display().to_string().replace('\'', "''")
    )
}

#[cfg(windows)]
fn discover(versions_root: &Path) -> Result<Vec<RunningProcess>> {
    let script = windows_discovery_script(versions_root);
    let output = background_command("powershell.exe")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ])
        .output()
        .map_err(InstallerError::CleanupHelper)?;

    Ok(parse_discovery(&String::from_utf8_lossy(&output.stdout)))
}

#[cfg(not(windows))]
fn discover(versions_root: &Path) -> Result<Vec<RunningProcess>> {
    let output = background_command("ps")
        .args(["-eo", "pid=,args="])
        .output()
        .map_err(InstallerError::CleanupHelper)?;
    let prefix = versions_root.display().to_string();

    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let (pid, command) = line.split_once(char::is_whitespace)?;
            let executable = command.split_whitespace().next()?;
            executable
                .starts_with(&prefix)
                .then(|| RunningProcess {
                    pid: pid.trim().parse().ok()?,
                    executable: PathBuf::from(executable),
                })
                .flatten()
        })
        .collect())
}

/// Splits the `pid|path` lines the Windows discovery emits.
#[cfg_attr(not(windows), allow(dead_code))]
fn parse_discovery(output: &str) -> Vec<RunningProcess> {
    output
        .lines()
        .filter_map(|line| {
            let (pid, executable) = line.trim().split_once('|')?;
            Some(RunningProcess {
                pid: pid.trim().parse().ok()?,
                executable: PathBuf::from(executable.trim()),
            })
        })
        .collect()
}

/// Asks a process to close its window and exit of its own accord.
fn request_close(pid: u32) {
    #[cfg(windows)]
    let _ = background_command("taskkill")
        .args(["/PID", &pid.to_string()])
        .output();
    #[cfg(not(windows))]
    let _ = background_command("kill")
        .args(["-TERM", &pid.to_string()])
        .output();
}

/// Ends a process that would not leave on its own.
fn terminate(pid: u32) {
    #[cfg(windows)]
    let _ = background_command("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .output();
    #[cfg(not(windows))]
    let _ = background_command("kill")
        .args(["-KILL", &pid.to_string()])
        .output();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_lines_become_processes_and_junk_is_ignored() {
        let parsed = parse_discovery(
            "1234|C:\\Artisan\\versions\\0.2.11\\editor\\Artisan Editor.exe\n\nnot-a-line\n7|C:\\Artisan\\versions\\0.2.11\\forge\\node.exe\n",
        );
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].pid, 1234);
        assert_eq!(parsed[1].pid, 7);
    }

    #[test]
    fn the_component_is_read_from_the_versioned_path() {
        let versions = Path::new("/Artisan/versions");
        let editor = RunningProcess {
            pid: 1,
            executable: PathBuf::from("/Artisan/versions/0.2.11/editor/Artisan Editor"),
        };
        let forge = RunningProcess {
            pid: 2,
            executable: PathBuf::from("/Artisan/versions/0.2.11/forge/host"),
        };
        assert_eq!(editor.component(versions).as_deref(), Some("editor"));
        assert_eq!(forge.component(versions).as_deref(), Some("forge"));
    }

    /// The incoming release's own processes are not staleness; retiring them
    /// would kill the very instance an update is about to hand back to.
    #[test]
    fn the_incoming_release_is_never_superseded() {
        let release = Path::new("/Artisan/versions/0.2.14");
        let current = RunningProcess {
            pid: 3,
            executable: PathBuf::from("/Artisan/versions/0.2.14/editor/Artisan Editor"),
        };
        let stale = RunningProcess {
            pid: 4,
            executable: PathBuf::from("/Artisan/versions/0.2.11/editor/Artisan Editor"),
        };
        assert!(current.executable.starts_with(release));
        assert!(!stale.executable.starts_with(release));
    }

    #[test]
    fn a_reused_pid_does_not_keep_a_retired_forge_alive() {
        let expected = RunningProcess {
            pid: 6172,
            executable: PathBuf::from("/Artisan/versions/0.2.27/forge/Artisan Forge"),
        };
        let reused = RunningProcess {
            pid: 6172,
            executable: PathBuf::from("/Windows/System32/notepad.exe"),
        };
        assert!(still_running(&[&expected], &[reused]).is_empty());
    }

    #[test]
    fn the_same_versioned_executable_remains_a_live_identity() {
        let expected = RunningProcess {
            pid: 6172,
            executable: PathBuf::from("/Artisan/versions/0.2.27/forge/Artisan Forge"),
        };
        let discovered = expected.clone();
        assert!(same_process(&discovered, &expected));
    }

    #[test]
    fn orderly_retirement_targets_the_discovered_forge_pid() {
        assert_eq!(
            exact_stop_arguments(6172, true),
            ["stop", "--pid", "6172", "--if-idle"].map(str::to_owned)
        );
        assert_eq!(
            exact_stop_arguments(6172, false),
            ["stop", "--pid", "6172"].map(str::to_owned)
        );
    }

    #[test]
    fn only_an_authenticated_busy_report_cancels_safe_retirement() {
        assert_eq!(
            forge_stop_disposition(Some(FORGE_BUSY_EXIT_CODE)),
            ForgeStopDisposition::Busy
        );
        assert_eq!(
            forge_stop_disposition(Some(FORGE_ACTIVITY_UNAVAILABLE_EXIT_CODE)),
            ForgeStopDisposition::Unresponsive
        );
        assert_eq!(
            forge_stop_disposition(Some(0)),
            ForgeStopDisposition::Accepted
        );
        assert_eq!(
            forge_stop_disposition(Some(1)),
            ForgeStopDisposition::Failed
        );
    }

    #[cfg(windows)]
    #[test]
    fn discovery_keeps_the_owned_engine_host_out_of_forge_control() {
        let script = windows_discovery_script(Path::new("C:/Artisan/versions"));

        assert!(script.contains("--artisan-internal-windows-process-host"));
        assert!(script.contains("-notlike"));
    }
}
