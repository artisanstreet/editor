//! Bounded, non-billable Cursor readiness probe.
//!
//! Two spawns only, mirroring the shared ACP `Probe` in
//! `modules/engines/src/acp/engine.ts`: a `--version` spawn whose output must
//! parse via [`parse_cursor_version`](super::discovery::parse_cursor_version),
//! then the `status` auth probe from the TypeScript Cursor definition. No
//! prompts, no inference, no session or account changes.
//!
//! Classification keeps every outcome distinct: installed/authenticated,
//! installed-but-absent (not signed in), unavailable (the CLI ran but its
//! answer was unusable), timeout, malformed (output bound exceeded), and
//! invalid binary / not installed at the top level. Secrets never enter
//! errors: only [`redact_probe_excerpt`] excerpts (capped, environment-free)
//! are embedded.
//!
//! The auth-method selection mirrors TypeScript `AuthMethod`
//! (`cursor_login` when advertised) and is recorded as data for the later
//! ACP runtime packet, not executed here.

use std::fmt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use super::discovery::{
    CURSOR_AUTH_PROBE_ARGS, CURSOR_VERSION_ARGS, parse_cursor_version, resolve_live,
};

/// ACP `authMethods` id used by the TypeScript Cursor definition.
pub const CURSOR_AUTH_METHOD_LOGIN: &str = "cursor_login";

/// Per-stream output bound mirroring the shared ACP probe budget (1 MiB).
pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 1_048_576;

/// Bounded deadline for the `--version` spawn.
pub const DEFAULT_VERSION_TIMEOUT: Duration = Duration::from_secs(30);

/// Bounded deadline for the `status` auth spawn.
pub const DEFAULT_AUTH_TIMEOUT: Duration = Duration::from_secs(30);

/// Cap for redacted excerpts embedded in error reasons.
pub const MAX_PROBE_EXCERPT_CHARS: usize = 256;

/// Auth method recorded for the later ACP runtime packet. Mirrors the
/// TypeScript `AuthMethod` id exactly.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CursorAuthMethod {
    CursorLogin,
}

impl CursorAuthMethod {
    /// Returns the ACP `authMethods` id for this method.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CursorLogin => CURSOR_AUTH_METHOD_LOGIN,
        }
    }
}

impl fmt::Display for CursorAuthMethod {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Which probe spawn a timeout or output bound applies to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CursorProbePhase {
    Version,
    Auth,
}

impl fmt::Display for CursorProbePhase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Version => "version",
            Self::Auth => "auth",
        })
    }
}

/// Auth readiness of an installed Cursor CLI. `Absent` (answered: not signed
/// in) stays distinct from `Unavailable` (ran but unusable answer),
/// `Timeout`, and `Malformed` (output bound exceeded).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CursorAuthState {
    Authenticated,
    Absent,
    Unavailable,
    Timeout,
    Malformed,
}

/// Readiness facts for one installed Cursor CLI. Data only: this never marks
/// the engine runnable; the controller decides that from a later packet.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CursorProbe {
    pub executable: PathBuf,
    pub version: String,
    pub auth: CursorAuthState,
}

impl CursorProbe {
    /// Returns the resolved executable this probe ran.
    #[must_use]
    pub fn executable(&self) -> &Path {
        &self.executable
    }

    /// Returns the parsed `--version` string.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Returns the classified `status` auth state.
    #[must_use]
    pub const fn auth(&self) -> CursorAuthState {
        self.auth
    }

    /// Returns whether the probe found an authenticated CLI. Data only.
    #[must_use]
    pub const fn ready(&self) -> bool {
        matches!(self.auth, CursorAuthState::Authenticated)
    }
}

/// Top-level probe failure. Auth-spawn transport problems surface as
/// [`CursorAuthState`] inside [`CursorProbe`] instead; only version-phase
/// problems (and a missing binary) become errors, keeping invalid-binary
/// distinct from auth states.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CursorProbeError {
    NotInstalled,
    InvalidBinary { reason: String },
    Unavailable { reason: String },
    Timeout { phase: CursorProbePhase },
    OutputTooLarge { phase: CursorProbePhase },
}

impl fmt::Display for CursorProbeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInstalled => formatter.write_str("cursor CLI is not installed"),
            Self::InvalidBinary { reason } => {
                write!(formatter, "cursor CLI is not a valid binary: {reason}")
            }
            Self::Unavailable { reason } => {
                write!(formatter, "cursor probe is unavailable: {reason}")
            }
            Self::Timeout { phase } => write!(formatter, "cursor probe {phase} timed out"),
            Self::OutputTooLarge { phase } => {
                write!(formatter, "cursor probe {phase} exceeded its output bound")
            }
        }
    }
}

impl std::error::Error for CursorProbeError {}

/// TypeScript `AuthMethod`: `cursor_login` when advertised, else `None`.
/// Pure fixture inputs; the later ACP runtime packet calls this with the
/// live `initialize.authMethods` list.
#[must_use]
pub fn select_auth_method(available_method_ids: &[&str]) -> Option<CursorAuthMethod> {
    if available_method_ids.contains(&CURSOR_AUTH_METHOD_LOGIN) {
        return Some(CursorAuthMethod::CursorLogin);
    }
    None
}

/// TypeScript `Authenticated`: rejects outputs matching
/// not-authenticated/not-logged-in semantics (case-insensitive).
#[must_use]
pub fn is_authenticated_output(output: &str) -> bool {
    let lowered = output.to_ascii_lowercase();
    !lowered.contains("not authenticated") && !lowered.contains("not logged in")
}

/// Caps an excerpt for error reasons. Trims first so padded output stays
/// compact, then applies the character cap. Never carries environment
/// values; callers must not append any.
#[must_use]
pub fn redact_probe_excerpt(output: &str) -> String {
    output
        .trim()
        .chars()
        .take(MAX_PROBE_EXCERPT_CHARS)
        .collect()
}

/// Classifies a completed `--version` spawn. Non-zero exits and unparsable
/// output are both `InvalidBinary`, distinct from `NotInstalled`.
/// Mirrors the shared ACP version gate (`exit.code !== 0 || version ===
/// undefined` ⇒ unavailable).
///
/// # Errors
///
/// Returns [`CursorProbeError::InvalidBinary`] for non-zero exits or output
/// without a parseable version.
#[must_use]
pub fn classify_version_output(exit_code: i32, output: &str) -> Result<String, CursorProbeError> {
    if exit_code != 0 {
        let excerpt = redact_probe_excerpt(output);
        let detail = if excerpt.is_empty() {
            "no version output".to_owned()
        } else {
            excerpt
        };
        return Err(CursorProbeError::InvalidBinary {
            reason: format!("cursor --version exited {exit_code}: {detail}"),
        });
    }
    parse_cursor_version(output).ok_or_else(|| {
        let excerpt = redact_probe_excerpt(output);
        let detail = if excerpt.is_empty() {
            "no version output".to_owned()
        } else {
            excerpt
        };
        CursorProbeError::InvalidBinary {
            reason: format!("cursor --version did not report a valid version: {detail}"),
        }
    })
}

/// Classifies a completed `status` auth spawn. Output matching
/// not-authenticated/not-logged-in semantics is `Absent` regardless of exit
/// code (the CLI answered: not signed in); otherwise exit `0` is
/// `Authenticated` and any other exit is `Unavailable`. Mirrors the shared
/// ACP auth gate (`exit.code === 0 && Authenticated(output)`).
#[must_use]
pub fn classify_auth_result(exit_code: i32, output: &str) -> CursorAuthState {
    if !is_authenticated_output(output) {
        return CursorAuthState::Absent;
    }
    if exit_code == 0 {
        CursorAuthState::Authenticated
    } else {
        CursorAuthState::Unavailable
    }
}

/// Maps an auth-spawn outcome (including transport failures) onto
/// [`CursorAuthState`], keeping timeout and output-bound (malformed) distinct
/// from absent and unavailable.
#[must_use]
pub fn classify_auth_spawn(
    result: &Result<BoundedChildOutput, CursorProbeError>,
) -> CursorAuthState {
    match result {
        Ok(output) => classify_auth_result(output.exit_code, &output.combined_output),
        Err(CursorProbeError::Timeout { .. }) => CursorAuthState::Timeout,
        Err(CursorProbeError::OutputTooLarge { .. }) => CursorAuthState::Malformed,
        Err(_) => CursorAuthState::Unavailable,
    }
}

/// One completed bounded child process. `combined_output` merges stdout and
/// stderr exactly like the shared ACP probe (`stdout\nstderr`, trimmed).
#[derive(Debug, PartialEq, Eq)]
pub struct BoundedChildOutput {
    pub exit_code: i32,
    pub combined_output: String,
}

enum CappedRead {
    Done(Vec<u8>),
    TooLarge,
}

fn read_capped(mut stream: impl std::io::Read, max_bytes: usize) -> CappedRead {
    let mut buffered = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return CappedRead::Done(buffered),
            Ok(read) => {
                if buffered.len() + read > max_bytes {
                    return CappedRead::TooLarge;
                }
                buffered.extend_from_slice(&chunk[..read]);
            }
        }
    }
}

fn map_spawn_error(error: &std::io::Error, phase: CursorProbePhase) -> CursorProbeError {
    if error.kind() == std::io::ErrorKind::NotFound {
        return CursorProbeError::NotInstalled;
    }
    CursorProbeError::Unavailable {
        reason: format!("cursor probe {phase} spawn failed: {error}"),
    }
}

/// Grace period for a drain thread to finish after its child is reaped
/// before the probe stops waiting for it.
const DRAIN_JOIN_GRACE: Duration = Duration::from_secs(1);

/// Joins a drain thread, detaching it after a bounded grace period.
///
/// Detachment only happens when another process inherited the pipe and holds
/// it open (e.g. a grandchild surviving its parent's kill, like `ping` under
/// a killed `cmd.exe`); normally reaped children always join. A detached
/// thread keeps draining until the orphan exits and its pipes close, while
/// the probe result is already returned — orphans exit on their own, so no
/// unbounded wait and no leaked result. Like the codex precedent.
fn join_or_detach(handle: thread::JoinHandle<CappedRead>) -> Option<CappedRead> {
    let grace = Instant::now().checked_add(DRAIN_JOIN_GRACE);
    while !handle.is_finished() {
        if grace.is_none_or(|deadline| Instant::now() >= deadline) {
            return None;
        }
        thread::sleep(Duration::from_millis(5));
    }
    handle.join().ok()
}

/// Runs one bounded child process: concurrent stdout/stderr drains (no pipe
/// deadlock), a hard deadline, a per-stream byte bound, and child cleanup on
/// every path (kill + reap on timeout, bound breach, or wait failure — no
/// unbounded waits, no unreaped children). Drain threads are joined with a
/// bounded grace period and detached when an orphaned pipe-holder outlives
/// the kill, so the result never waits for a grandchild's natural exit.
///
/// This is the testable seam: tests drive it with fixture processes.
///
/// # Errors
///
/// Returns [`CursorProbeError::NotInstalled`] when the program is absent,
/// [`CursorProbeError::Timeout`] past the deadline,
/// [`CursorProbeError::OutputTooLarge`] past the byte bound, or
/// [`CursorProbeError::Unavailable`] for other spawn/wait failures.
pub fn run_bounded_command(
    program: &Path,
    args: &[&str],
    timeout: Duration,
    max_bytes: usize,
    phase: CursorProbePhase,
) -> Result<BoundedChildOutput, CursorProbeError> {
    use std::process::Stdio;

    let mut child = std::process::Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| map_spawn_error(&error, phase))?;

    let stdout_reader = std::thread::spawn({
        let stdout = child.stdout.take();
        move || {
            stdout.map_or(CappedRead::Done(Vec::new()), |stream| {
                read_capped(stream, max_bytes)
            })
        }
    });
    let stderr_reader = std::thread::spawn({
        let stderr = child.stderr.take();
        move || {
            stderr.map_or(CappedRead::Done(Vec::new()), |stream| {
                read_capped(stream, max_bytes)
            })
        }
    });

    let reap = |child: &mut std::process::Child| {
        let _ = child.kill();
        let _ = child.wait();
    };

    let start = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if start.elapsed() >= timeout {
                    reap(&mut child);
                    join_or_detach(stdout_reader);
                    join_or_detach(stderr_reader);
                    return Err(CursorProbeError::Timeout { phase });
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(error) => {
                reap(&mut child);
                join_or_detach(stdout_reader);
                join_or_detach(stderr_reader);
                return Err(CursorProbeError::Unavailable {
                    reason: format!("cursor probe {phase} wait failed: {error}"),
                });
            }
        }
    };

    let stdout_bytes = match join_or_detach(stdout_reader) {
        Some(CappedRead::Done(bytes)) => bytes,
        Some(CappedRead::TooLarge) | None => {
            reap(&mut child);
            return Err(CursorProbeError::OutputTooLarge { phase });
        }
    };
    let stderr_bytes = match join_or_detach(stderr_reader) {
        Some(CappedRead::Done(bytes)) => bytes,
        Some(CappedRead::TooLarge) | None => {
            reap(&mut child);
            return Err(CursorProbeError::OutputTooLarge { phase });
        }
    };

    let Some(exit_code) = status.code() else {
        return Err(CursorProbeError::Unavailable {
            reason: format!("cursor probe {phase} ended without an exit code"),
        });
    };
    let stdout_text = String::from_utf8_lossy(&stdout_bytes);
    let stderr_text = String::from_utf8_lossy(&stderr_bytes);
    Ok(BoundedChildOutput {
        exit_code,
        combined_output: format!("{stdout_text}\n{stderr_text}").trim().to_owned(),
    })
}

/// Bounds for [`probe_cursor_readiness`].
pub struct CursorProbeLimits {
    pub version_timeout: Duration,
    pub auth_timeout: Duration,
    pub max_output_bytes: usize,
}

impl Default for CursorProbeLimits {
    fn default() -> Self {
        Self {
            version_timeout: DEFAULT_VERSION_TIMEOUT,
            auth_timeout: DEFAULT_AUTH_TIMEOUT,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
        }
    }
}

/// Runs the full readiness probe: bounded `--version` spawn (top-level
/// errors) then the bounded `status` auth spawn (recorded as
/// [`CursorAuthState`], so auth timeout/malformed stay distinct from absent,
/// unavailable, and invalid-binary).
///
/// # Errors
///
/// Returns [`CursorProbeError::NotInstalled`] when the binary is absent,
/// [`CursorProbeError::InvalidBinary`] when the version spawn fails or its
/// output is unparsable, or the version spawn's timeout/bound/unavailable
/// failure. Auth-spawn failures do not error; they become `auth` states.
pub fn probe_cursor_readiness(
    executable: &Path,
    limits: &CursorProbeLimits,
) -> Result<CursorProbe, CursorProbeError> {
    let version_output = run_bounded_command(
        executable,
        CURSOR_VERSION_ARGS,
        limits.version_timeout,
        limits.max_output_bytes,
        CursorProbePhase::Version,
    )?;
    let version =
        classify_version_output(version_output.exit_code, &version_output.combined_output)?;
    let auth_output = run_bounded_command(
        executable,
        CURSOR_AUTH_PROBE_ARGS,
        limits.auth_timeout,
        limits.max_output_bytes,
        CursorProbePhase::Auth,
    );
    Ok(CursorProbe {
        executable: executable.to_path_buf(),
        version,
        auth: classify_auth_spawn(&auth_output),
    })
}

/// Probes the live environment: resolves via [`resolve_live`] and runs
/// [`probe_cursor_readiness`]. Reports `NotInstalled` honestly when no Cursor
/// CLI is on `PATH`; never fabricates authentication.
///
/// # Errors
///
/// Same as [`probe_cursor_readiness`], plus [`CursorProbeError::NotInstalled`]
/// when resolution finds no binary.
pub fn probe_live_readiness() -> Result<CursorProbe, CursorProbeError> {
    let Some(resolved) = resolve_live() else {
        return Err(CursorProbeError::NotInstalled);
    };
    probe_cursor_readiness(resolved.path(), &CursorProbeLimits::default())
}
