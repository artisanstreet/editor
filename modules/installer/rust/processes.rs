use std::{
    ffi::OsStr,
    path::{Component, Path, PathBuf},
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

/// The only versioned processes the installer owns during retirement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProcessRole {
    Editor,
    Forge,
}

/// One running process launched out of the installation's `versions` tree.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunningProcess {
    pub pid: u32,
    pub executable: PathBuf,
}

impl RunningProcess {
    /// Native payloads have one owned executable per role at
    /// `versions/<version>/bin/<role>[.exe]`. Everything else is deliberately
    /// outside retirement ownership, including helpers and legacy layouts.
    fn role(&self, versions_root: &Path) -> Option<ProcessRole> {
        classify_executable(&self.executable, versions_root, cfg!(windows))
    }
}

fn classify_executable(
    executable: &Path,
    versions_root: &Path,
    windows: bool,
) -> Option<ProcessRole> {
    let mut components = executable.strip_prefix(versions_root).ok()?.components();
    let Some(Component::Normal(version)) = components.next() else {
        return None;
    };
    if version.is_empty() {
        return None;
    }
    let Some(Component::Normal(bin)) = components.next() else {
        return None;
    };
    if bin != OsStr::new("bin") {
        return None;
    }
    let Some(Component::Normal(leaf)) = components.next() else {
        return None;
    };
    if components.next().is_some() {
        return None;
    }

    native_role_for_leaf(leaf, windows)
}

fn native_role_for_leaf(leaf: &OsStr, windows: bool) -> Option<ProcessRole> {
    let leaf = leaf.to_str()?;
    if windows {
        if leaf.eq_ignore_ascii_case("editor.exe") {
            return Some(ProcessRole::Editor);
        }
        if leaf.eq_ignore_ascii_case("forge.exe") {
            return Some(ProcessRole::Forge);
        }
        return None;
    }

    match leaf {
        "editor" => Some(ProcessRole::Editor),
        "forge" => Some(ProcessRole::Forge),
        _ => None,
    }
}

fn partition_by_role<'a>(
    processes: &'a [RunningProcess],
    versions_root: &Path,
) -> (Vec<&'a RunningProcess>, Vec<&'a RunningProcess>) {
    let mut forges = Vec::new();
    let mut editors = Vec::new();
    for process in processes {
        match process.role(versions_root) {
            Some(ProcessRole::Forge) => forges.push(process),
            Some(ProcessRole::Editor) => editors.push(process),
            None => {}
        }
    }
    (forges, editors)
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
/// The native Editor may hold a single-instance lock: launching the new editor
/// while an old one runs exits immediately and focuses the stale window,
/// leaving a user staring at the previous build with `active_version` already
/// flipped — an update that silently did nothing. The Editor is closed
/// gracefully and then forced; Forge owns live agent runs and the database
/// lock, so an authenticated active-work report cancels retirement unless
/// `--force` was explicit. An unresponsive Forge is treated as broken and
/// retired so it cannot wedge every future build. Forge is gated first so a
/// busy refusal leaves the Editor open around its work.
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
    let (forges, editors) = partition_by_role(&superseded, &versions_root);

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

    for process in &editors {
        request_close(process.pid);
    }
    let editors_left = await_exit(&versions_root, &editors, GRACEFUL_WINDOW)?;
    for process in &editors_left {
        terminate(process.pid);
    }
    retirement.editors_closed = editors.len();

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
    // Project the native executable path only. The typed classifier below owns
    // the exact `bin/editor.exe` and `bin/forge.exe` leaves; process names and
    // command-line arguments are not part of retirement identity.
    format!(
        "Get-CimInstance Win32_Process | \
         Where-Object {{ $_.ExecutablePath -and \
           $_.ExecutablePath.StartsWith('{}' + [System.IO.Path]::DirectorySeparatorChar, 'OrdinalIgnoreCase') }} | \
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
    // GNU/procps and compatible implementations expose the native executable
    // path without argv data through `exe`. Keeping this projection argument
    // free preserves paths such as `/opt/Artisan Street` exactly.
    let output = background_command("ps")
        .args(["-eo", "pid=,exe="])
        .output()
        .map_err(InstallerError::CleanupHelper)?;
    if output.status.success() {
        return Ok(parse_executable_projection(
            &String::from_utf8_lossy(&output.stdout),
            versions_root,
        ));
    }

    // Some BSD/System V ps dialects do not implement `exe`. Their `args`
    // projection is ambiguous in general, so the bounded fallback below only
    // reconstructs a candidate when the exact versions root, one version
    // segment, `bin`, and one native role leaf are present. It never stores an
    // arguments suffix as executable identity; an unparseable row is ignored.
    let output = background_command("ps")
        .args(["-eo", "pid=,args="])
        .output()
        .map_err(InstallerError::CleanupHelper)?;
    if !output.status.success() {
        return Ok(Vec::new());
    }

    Ok(parse_ps_args_discovery(
        &String::from_utf8_lossy(&output.stdout),
        versions_root,
    ))
}

#[cfg(not(windows))]
fn parse_executable_projection(output: &str, versions_root: &Path) -> Vec<RunningProcess> {
    output
        .lines()
        .filter_map(|line| {
            let (pid, executable) = split_pid_and_field(line)?;
            let executable = PathBuf::from(executable);
            executable
                .starts_with(versions_root)
                .then_some(RunningProcess { pid, executable })
        })
        .collect()
}

#[cfg(not(windows))]
fn parse_ps_args_discovery(output: &str, versions_root: &Path) -> Vec<RunningProcess> {
    output
        .lines()
        .filter_map(|line| {
            let (pid, command) = split_pid_and_field(line)?;
            Some(RunningProcess {
                pid,
                executable: executable_from_ps_args(command, versions_root)?,
            })
        })
        .collect()
}

#[cfg(not(windows))]
fn split_pid_and_field(line: &str) -> Option<(u32, &str)> {
    let (pid, field) = line.trim().split_once(char::is_whitespace)?;
    Some((pid.trim().parse().ok()?, field.trim()))
}

#[cfg(not(windows))]
fn executable_from_ps_args(command: &str, versions_root: &Path) -> Option<PathBuf> {
    let root = versions_root.to_str()?;
    let relative = command.strip_prefix(root)?.strip_prefix('/')?;
    let bin = relative.find("/bin/")?;
    let version = &relative[..bin];
    if version.is_empty() || version.contains('/') {
        return None;
    }

    let leaf_and_arguments = &relative[bin + "/bin/".len()..];
    for leaf in ["editor", "forge"] {
        let Some(remainder) = leaf_and_arguments.strip_prefix(leaf) else {
            continue;
        };
        if remainder.is_empty() || remainder.chars().next().is_some_and(char::is_whitespace) {
            return Some(versions_root.join(version).join("bin").join(leaf));
        }
    }

    None
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

    fn process(pid: u32, executable: &str) -> RunningProcess {
        RunningProcess {
            pid,
            executable: PathBuf::from(executable),
        }
    }

    #[test]
    fn discovery_lines_become_processes_and_junk_is_ignored() {
        let parsed = parse_discovery(
            "1234|C:\\Artisan\\versions\\0.2.11\\bin\\editor.exe\n\nnot-a-line\n7|C:\\Artisan\\versions\\0.2.11\\bin\\forge.exe\n",
        );
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].pid, 1234);
        assert_eq!(parsed[1].pid, 7);
    }

    #[test]
    fn native_roles_require_the_exact_three_component_layout() {
        let versions = Path::new("/Artisan/versions");

        assert_eq!(
            classify_executable(
                Path::new("/Artisan/versions/0.2.11/bin/editor.exe"),
                versions,
                true,
            ),
            Some(ProcessRole::Editor)
        );
        assert_eq!(
            classify_executable(
                Path::new("/Artisan/versions/0.2.11/bin/FORGE.EXE"),
                versions,
                true,
            ),
            Some(ProcessRole::Forge)
        );
        assert_eq!(
            classify_executable(
                Path::new("/Artisan/versions/0.2.11/bin/editor"),
                versions,
                false,
            ),
            Some(ProcessRole::Editor)
        );
        assert_eq!(
            classify_executable(
                Path::new("/Artisan/versions/0.2.11/bin/forge"),
                versions,
                false,
            ),
            Some(ProcessRole::Forge)
        );
    }

    #[test]
    fn leaf_case_rules_are_platform_specific() {
        let versions = Path::new("/Artisan/versions");
        assert_eq!(
            classify_executable(
                Path::new("/Artisan/versions/0.2.11/bin/EdItOr.ExE"),
                versions,
                true,
            ),
            Some(ProcessRole::Editor)
        );
        assert_eq!(
            classify_executable(
                Path::new("/Artisan/versions/0.2.11/bin/EdItOr"),
                versions,
                false,
            ),
            None
        );
        assert_eq!(
            classify_executable(
                Path::new("/Artisan/versions/0.2.11/bin/editor.exe"),
                versions,
                false,
            ),
            None
        );
        assert_eq!(
            classify_executable(
                Path::new("/Artisan/versions/0.2.11/bin/EDITOR"),
                versions,
                false,
            ),
            None
        );
    }

    #[test]
    fn non_native_paths_remain_unclassified() {
        let versions = Path::new("/Artisan/versions");
        let windows_rejections = [
            "/Artisan/versions/0.2.11/bin/ae.exe",
            "/Artisan/versions/0.2.11/bin/installer.exe",
            "/Artisan/versions/0.2.11/bin/readme.exe",
            "/Artisan/versions/0.2.11/bin/editor/child.exe",
            "/Artisan/versions/0.2.11/editor/Artisan Editor.exe",
            "/Artisan/versions/0.2.11/forge/forge.exe",
            "/Artisan/versions/0.2.11/bin/Artisan Editor.exe",
            "/Artisan/versions/0.2.11/bin/node.exe",
            "/Artisan/versions/0.2.11/bin/broker.exe",
            "/Artisan/versions-old/0.2.11/bin/editor.exe",
            "/Artisan/versions-sibling/0.2.11/bin/forge.exe",
            "/Other/versions/0.2.11/bin/editor.exe",
        ];
        for path in windows_rejections {
            assert_eq!(
                classify_executable(Path::new(path), versions, true),
                None,
                "classified non-native Windows path: {path}"
            );
        }

        let non_windows_rejections = [
            "/Artisan/versions/0.2.11/bin/ae",
            "/Artisan/versions/0.2.11/bin/installer",
            "/Artisan/versions/0.2.11/bin/readme",
            "/Artisan/versions/0.2.11/bin/editor/child",
            "/Artisan/versions/0.2.11/editor/display-name",
            "/Artisan/versions/0.2.11/forge/node",
            "/Artisan/versions/0.2.11/bin/Artisan Editor",
            "/Artisan/versions/0.2.11/bin/node",
            "/Artisan/versions/0.2.11/bin/broker",
            "/Artisan/versions-old/0.2.11/bin/editor",
            "/Artisan/versions-sibling/0.2.11/bin/forge",
            "/Other/versions/0.2.11/bin/editor",
        ];
        for path in non_windows_rejections {
            assert_eq!(
                classify_executable(Path::new(path), versions, false),
                None,
                "classified non-native non-Windows path: {path}"
            );
        }
    }

    /// The incoming release's own processes are not staleness; retiring them
    /// would kill the very instance an update is about to hand back to.
    #[test]
    fn the_incoming_release_is_never_superseded() {
        let release = Path::new("/Artisan/versions/0.2.14");
        let current = process(3, "/Artisan/versions/0.2.14/bin/editor");
        let stale = process(4, "/Artisan/versions/0.2.11/bin/editor");
        assert!(current.executable.starts_with(release));
        assert!(!stale.executable.starts_with(release));
        assert!(!Path::new("/Artisan/versions/0.2.140").starts_with(release));
    }

    #[test]
    fn retirement_partition_contains_only_native_forge_and_editor_processes() {
        let versions = Path::new("/Artisan/versions");
        let suffix = if cfg!(windows) { ".exe" } else { "" };
        let processes = vec![
            process(1, &format!("/Artisan/versions/0.2.11/bin/forge{suffix}")),
            process(2, &format!("/Artisan/versions/0.2.11/bin/editor{suffix}")),
            process(3, &format!("/Artisan/versions/0.2.11/bin/ae{suffix}")),
            process(
                4,
                &format!("/Artisan/versions/0.2.11/editor/display{suffix}"),
            ),
            process(
                5,
                &format!("/Artisan/versions/0.2.11/bin/forge/child{suffix}"),
            ),
        ];
        let (forges, editors) = partition_by_role(&processes, versions);
        assert_eq!(
            forges.iter().map(|process| process.pid).collect::<Vec<_>>(),
            vec![1]
        );
        assert_eq!(
            editors
                .iter()
                .map(|process| process.pid)
                .collect::<Vec<_>>(),
            vec![2]
        );
    }

    #[test]
    fn a_reused_pid_does_not_keep_a_retired_forge_alive() {
        let expected = process(6172, "/Artisan/versions/0.2.27/bin/forge");
        let reused = process(6172, "/Windows/System32/notepad.exe");
        assert!(still_running(&[&expected], &[reused]).is_empty());
    }

    #[test]
    fn the_same_versioned_executable_remains_a_live_identity() {
        let expected = process(6172, "/Artisan/versions/0.2.27/bin/forge");
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

    #[cfg(not(windows))]
    #[test]
    fn executable_projection_preserves_space_in_root_without_arguments() {
        let versions = Path::new("/opt/Artisan Street/versions");
        let parsed = parse_executable_projection(
            "1234 /opt/Artisan Street/versions/0.2.11/bin/editor\n7 /opt/Artisan Street/versions/0.2.11/bin/forge\n",
            versions,
        );
        assert_eq!(parsed.len(), 2);
        assert_eq!(
            parsed[0].executable,
            PathBuf::from("/opt/Artisan Street/versions/0.2.11/bin/editor")
        );
        assert_eq!(parsed[1].executable, versions.join("0.2.11/bin/forge"));
    }

    #[cfg(not(windows))]
    #[test]
    fn ps_args_fallback_recovers_native_path_before_arguments() {
        let versions = Path::new("/opt/Artisan Street/versions");
        let parsed = parse_ps_args_discovery(
            "1234 /opt/Artisan Street/versions/0.2.11/bin/editor --project '/tmp/with spaces'\n7 /opt/Artisan Street/versions/0.2.11/bin/forge --idle\n8 /opt/Artisan Street/versions/0.2.11/bin/editor.exe --wrong-extension\n9 /opt/Artisan Street/versions-old/0.2.11/bin/editor\n",
            versions,
        );
        assert_eq!(parsed.len(), 2);
        assert_eq!(
            parsed[0].executable,
            PathBuf::from("/opt/Artisan Street/versions/0.2.11/bin/editor")
        );
        assert_eq!(
            parsed[1].executable,
            PathBuf::from("/opt/Artisan Street/versions/0.2.11/bin/forge")
        );
        assert!(!parsed[0].executable.to_string_lossy().contains("--project"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_discovery_projects_only_native_executable_paths() {
        let script = windows_discovery_script(Path::new("C:/Artisan/versions"));

        assert!(script.contains("ExecutablePath"));
        assert!(script.contains("ProcessId"));
        assert!(!script.contains("CommandLine"));
    }
}
