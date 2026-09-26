//! Launching the installed Editor with bounded startup confirmation.
//!
//! The launcher spawns the **installed** Editor (`<root>/versions/<v>`),
//! never a build-output binary, on the host the runner registered. After
//! spawning, it waits for the opt-in startup receipt the Editor's own
//! transport service writes once authenticated initial queries complete —
//! the existing startup signal, not a separate probe. On failure or timeout
//! the launcher stops the Editor it started and reports an honest stage
//! result instead of an exit-code wait.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::{Child, Stdio},
    time::{Duration, Instant},
};

use crate::{
    error::DevError,
    paths::{DEV_HOME_ENV, DevPaths, STRIPPED_DEV_HOME_ENV, STRIPPED_DEV_READY_ENV, exe_name},
};

/// Environment variable selecting the Editor's startup receipt file.
///
/// Must match `dev_startup_receipt::STARTUP_RECEIPT_ENV` in
/// `modules/frontend`; the contract test pins the literal on both sides.
pub const STARTUP_RECEIPT_ENV: &str = "ARTISAN_DEV_STARTUP_RECEIPT";

/// Schema marker the Editor writes into every receipt.
pub const STARTUP_RECEIPT_SCHEMA: &str = "artisan-dev-startup-v1";

/// Bounded wait for the Editor's startup receipt, in milliseconds.
///
/// A cold Forge can take well over 30 seconds to initialize durable state
/// before the first authenticated queries complete.
pub const DEV_STARTUP_TIMEOUT_MS: u64 = 90_000;

/// Poll interval while waiting for the startup receipt, in milliseconds.
pub const DEV_STARTUP_POLL_MS: u64 = 100;

/// Maximum receipts-stage text retained for diagnostics.
pub const MAX_RECEIPT_TEXT: usize = 256;

/// Library directories the Nix-built Linux Editor loads at runtime (Vulkan,
/// Wayland, X11, fonts), recorded when the Linux runner is built.
const GRAPHICS_LIBRARY_PATH: Option<&str> = option_env!("ARTISAN_DEV_GRAPHICS_LIBRARY_PATH");

/// Host driver directories added when present (on WSL and on `NixOS`).
const HOST_DRIVER_DIRECTORIES: [&str; 2] = ["/usr/lib/wsl/lib", "/run/opengl-driver/lib"];

/// Per-launch receipt path inside the dev directory.
///
/// The process identity makes each launch's receipt unique, so a stale
/// receipt from a crashed run can never confirm a new launch.
#[must_use]
pub fn fresh_receipt_path(paths: &DevPaths) -> PathBuf {
    paths
        .runner_dir()
        .join(format!("startup-receipt-{}.json", std::process::id()))
}

/// Removes a stale receipt before spawning the Editor.
///
/// A missing file is the expected first-launch state. Any other removal
/// failure aborts the launch: silently waiting on an unreadable path
/// would read a stale `ready` first.
///
/// # Errors
///
/// Returns [`DevError::Stage`] when an existing receipt cannot be removed.
pub fn clear_stale_receipt(path: &Path) -> Result<(), DevError> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(DevError::Stage {
            stage: "launch",
            reason: format!(
                "cannot clear stale startup receipt {}: {error}",
                path.display()
            ),
        }),
    }
}

/// Installed Editor binary of one version root.
#[must_use]
pub fn staged_editor(version_root: &Path) -> PathBuf {
    version_root.join("bin").join(exe_name("editor"))
}

/// Installed Forge binary of one version root.
#[must_use]
pub fn staged_forge(version_root: &Path) -> PathBuf {
    version_root.join("bin").join(exe_name("forge"))
}

/// Outcome of waiting for the startup receipt.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StartupWait {
    /// The Editor confirmed authenticated initial-query completion.
    Ready {
        /// Receipt stage text.
        stage: String,
    },
    /// The Editor reported a startup failure.
    Failed {
        /// Secret-free failure stage.
        stage: String,
        /// Secret-free failure detail.
        reason: String,
    },
    /// No receipt arrived before the deadline.
    Timeout,
    /// The Editor exited before confirming startup.
    EditorExited {
        /// Exit code when available.
        code: Option<i32>,
    },
}

/// Where the launched Editor's standard streams go.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EditorOutput {
    /// The runner's own streams: an interactive terminal, or `--attach`,
    /// where the runner lives exactly as long as the Editor anyway.
    Inherit,
    /// Detached from the runner's streams. A caller that reads the runner
    /// through a pipe (an agent shell, `| tee`, CI) waits for end-of-file,
    /// which never comes while a long-lived Editor holds the write end, so
    /// the run looks hung long after the runner returned. On Unix the
    /// Editor writes to this log instead; on Windows every inheritable
    /// handle leaks into a child regardless of its standard streams, so the
    /// Editor is started through the shell, which inherits none, and its
    /// output is not captured.
    Detached {
        /// Per-root Editor log (Unix).
        log: PathBuf,
    },
}

/// Editor output log inside the runner directory.
#[must_use]
pub fn editor_log_path(paths: &DevPaths) -> PathBuf {
    paths.runner_dir().join("editor.log")
}

/// Chooses the Editor's output routing for one launch.
///
/// Only an attached run, or a detached run whose stdout and stderr are both
/// terminals, may hand the Editor the runner's streams: a terminal has no
/// end-of-file for anyone to wait on.
#[must_use]
pub fn editor_output(attach: bool, streams_are_terminals: bool, log: PathBuf) -> EditorOutput {
    if attach || streams_are_terminals {
        EditorOutput::Inherit
    } else {
        EditorOutput::Detached { log }
    }
}

/// Whether the runner's stdout and stderr are both interactive terminals.
#[must_use]
pub fn streams_are_terminals() -> bool {
    use std::io::IsTerminal as _;

    std::io::stdout().is_terminal() && std::io::stderr().is_terminal()
}

/// Bound for the Windows shell launch to report the Editor's process id.
pub const DEV_LAUNCH_REPORT_TIMEOUT_MS: u64 = 30_000;

/// A launched dev Editor under the runner's control.
///
/// `child` is the Editor itself, except for a detached Windows launch, where
/// it is the watcher that started the Editor through the shell and exits
/// with the Editor's exit code.
#[derive(Debug)]
pub struct EditorProcess {
    child: Child,
    pid: u32,
    watcher: bool,
}

impl EditorProcess {
    /// Wraps an Editor child spawned directly.
    #[must_use]
    pub fn direct(child: Child) -> Self {
        let pid = child.id();
        Self {
            child,
            pid,
            watcher: false,
        }
    }

    /// The Editor's process id.
    #[must_use]
    pub const fn pid(&self) -> u32 {
        self.pid
    }

    /// The process whose exit reports the Editor's exit.
    pub const fn child_mut(&mut self) -> &mut Child {
        &mut self.child
    }

    /// Follows the Editor until it exits.
    ///
    /// # Errors
    ///
    /// Returns the wait failure.
    pub fn wait(mut self) -> std::io::Result<std::process::ExitStatus> {
        self.child.wait()
    }

    /// Leaves a confirmed Editor running on its own.
    ///
    /// A Windows watcher still holds the runner's inherited handles, so it is
    /// stopped here; the Editor it started owns none of them and keeps
    /// running. A direct child is simply no longer followed.
    pub fn release(mut self) {
        if self.watcher {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    /// Stops the Editor after unconfirmed startup and returns its exit code
    /// when known.
    #[must_use]
    pub fn stop(mut self) -> Option<i32> {
        if self.watcher {
            terminate_pid(self.pid);
        } else {
            let _ = self.child.kill();
        }
        self.child.wait().ok().and_then(|status| status.code())
    }
}

/// Spawns the installed Editor with `arguments`.
///
/// The child inherits the environment with `ARTISAN_HOME` pointed at the
/// dev installation, the manual-forge escape hatches removed, and the
/// startup receipt path set; it writes the receipt once its first host
/// connection completes the initial queries. The Linux Editor also gets the
/// graphics library path it loads its drivers from. `output` routes its
/// standard streams (see [`EditorOutput`]).
///
/// A detached Editor leaves the runner's process group on Unix and, on
/// Windows, breaks away from the runner's job object: a runner started
/// through WSL interop runs in a job that terminates its processes when the
/// WSL session ends.
///
/// # Errors
///
/// Returns [`DevError::Stage`] when the log cannot be created or the Editor
/// cannot be spawned.
pub fn spawn_editor(
    editor_exe: &Path,
    home: &Path,
    receipt_path: &Path,
    output: &EditorOutput,
    arguments: &[OsString],
) -> Result<EditorProcess, DevError> {
    let cannot_launch = || DevError::Stage {
        stage: "launch",
        reason: format!("cannot launch {}", editor_exe.display()),
    };
    match output {
        EditorOutput::Inherit => {
            let mut command = std::process::Command::new(editor_exe);
            configure_editor_environment(&mut command, home, receipt_path);
            command
                .args(arguments)
                .stdin(Stdio::inherit())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .spawn()
                .map(EditorProcess::direct)
                .map_err(|_| cannot_launch())
        }
        EditorOutput::Detached { log } => {
            spawn_detached(editor_exe, home, receipt_path, log, arguments)
        }
    }
}

/// Applies the dev launch environment to a command that starts the Editor.
fn configure_editor_environment(
    command: &mut std::process::Command,
    home: &Path,
    receipt_path: &Path,
) {
    command
        .env(DEV_HOME_ENV, home)
        .env(STARTUP_RECEIPT_ENV, receipt_path)
        .env_remove(STRIPPED_DEV_HOME_ENV)
        .env_remove(STRIPPED_DEV_READY_ENV);
    if cfg!(unix)
        && let Some(path) = editor_library_path(
            GRAPHICS_LIBRARY_PATH,
            std::env::var("LD_LIBRARY_PATH").ok().as_deref(),
            |directory| Path::new(directory).is_dir(),
        )
    {
        command.env("LD_LIBRARY_PATH", path);
    }
}

/// The Linux Editor's `LD_LIBRARY_PATH`: the recorded graphics libraries,
/// the caller's own entries, then host driver directories that exist.
/// `None` for a runner built without recorded graphics libraries.
#[must_use]
pub fn editor_library_path(
    graphics: Option<&str>,
    inherited: Option<&str>,
    exists: impl Fn(&str) -> bool,
) -> Option<String> {
    let graphics = graphics.filter(|path| !path.is_empty())?;
    let mut entries = vec![graphics.to_owned()];
    entries.extend(inherited.filter(|path| !path.is_empty()).map(str::to_owned));
    entries.extend(
        HOST_DRIVER_DIRECTORIES
            .iter()
            .filter(|directory| exists(directory))
            .map(|directory| (*directory).to_owned()),
    );
    Some(entries.join(":"))
}

/// Detached Unix launch: the Editor writes to the per-root log. Rust opens
/// every descriptor close-on-exec and the child's standard descriptors are
/// replaced, so the caller's pipe does not survive into the Editor.
#[cfg(not(windows))]
fn spawn_detached(
    editor_exe: &Path,
    home: &Path,
    receipt_path: &Path,
    log: &Path,
    arguments: &[OsString],
) -> Result<EditorProcess, DevError> {
    use std::os::unix::process::CommandExt as _;

    let log_error = |_| DevError::Stage {
        stage: "launch",
        reason: format!("cannot open the editor log {}", log.display()),
    };
    let stdout = std::fs::File::create(log).map_err(log_error)?;
    let stderr = stdout.try_clone().map_err(log_error)?;
    let mut command = std::process::Command::new(editor_exe);
    configure_editor_environment(&mut command, home, receipt_path);
    // Its own process group: Ctrl-C and hangups aimed at the terminal job
    // that ran the runner do not reach the Editor.
    command
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .process_group(0)
        .spawn()
        .map(EditorProcess::direct)
        .map_err(|_| DevError::Stage {
            stage: "launch",
            reason: format!("cannot launch {}", editor_exe.display()),
        })
}

/// Environment variables carrying the Editor path and its command line to
/// the Windows watcher, so nothing is ever spliced into the script text.
#[cfg(windows)]
const LAUNCH_EXE_ENV: &str = "ARTISAN_DEV_LAUNCH_EXE";
#[cfg(windows)]
const LAUNCH_ARGUMENTS_ENV: &str = "ARTISAN_DEV_LAUNCH_ARGUMENTS";

/// Windows watcher script: starts the Editor through the shell
/// (`ShellExecuteEx` inherits no handles but passes this environment) with
/// the pre-quoted command line, reports its process id, then exits with its
/// exit code.
#[cfg(windows)]
const WATCHER_SCRIPT: &str = "$exe = $env:ARTISAN_DEV_LAUNCH_EXE; \
    $arguments = $env:ARTISAN_DEV_LAUNCH_ARGUMENTS; \
    Remove-Item Env:ARTISAN_DEV_LAUNCH_EXE; \
    Remove-Item Env:ARTISAN_DEV_LAUNCH_ARGUMENTS; \
    if ($arguments) { $p = Start-Process -FilePath $exe -ArgumentList $arguments -PassThru } \
    else { $p = Start-Process -FilePath $exe -PassThru }; \
    $null = $p.Handle; \
    [Console]::Out.WriteLine($p.Id); [Console]::Out.Flush(); \
    $p.WaitForExit(); exit $p.ExitCode";

/// `CREATE_NO_WINDOW`: the watcher never shows a console.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// `CREATE_BREAKAWAY_FROM_JOB`: a runner started through WSL interop runs in
/// a job that ends its processes with the WSL session; the watcher, and so
/// the Editor it starts, leave that job.
#[cfg(windows)]
const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

/// Windows PowerShell, which every supported Windows ships.
#[cfg(windows)]
fn windows_powershell() -> Option<PathBuf> {
    let root = std::env::var_os("SystemRoot").map(PathBuf::from)?;
    let path = root
        .join("System32")
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe");
    path.is_file().then_some(path)
}

/// One Windows command line from `arguments`, quoted the way the C runtime
/// splits it back (backslashes double only before a quote).
#[must_use]
pub fn windows_command_line(arguments: &[OsString]) -> String {
    arguments
        .iter()
        .map(|argument| {
            let argument = argument.to_string_lossy();
            if !argument.is_empty() && !argument.contains([' ', '\t', '"']) {
                return argument.into_owned();
            }
            let mut quoted = String::from("\"");
            let mut backslashes = 0;
            for character in argument.chars() {
                match character {
                    '\\' => backslashes += 1,
                    '"' => {
                        quoted.push_str(&"\\".repeat(backslashes * 2 + 1));
                        quoted.push('"');
                        backslashes = 0;
                    }
                    character => {
                        quoted.push_str(&"\\".repeat(backslashes));
                        quoted.push(character);
                        backslashes = 0;
                    }
                }
            }
            quoted.push_str(&"\\".repeat(backslashes * 2));
            quoted.push('"');
            quoted
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Detached Windows launch through the shell.
///
/// `CreateProcess` hands a child every inheritable handle of its parent,
/// including the runner's own standard handles, whatever the child's
/// standard streams are set to; the only safe way to start a process that
/// holds none of them is the shell. The watcher that asks the shell does
/// hold them, and is stopped once startup resolves.
#[cfg(windows)]
fn spawn_detached(
    editor_exe: &Path,
    home: &Path,
    receipt_path: &Path,
    _log: &Path,
    arguments: &[OsString],
) -> Result<EditorProcess, DevError> {
    use std::os::windows::process::CommandExt as _;

    let cannot_launch = |detail: &str| DevError::Stage {
        stage: "launch",
        reason: format!("cannot launch {}: {detail}", editor_exe.display()),
    };
    let powershell = windows_powershell().ok_or_else(|| cannot_launch("no Windows PowerShell"))?;
    let mut command = std::process::Command::new(powershell);
    configure_editor_environment(&mut command, home, receipt_path);
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            WATCHER_SCRIPT,
        ])
        .env(LAUNCH_EXE_ENV, editor_exe)
        .env(LAUNCH_ARGUMENTS_ENV, windows_command_line(arguments))
        .creation_flags(CREATE_NO_WINDOW | CREATE_BREAKAWAY_FROM_JOB)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command
        .spawn()
        .map_err(|_| cannot_launch("the launcher did not start"))?;
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(cannot_launch("the launcher has no output"));
    };
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = std::io::BufRead::read_line(&mut std::io::BufReader::new(stdout), &mut line);
        let _ = sender.send(line);
    });
    let reported = receiver
        .recv_timeout(Duration::from_millis(DEV_LAUNCH_REPORT_TIMEOUT_MS))
        .ok()
        .and_then(|line| line.trim().parse::<u32>().ok())
        .filter(|pid| *pid != 0);
    let Some(pid) = reported else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(cannot_launch(&format!(
            "the shell did not report the editor process within {}s",
            DEV_LAUNCH_REPORT_TIMEOUT_MS / 1_000
        )));
    };
    Ok(EditorProcess {
        child,
        pid,
        watcher: true,
    })
}

/// Terminates one process by id, best-effort.
#[cfg(windows)]
fn terminate_pid(pid: u32) {
    let taskkill = std::env::var_os("SystemRoot")
        .map(|root| PathBuf::from(root).join("System32").join("taskkill.exe"));
    if let Some(taskkill) = taskkill {
        let _ = std::process::Command::new(taskkill)
            .args(["/PID", &pid.to_string(), "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// Unix launches never use a watcher.
#[cfg(not(windows))]
const fn terminate_pid(_: u32) {}

/// Reads one receipt file without blocking.
///
/// Returns `None` when the file is absent or not yet a valid receipt
/// document; the caller keeps polling until the deadline.
#[must_use]
pub fn read_receipt(path: &Path) -> Option<StartupWait> {
    let bytes = std::fs::read(path).ok()?;
    let document: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    if document.get("schema")?.as_str()? != STARTUP_RECEIPT_SCHEMA {
        return None;
    }
    let stage = document
        .get("stage")?
        .as_str()?
        .chars()
        .take(MAX_RECEIPT_TEXT)
        .collect::<String>();
    match document.get("status")?.as_str()? {
        "ready" => Some(StartupWait::Ready { stage }),
        "failed" => {
            let reason = document
                .get("detail")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("startup failed")
                .chars()
                .take(MAX_RECEIPT_TEXT)
                .collect::<String>();
            Some(StartupWait::Failed { stage, reason })
        }
        _ => None,
    }
}

/// Waits for the startup receipt while the Editor is alive.
///
/// Polls the receipt file until it parses, the Editor exits, or the
/// deadline passes. Never consumes Forge credentials: the receipt is the
/// Editor's own startup signal.
pub fn wait_for_startup(child: &mut Child, receipt_path: &Path, timeout: Duration) -> StartupWait {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(outcome) = read_receipt(receipt_path) {
            return outcome;
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                return StartupWait::EditorExited {
                    code: status.code(),
                };
            }
            Ok(None) => {}
            Err(_) => {
                return StartupWait::EditorExited { code: None };
            }
        }
        if Instant::now() >= deadline {
            return StartupWait::Timeout;
        }
        std::thread::sleep(
            Duration::from_millis(DEV_STARTUP_POLL_MS).min(
                deadline
                    .checked_duration_since(Instant::now())
                    .unwrap_or(Duration::ZERO),
            ),
        );
    }
}
