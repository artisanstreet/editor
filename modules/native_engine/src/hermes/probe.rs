//! Bounded `hermes --version` probe with profile-owned authentication.
//!
//! TypeScript evidence (`modules/engines/src/hermes/service.ts`, `HermesVersion`):
//! the probe spawns the resolved executable with `--version`, drains standard
//! output and error concurrently under a `1 MiB` bound each, requires exit
//! code `0`, and parses standard output for the version pattern. Absent,
//! too-old, timeout, and malformed outcomes stay distinct here, and every
//! diagnostic redacts child output (sizes only, never text) because process
//! output may carry paths or gleamed profile text.

use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use super::inventory::{
    AUTH_UNKNOWN_REASON, HermesInstalledProfileLayout, HermesInventoryRequest,
    installed_profile_layout, live_inventory_request,
};
use super::resolve::{ResolvedHermesExecutable, resolve_hermes_executable};
use super::version::{HermesVersion, MINIMUM_HERMES_VERSION, parse_hermes_version};

/// Per-stream output bound for `--version`, mirroring
/// `maximum_version_output_bytes` (`1 MiB`).
pub const MAXIMUM_VERSION_OUTPUT_BYTES: u64 = 1024 * 1024;

/// Default deadline for the whole `--version` spawn.
pub const DEFAULT_PROBE_DEADLINE: Duration = Duration::from_secs(30);

/// Poll interval while waiting for the `--version` child to exit.
pub const PROBE_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Grace period for drain threads to observe pipe EOF after a kill.
///
/// A killed child's grandchildren can outlive it while holding the pipe write
/// ends open, so pipe EOF may arrive long after the kill. Past this grace the
/// probe detaches instead of blocking its return.
pub const DRAIN_JOIN_GRACE: Duration = Duration::from_secs(1);

/// Byte and time bounds for one `--version` spawn.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProbeLimits {
    maximum_bytes_per_stream: u64,
    deadline: Duration,
}

impl ProbeLimits {
    /// Creates explicit probe bounds.
    #[must_use]
    pub const fn new(maximum_bytes_per_stream: u64, deadline: Duration) -> Self {
        Self {
            maximum_bytes_per_stream,
            deadline,
        }
    }

    /// Returns the per-stream output bound in bytes.
    #[must_use]
    pub const fn maximum_bytes_per_stream(&self) -> u64 {
        self.maximum_bytes_per_stream
    }

    /// Returns the whole-spawn deadline.
    #[must_use]
    pub const fn deadline(&self) -> Duration {
        self.deadline
    }
}

impl Default for ProbeLimits {
    fn default() -> Self {
        Self::new(MAXIMUM_VERSION_OUTPUT_BYTES, DEFAULT_PROBE_DEADLINE)
    }
}

/// Returns the default `--version` bounds (`1 MiB` per stream, 30 seconds).
#[must_use]
pub fn default_probe_limits() -> ProbeLimits {
    ProbeLimits::default()
}

/// Captured `--version` output. Debug redacts stream contents (lengths only).
#[derive(Clone, Eq, PartialEq)]
pub struct SpawnedVersionOutput {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    exit_code: Option<i32>,
}

impl SpawnedVersionOutput {
    /// Returns captured standard output bytes.
    #[must_use]
    pub fn stdout(&self) -> &[u8] {
        &self.stdout
    }

    /// Returns captured standard error bytes.
    #[must_use]
    pub fn stderr(&self) -> &[u8] {
        &self.stderr
    }

    /// Returns the child exit code, if it reported one.
    #[must_use]
    pub const fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }

    /// Returns the standard output length, saturating on exotic platforms.
    #[must_use]
    pub fn stdout_len(&self) -> u64 {
        u64::try_from(self.stdout.len()).unwrap_or(u64::MAX)
    }

    /// Returns the standard error length, saturating on exotic platforms.
    #[must_use]
    pub fn stderr_len(&self) -> u64 {
        u64::try_from(self.stderr.len()).unwrap_or(u64::MAX)
    }
}

impl fmt::Debug for SpawnedVersionOutput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SpawnedVersionOutput")
            .field(
                "stdout",
                &format_args!("<{} bytes redacted>", self.stdout_len()),
            )
            .field(
                "stderr",
                &format_args!("<{} bytes redacted>", self.stderr_len()),
            )
            .field("exit_code", &self.exit_code)
            .finish()
    }
}

/// Distinct, output-redacted failures from the `--version` probe.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HermesProbeError {
    /// No executable resolved from override, installed path, or `PATH`.
    ExecutableAbsent,
    /// The child or its pipes could not be spawned, waited on, or drained.
    SpawnFailed,
    /// The child did not exit before the deadline; it was killed and reaped.
    ProbeTimeout,
    /// A stream exceeded the byte bound; the child was killed and reaped.
    OutputTooLarge,
    /// The child exited unsuccessfully; standard error text is redacted.
    NonZeroExit {
        /// The reported exit code, when the platform provides one.
        exit_code: Option<i32>,
    },
    /// The output held no recognizable version; only sizes are retained.
    VersionUnrecognized {
        /// Standard output length in bytes.
        stdout_bytes: u64,
        /// Standard error length in bytes.
        stderr_bytes: u64,
    },
    /// The installed version is older than the supported gateway.
    VersionTooOld {
        /// The installed `[major, minor, patch]` triple.
        found: [u64; 3],
    },
}

impl fmt::Display for HermesProbeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ExecutableAbsent => formatter.write_str("no hermes executable was found"),
            Self::SpawnFailed => formatter.write_str("hermes version probe could not run"),
            Self::ProbeTimeout => formatter.write_str("hermes version probe timed out"),
            Self::OutputTooLarge => {
                formatter.write_str("hermes version probe output exceeded its bound")
            }
            Self::NonZeroExit { exit_code } => {
                write!(formatter, "hermes version probe exited unsuccessfully")?;
                if let Some(code) = exit_code {
                    write!(formatter, " (code {code})")?;
                }
                Ok(())
            }
            Self::VersionUnrecognized {
                stdout_bytes,
                stderr_bytes,
            } => write!(
                formatter,
                "hermes returned an unrecognized version string ({stdout_bytes} stdout bytes, {stderr_bytes} stderr bytes)"
            ),
            Self::VersionTooOld { found } => {
                let [major, minor, patch] = *found;
                let [minimum_major, minimum_minor, minimum_patch] = MINIMUM_HERMES_VERSION;
                write!(
                    formatter,
                    "hermes {major}.{minor}.{patch} is older than the supported {minimum_major}.{minimum_minor}.{minimum_patch} gateway"
                )
            }
        }
    }
}

impl std::error::Error for HermesProbeError {}

/// Authentication state for the probe: always `Unknown`.
///
/// Auth is profile-owned by design (TypeScript descriptor `auth:
/// unsupported`); the probe never synthesizes an authenticated state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HermesAuthState {
    /// Authentication is owned by the installed Hermes profile.
    Unknown {
        /// Machine-readable reason (`owned-by-installed-profile`).
        reason: &'static str,
    },
}

impl HermesAuthState {
    /// Returns the stable state spelling (`unknown`).
    #[must_use]
    pub const fn state(&self) -> &'static str {
        match self {
            Self::Unknown { .. } => "unknown",
        }
    }

    /// Returns the machine-readable reason for the state.
    #[must_use]
    pub const fn reason(&self) -> &'static str {
        match self {
            Self::Unknown { reason } => reason,
        }
    }
}

/// Input scope for a probe, mirroring the TypeScript probe scope
/// (`profile_id`, `working_directory`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HermesProbeInput {
    profile_id: String,
    working_directory: PathBuf,
}

impl HermesProbeInput {
    /// Creates a probe scope for the given profile and working directory.
    #[must_use]
    pub fn new(profile_id: impl Into<String>, working_directory: impl Into<PathBuf>) -> Self {
        Self {
            profile_id: profile_id.into(),
            working_directory: working_directory.into(),
        }
    }

    /// Returns the profile identifier selecting the installed Hermes profile.
    #[must_use]
    pub fn profile_id(&self) -> &str {
        &self.profile_id
    }

    /// Returns the working directory the service would spawn under.
    #[must_use]
    pub fn working_directory(&self) -> &Path {
        &self.working_directory
    }
}

/// Non-billable readiness result for an installed Hermes executable.
///
/// Readiness means the executable exists and reports a supported version.
/// Debug redacts filesystem paths (resolution source and version only).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HermesProbe {
    executable: ResolvedHermesExecutable,
    input: HermesProbeInput,
    version: HermesVersion,
    authentication: HermesAuthState,
    inventory: HermesInventoryRequest,
    profile: HermesInstalledProfileLayout,
}

impl HermesProbe {
    /// Returns the resolved executable that was probed.
    #[must_use]
    pub fn executable(&self) -> &ResolvedHermesExecutable {
        &self.executable
    }

    /// Returns the probe input scope.
    #[must_use]
    pub fn input(&self) -> &HermesProbeInput {
        &self.input
    }

    /// Returns the installed version.
    #[must_use]
    pub fn version(&self) -> &HermesVersion {
        &self.version
    }

    /// Returns the authentication state (always `Unknown`).
    #[must_use]
    pub const fn authentication(&self) -> HermesAuthState {
        self.authentication
    }

    /// Returns whether the installed Hermes service is reachable-by-version.
    #[must_use]
    pub const fn ready(&self) -> bool {
        true
    }

    /// Returns the recorded `model.options` live-inventory request shape.
    #[must_use]
    pub const fn inventory_request(&self) -> HermesInventoryRequest {
        self.inventory
    }

    /// Returns the recorded installed-profile layout.
    #[must_use]
    pub fn profile_layout(&self) -> &HermesInstalledProfileLayout {
        &self.profile
    }
}

fn drain_bounded(mut reader: impl Read, maximum_bytes: u64) -> Result<Vec<u8>, HermesProbeError> {
    let mut collected = Vec::new();
    let mut total: u64 = 0;
    let mut chunk = [0_u8; 8192];
    loop {
        let read = reader
            .read(&mut chunk)
            .map_err(|_| HermesProbeError::SpawnFailed)?;
        if read == 0 {
            return Ok(collected);
        }
        total = total.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        if total > maximum_bytes {
            return Err(HermesProbeError::OutputTooLarge);
        }
        collected.extend_from_slice(&chunk[..read]);
    }
}

fn kill_and_reap(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Joins a drain thread within a bounded grace, then detaches.
///
/// After a kill, an orphaned grandchild may keep the pipe write ends open, so
/// the drain's blocking `read()` may not observe EOF promptly. This polls for
/// thread completion up to `DRAIN_JOIN_GRACE` and then drops the handle,
/// detaching the drain: the thread keeps its owned pipe handle and exits on
/// its own when EOF finally arrives, while the probe returns immediately.
/// Dropping the handle retains no child output.
fn join_or_detach(thread: thread::JoinHandle<Result<Vec<u8>, HermesProbeError>>) {
    let start = Instant::now();
    while !thread.is_finished() {
        if start.elapsed() >= DRAIN_JOIN_GRACE {
            return;
        }
        thread::sleep(PROBE_POLL_INTERVAL);
    }
    let _ = thread.join();
}

/// Spawns one executable with the given arguments and captures both streams.
///
/// Both pipes drain concurrently on helper threads under the per-stream byte
/// bound while the caller waits for exit under the deadline. The child is
/// killed and reaped on timeout and on any plumbing failure, and the drain
/// threads are then joined only within a bounded grace (an orphaned
/// grandchild may hold the pipes open past the kill); the observed exit path
/// reaps via wait before the drain results are collected.
///
/// # Errors
///
/// Returns [`HermesProbeError::SpawnFailed`] when the child or its pipes
/// cannot run, [`HermesProbeError::ProbeTimeout`] past the deadline, or
/// [`HermesProbeError::OutputTooLarge`] past the byte bound.
pub fn spawn_capture(
    executable: &Path,
    args: &[&str],
    limits: &ProbeLimits,
) -> Result<SpawnedVersionOutput, HermesProbeError> {
    let mut child = Command::new(executable)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| HermesProbeError::SpawnFailed)?;
    let Some(stdout) = child.stdout.take() else {
        kill_and_reap(&mut child);
        return Err(HermesProbeError::SpawnFailed);
    };
    let Some(stderr) = child.stderr.take() else {
        kill_and_reap(&mut child);
        return Err(HermesProbeError::SpawnFailed);
    };
    let maximum_bytes = limits.maximum_bytes_per_stream();
    let stdout_thread = thread::spawn(move || drain_bounded(stdout, maximum_bytes));
    let stderr_thread = thread::spawn(move || drain_bounded(stderr, maximum_bytes));
    let deadline = Instant::now()
        .checked_add(limits.deadline())
        .ok_or(HermesProbeError::SpawnFailed)?;
    loop {
        let status = match child.try_wait() {
            Ok(Some(status)) => status,
            Ok(None) => {
                if Instant::now() >= deadline {
                    kill_and_reap(&mut child);
                    join_or_detach(stdout_thread);
                    join_or_detach(stderr_thread);
                    return Err(HermesProbeError::ProbeTimeout);
                }
                thread::sleep(PROBE_POLL_INTERVAL);
                continue;
            }
            Err(_) => {
                kill_and_reap(&mut child);
                join_or_detach(stdout_thread);
                join_or_detach(stderr_thread);
                return Err(HermesProbeError::SpawnFailed);
            }
        };
        let exit_code = status.code();
        // Best-effort reap; `try_wait` already collected the status on most
        // platforms, so any error here is ignored.
        let _ = child.wait();
        let stdout = stdout_thread
            .join()
            .map_err(|_| HermesProbeError::SpawnFailed)??;
        let stderr = stderr_thread
            .join()
            .map_err(|_| HermesProbeError::SpawnFailed)??;
        return Ok(SpawnedVersionOutput {
            stdout,
            stderr,
            exit_code,
        });
    }
}

/// Interprets captured `--version` output: exit `0`, UTF-8 standard output,
/// and the TypeScript version pattern.
///
/// # Errors
///
/// Returns [`HermesProbeError::NonZeroExit`] for unsuccessful exit or
/// [`HermesProbeError::VersionUnrecognized`] (sizes only, text redacted) for
/// non-UTF-8 or pattern-less output.
pub fn evaluate_version_output(
    output: &SpawnedVersionOutput,
) -> Result<HermesVersion, HermesProbeError> {
    if output.exit_code() != Some(0) {
        return Err(HermesProbeError::NonZeroExit {
            exit_code: output.exit_code(),
        });
    }
    let text = std::str::from_utf8(output.stdout()).map_err(|_| {
        HermesProbeError::VersionUnrecognized {
            stdout_bytes: output.stdout_len(),
            stderr_bytes: output.stderr_len(),
        }
    })?;
    parse_hermes_version(text).map_err(|_| HermesProbeError::VersionUnrecognized {
        stdout_bytes: output.stdout_len(),
        stderr_bytes: output.stderr_len(),
    })
}

/// Enforces the minimum-version gate for a parsed version.
///
/// # Errors
///
/// Returns [`HermesProbeError::VersionTooOld`] below `[0, 20, 0]`.
pub fn check_minimum_version(version: &HermesVersion) -> Result<(), HermesProbeError> {
    if version.meets_minimum() {
        Ok(())
    } else {
        Err(HermesProbeError::VersionTooOld {
            found: version.triple(),
        })
    }
}

/// Assembles a readiness result without executing anything.
///
/// Authentication is always `Unknown` with reason
/// `owned-by-installed-profile`; the `model.options` inventory shape and the
/// installed-profile layout are recorded as data for the later runtime packet.
#[must_use]
pub fn assemble_probe(
    executable: &ResolvedHermesExecutable,
    input: &HermesProbeInput,
    version: HermesVersion,
) -> HermesProbe {
    HermesProbe {
        executable: executable.clone(),
        profile: installed_profile_layout(input.profile_id()),
        input: input.clone(),
        version,
        authentication: HermesAuthState::Unknown {
            reason: AUTH_UNKNOWN_REASON,
        },
        inventory: live_inventory_request(),
    }
}

/// Probes one resolved executable with `hermes --version`.
///
/// This is the only spawn in the discovery packet: no service spawn, no
/// WebSocket connect, no session, no prompt, no inference, no account change.
///
/// # Errors
///
/// Returns the distinct [`HermesProbeError`] for spawn, timeout, bound,
/// exit, recognition, and minimum-version failures.
pub fn probe_installed_hermes(
    executable: &ResolvedHermesExecutable,
    input: &HermesProbeInput,
    limits: &ProbeLimits,
) -> Result<HermesProbe, HermesProbeError> {
    let output = spawn_capture(executable.path(), &["--version"], limits)?;
    let version = evaluate_version_output(&output)?;
    check_minimum_version(&version)?;
    Ok(assemble_probe(executable, input, version))
}

/// Resolves the live executable and probes it with `hermes --version`.
///
/// # Errors
///
/// Returns [`HermesProbeError::ExecutableAbsent`] when nothing resolves, or
/// the [`HermesProbeError`] from [`probe_installed_hermes`] otherwise.
pub fn run_hermes_probe(
    input: &HermesProbeInput,
    limits: &ProbeLimits,
) -> Result<HermesProbe, HermesProbeError> {
    let executable = resolve_hermes_executable().ok_or(HermesProbeError::ExecutableAbsent)?;
    probe_installed_hermes(&executable, input, limits)
}
