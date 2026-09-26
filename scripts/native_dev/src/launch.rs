//! Launching the staged Editor with bounded startup confirmation.
//!
//! The launcher spawns the **installed** Editor (`<root>/versions/<v>`),
//! never a build-output binary. Superseded dev instances are retired by the
//! installer before activation, so no live Forge runs the old binaries. After spawning, it waits
//! for the opt-in startup receipt the Editor's own transport service
//! writes once authenticated initial queries complete — the existing
//! startup signal, not a separate probe, so no bootstrap credential is
//! ever consumed outside the owned session. On failure or timeout the
//! launcher stops the Editor it owns (which releases the owned Forge
//! through the Editor's lease and Job Object containment) and reports an
//! honest stage result instead of an exit-code wait.

use std::{
    path::{Path, PathBuf},
    process::{Child, Stdio},
    time::{Duration, Instant},
};

use artisan_editor_cli::process::{self, ForgeReadiness, ForgeReadinessStatus};

use crate::{
    error::DevError,
    paths::{
        DEV_HOME_ENV, DevPaths, OWNED_DEV_FORGE_ENV, STRIPPED_DEV_HOME_ENV, STRIPPED_DEV_READY_ENV,
        exe_name,
    },
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

/// Installed Forge binary of one version root, which the Editor's owned
/// startup launches.
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

/// Refuses staging when a previous dev Forge still owns the dev home.
///
/// A stale readiness receipt is fine: the staged Editor's owned startup
/// replaces it the way production does.
///
/// # Errors
///
/// Returns [`DevError::PreviousForgeRunning`] when the readiness receipt
/// identifies a live Forge running the staged binary.
pub fn refuse_live_forge(paths: &DevPaths, forge_exe: &Path) -> Result<(), DevError> {
    match process::readiness_status(&paths.readiness_path(), forge_exe) {
        ForgeReadinessStatus::Ready(readiness) => Err(DevError::PreviousForgeRunning {
            pid: readiness.pid(),
        }),
        ForgeReadinessStatus::Missing | ForgeReadinessStatus::Invalid => Ok(()),
    }
}

/// Outcome of pre-launch readiness reconciliation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadinessReconcile {
    /// No receipt file exists; only orphan publish temporaries were swept.
    Absent,
    /// A stale receipt naming a dead Forge was removed.
    CleanedStale {
        /// Dead Forge identity the stale receipt named.
        pid: u32,
    },
}

/// Bound for one readiness receipt read, matching the CLI bound.
const READINESS_MAX_BYTES: u64 = 4_096;

/// Windows reparse-point attribute.
///
/// Mirrors the backend custody checks without depending on the backend
/// crate; `symlink_metadata` preserves this attribute for links and
/// junctions so either can be rejected before removal.
#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

/// Windows flag that opens a reparse point itself instead of following it.
#[cfg(windows)]
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

/// Returns whether directory metadata describes a plain regular file.
///
/// Symlinks, reparse points, directories, and anything else fail closed:
/// only a directly owned regular file may ever be removed.
fn is_plain_file(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return false;
        }
    }
    true
}

/// Validates every ancestor of a removal target without resolving links.
///
/// Any symlink, reparse point, non-directory, missing, or uninspectable
/// ancestor fails closed: removal must never operate through a redirected
/// parent (for example a junction swapped in after staging). Mirrors the
/// backend custody parent checks without depending on the backend crate.
fn validate_parent_chain(path: &Path) -> Result<(), DevError> {
    let mut current = path;
    loop {
        let metadata = match std::fs::symlink_metadata(current) {
            Ok(metadata) => metadata,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                return Err(DevError::Stage {
                    stage: "launch",
                    reason: format!("readiness parent is missing: {}", current.display()),
                });
            }
            Err(_) => {
                return Err(DevError::Stage {
                    stage: "launch",
                    reason: format!("cannot inspect {}", current.display()),
                });
            }
        };
        if metadata.file_type().is_symlink() {
            return Err(DevError::Stage {
                stage: "launch",
                reason: format!(
                    "readiness parent is a symbolic link; preserved, remove it by hand: {}",
                    current.display()
                ),
            });
        }
        if is_reparse_point(&metadata) {
            return Err(DevError::Stage {
                stage: "launch",
                reason: format!(
                    "readiness parent is a reparse point; preserved, remove it by hand: {}",
                    current.display()
                ),
            });
        }
        if !metadata.is_dir() {
            return Err(DevError::Stage {
                stage: "launch",
                reason: format!(
                    "readiness parent is not a directory; preserved: {}",
                    current.display()
                ),
            });
        }
        let Some(next) = current.parent() else {
            break;
        };
        if next == current || next.as_os_str().is_empty() {
            break;
        }
        current = next;
    }
    Ok(())
}

/// Returns whether metadata describes a Windows reparse point.
#[cfg(windows)]
fn is_reparse_point(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

/// Reparse points do not exist outside Windows.
#[cfg(not(windows))]
fn is_reparse_point(_: &std::fs::Metadata) -> bool {
    false
}

/// Forge custody probe: a nonblocking exclusive OS lock on the home's
/// custody file, mirroring `ForgeProcessCustody::acquire` narrowly.
///
/// A live Forge holds this lock from startup until after application
/// shutdown, so acquiring it proves no live Forge owns this home — even
/// when pid queries are unavailable. The guard retains the exact file
/// through the readiness recheck and removal; dropping it releases
/// custody. It is never held across spawn: the new Forge must acquire
/// custody itself on startup. No backend dependency: only `fs2` and the
/// same shape checks the backend applies.
struct CustodyProbe {
    _file: std::fs::File,
}

/// Acquires the custody probe for one home.
///
/// # Errors
///
/// Returns [`DevError::CustodyHeld`] when another owner holds the lock,
/// and [`DevError::Stage`] for any unsafe or unexpected custody shape.
fn acquire_custody_probe(custody_path: &Path) -> Result<CustodyProbe, DevError> {
    let Some(parent) = custody_path.parent() else {
        return Err(DevError::Stage {
            stage: "launch",
            reason: format!("custody path has no parent: {}", custody_path.display()),
        });
    };
    validate_parent_chain(parent)?;
    let file = open_or_create_custody_file(custody_path)?;
    validate_open_custody_file(custody_path, &file)?;
    match fs2::FileExt::try_lock_exclusive(&file) {
        Ok(()) => Ok(CustodyProbe { _file: file }),
        Err(source) if is_lock_contention(&source) => Err(DevError::CustodyHeld {
            path: custody_path.to_path_buf(),
        }),
        Err(_) => Err(DevError::Stage {
            stage: "launch",
            reason: format!("cannot lock {}", custody_path.display()),
        }),
    }
}

/// Opens the pre-existing regular custody file, or creates it atomically.
fn open_or_create_custody_file(custody_path: &Path) -> Result<std::fs::File, DevError> {
    match std::fs::symlink_metadata(custody_path) {
        Ok(metadata) => {
            validate_custody_metadata(custody_path, &metadata)?;
            open_custody_file(custody_path)
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            create_custody_file(custody_path).or_else(|source| {
                if source.kind() == std::io::ErrorKind::AlreadyExists {
                    // A concurrent creator won the race: re-inspect before
                    // opening, never retry blindly.
                    match std::fs::symlink_metadata(custody_path) {
                        Ok(metadata) => {
                            validate_custody_metadata(custody_path, &metadata)?;
                            open_custody_file(custody_path)
                        }
                        Err(_) => Err(DevError::Stage {
                            stage: "launch",
                            reason: format!("cannot inspect {}", custody_path.display()),
                        }),
                    }
                } else {
                    Err(DevError::Stage {
                        stage: "launch",
                        reason: format!("cannot create {}", custody_path.display()),
                    })
                }
            })
        }
        Err(_) => Err(DevError::Stage {
            stage: "launch",
            reason: format!("cannot inspect {}", custody_path.display()),
        }),
    }
}

/// Creates the custody carrier without truncating any existing file.
fn create_custody_file(custody_path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.mode(0o600);
    }
    configure_no_reparse_open(&mut options);
    options.open(custody_path)
}

/// Opens the custody carrier read/write without truncating it.
fn open_custody_file(custody_path: &Path) -> Result<std::fs::File, DevError> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true);
    configure_no_reparse_open(&mut options);
    options.open(custody_path).map_err(|_| DevError::Stage {
        stage: "launch",
        reason: format!("cannot open {}", custody_path.display()),
    })
}

/// Verifies metadata from the retained descriptor as well as the path.
fn validate_open_custody_file(custody_path: &Path, file: &std::fs::File) -> Result<(), DevError> {
    let metadata = file.metadata().map_err(|_| DevError::Stage {
        stage: "launch",
        reason: format!("cannot inspect {}", custody_path.display()),
    })?;
    validate_custody_metadata(custody_path, &metadata)
}

/// Rejects symlinks, reparse points, and non-regular custody entries.
fn validate_custody_metadata(
    custody_path: &Path,
    metadata: &std::fs::Metadata,
) -> Result<(), DevError> {
    if metadata.file_type().is_symlink() {
        return Err(DevError::Stage {
            stage: "launch",
            reason: format!(
                "custody path is a symbolic link; preserved: {}",
                custody_path.display()
            ),
        });
    }
    if is_reparse_point(metadata) {
        return Err(DevError::Stage {
            stage: "launch",
            reason: format!(
                "custody path is a reparse point; preserved: {}",
                custody_path.display()
            ),
        });
    }
    if !metadata.is_file() {
        return Err(DevError::Stage {
            stage: "launch",
            reason: format!(
                "custody path is not a regular file; preserved: {}",
                custody_path.display()
            ),
        });
    }
    Ok(())
}

/// Opens a reparse point itself instead of following it on Windows.
#[cfg(windows)]
fn configure_no_reparse_open(options: &mut std::fs::OpenOptions) {
    use std::os::windows::fs::OpenOptionsExt;

    options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
}

/// No special open flags outside Windows.
#[cfg(not(windows))]
fn configure_no_reparse_open(_: &mut std::fs::OpenOptions) {}

/// Maps OS lock errors to contention, mirroring the backend mapping
/// (`WouldBlock` everywhere, plus `ERROR_LOCK_VIOLATION` on Windows
/// where `fs2` preserves it instead of mapping to `WouldBlock`).
fn is_lock_contention(error: &std::io::Error) -> bool {
    if error.kind() == std::io::ErrorKind::WouldBlock {
        return true;
    }
    #[cfg(windows)]
    {
        error.raw_os_error() == Some(33)
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Reconciles a stale Forge readiness receipt before spawning the Editor.
///
/// Background: the Forge publishes its receipt with a no-clobber install
/// and removes it only on graceful shutdown; an owned Forge killed with
/// its Editor leaves the receipt behind, and the next Forge then refuses
/// to publish and dies. The dev runner owns this home's lifecycle and
/// holds the staging lock, so it — and only it — may clear the way, under
/// tight rules:
///
/// - A receipt identifying a **live** Forge running the staged binary is
///   refused, never touched ([`DevError::PreviousForgeRunning`]).
/// - Otherwise the safe regular parent chain is required, the receipt
///   must parse as valid Forge readiness (anything malformed, oversized,
///   or non-regular is preserved and refused), and the home's Forge
///   custody lock must be acquirable nonblocking — proving no live Forge
///   owns this home even when pid queries are unavailable. The probe is
///   retained while the receipt is rechecked against the exact bytes
///   validated before, and only those bytes are removed.
/// - Publish temporaries are never swept: a stale temporary cannot block
///   the next publish, and deleting files by pattern would violate the
///   preservation contract. The general readiness no-clobber invariant is
///   untouched: this never writes a receipt.
///
/// Tradeoff: custody proves no live Forge holds this home, and
/// pid-executable identity is rechecked on top; neither is cryptographic
/// ownership. The dangerous case — a live staged Forge — is refused twice:
/// once by the pid-identity check and once by custody contention, either
/// of which preserves the receipt.
///
/// # Errors
///
/// Returns [`DevError::PreviousForgeRunning`] for a live Forge,
/// [`DevError::CustodyHeld`] when custody is occupied, and
/// [`DevError::Stage`] when an unsafe or unreadable receipt blocks the
/// launch.
pub fn reconcile_stale_readiness(
    paths: &DevPaths,
    forge_exe: &Path,
) -> Result<ReadinessReconcile, DevError> {
    let readiness = paths.readiness_path();
    match process::readiness_status(&readiness, forge_exe) {
        ForgeReadinessStatus::Ready(readiness) => Err(DevError::PreviousForgeRunning {
            pid: readiness.pid(),
        }),
        ForgeReadinessStatus::Missing => Ok(ReadinessReconcile::Absent),
        ForgeReadinessStatus::Invalid => reconcile_invalid_readiness(paths, &readiness, forge_exe),
    }
}

/// Handles a present-but-unusable readiness receipt.
///
/// Only a regular file that parses as valid Forge readiness, under a safe
/// parent chain, with acquirable home custody, rechecked byte-identical,
/// is stale and removable; everything else is preserved and refused.
fn reconcile_invalid_readiness(
    paths: &DevPaths,
    readiness: &Path,
    forge_exe: &Path,
) -> Result<ReadinessReconcile, DevError> {
    let Some(readiness_dir) = readiness.parent() else {
        return Err(DevError::Stage {
            stage: "launch",
            reason: format!("readiness path has no parent: {}", readiness.display()),
        });
    };
    validate_parent_chain(readiness_dir)?;
    let metadata = match std::fs::symlink_metadata(readiness) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            // Raced away between the status check and now: nothing to clean.
            return Ok(ReadinessReconcile::Absent);
        }
        Err(_) => {
            return Err(DevError::Stage {
                stage: "launch",
                reason: format!("cannot inspect {}", readiness.display()),
            });
        }
    };
    if !is_plain_file(&metadata) {
        return Err(DevError::Stage {
            stage: "launch",
            reason: format!(
                "stale readiness at {} is not a regular file; preserved, remove it by hand",
                readiness.display()
            ),
        });
    }
    if metadata.len() > READINESS_MAX_BYTES {
        return Err(DevError::Stage {
            stage: "launch",
            reason: format!(
                "stale readiness at {} exceeds its size bound; preserved, remove it by hand",
                readiness.display()
            ),
        });
    }
    let validated = std::fs::read(readiness).map_err(|_| DevError::Stage {
        stage: "launch",
        reason: format!("cannot read {}", readiness.display()),
    })?;
    let receipt = ForgeReadiness::from_json(&validated).map_err(|_| DevError::Stage {
        stage: "launch",
        reason: format!(
            "stale readiness at {} is malformed; preserved, remove it by hand",
            readiness.display()
        ),
    })?;
    // No live Forge may own this home while the receipt is removed: the
    // custody probe proves it even when pid queries are unavailable, and
    // is retained through the recheck below so no Forge can start between
    // the checks and the removal.
    let _custody = acquire_custody_probe(&paths.custody_path())?;
    if let ForgeReadinessStatus::Ready(live) = process::readiness_status(readiness, forge_exe) {
        return Err(DevError::PreviousForgeRunning { pid: live.pid() });
    }
    let current = std::fs::read(readiness).map_err(|_| DevError::Stage {
        stage: "launch",
        reason: format!("cannot re-read {}", readiness.display()),
    })?;
    if current != validated {
        return Err(DevError::Stage {
            stage: "launch",
            reason: format!(
                "readiness at {} changed during reconciliation; retry the launch",
                readiness.display()
            ),
        });
    }
    std::fs::remove_file(readiness).map_err(|_| DevError::Stage {
        stage: "launch",
        reason: format!("cannot remove stale readiness at {}", readiness.display()),
    })?;
    Ok(ReadinessReconcile::CleanedStale { pid: receipt.pid() })
}

/// Where the launched Editor's standard streams go.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EditorOutput {
    /// The runner's own streams: an interactive terminal, or `--attach`,
    /// where the runner lives exactly as long as the Editor anyway.
    Inherit,
    /// Detached from the runner's streams. A caller that reads `cargo dev`
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
    ///
    /// Best-effort: killing the Editor releases its owned Forge through the
    /// Editor's lease and Job Object containment.
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

/// Spawns the staged Editor on the dev home.
///
/// The child inherits the environment with `ARTISAN_HOME` pointed at the
/// dev home, the manual-forge escape hatches removed, the owned dev Forge
/// requested, and the startup receipt path set. Without a registered host
/// the Editor therefore exercises its owned Forge custody path; with one
/// (the reopen hint, or the first registered host) it connects to that
/// host. Either way it writes the startup receipt once its first host
/// connection completes the initial queries. `output` routes its standard
/// streams (see [`EditorOutput`]).
///
/// # Errors
///
/// Returns [`DevError::Stage`] when the Editor cannot be spawned.
pub fn spawn_editor(
    editor_exe: &Path,
    home: &Path,
    receipt_path: &Path,
    output: &EditorOutput,
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
                .stdin(Stdio::inherit())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .spawn()
                .map(EditorProcess::direct)
                .map_err(|_| cannot_launch())
        }
        EditorOutput::Detached { log } => spawn_detached(editor_exe, home, receipt_path, log),
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
        .env(OWNED_DEV_FORGE_ENV, "1")
        .env_remove(STRIPPED_DEV_HOME_ENV)
        .env_remove(STRIPPED_DEV_READY_ENV);
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
) -> Result<EditorProcess, DevError> {
    let log_error = |_| DevError::Stage {
        stage: "launch",
        reason: format!("cannot open the editor log {}", log.display()),
    };
    let stdout = std::fs::File::create(log).map_err(log_error)?;
    let stderr = stdout.try_clone().map_err(log_error)?;
    let mut command = std::process::Command::new(editor_exe);
    configure_editor_environment(&mut command, home, receipt_path);
    command
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .map(EditorProcess::direct)
        .map_err(|_| DevError::Stage {
            stage: "launch",
            reason: format!("cannot launch {}", editor_exe.display()),
        })
}

/// Environment variable carrying the Editor path to the Windows watcher, so
/// no path is ever spliced into the script text.
#[cfg(windows)]
const LAUNCH_EXE_ENV: &str = "ARTISAN_DEV_LAUNCH_EXE";

/// Windows watcher script: starts the Editor through the shell
/// (`ShellExecuteEx` inherits no handles but passes this environment),
/// reports its process id, then exits with its exit code.
#[cfg(windows)]
const WATCHER_SCRIPT: &str = "$exe = $env:ARTISAN_DEV_LAUNCH_EXE; \
    Remove-Item Env:ARTISAN_DEV_LAUNCH_EXE; \
    $p = Start-Process -FilePath $exe -PassThru; \
    $null = $p.Handle; \
    [Console]::Out.WriteLine($p.Id); [Console]::Out.Flush(); \
    $p.WaitForExit(); exit $p.ExitCode";

/// `CREATE_NO_WINDOW`: the watcher never shows a console.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

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
        .creation_flags(CREATE_NO_WINDOW)
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
