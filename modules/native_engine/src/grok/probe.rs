//! Bounded, non-billable Grok readiness probe.
//!
//! Two spawns only, mirroring the shared ACP `Probe` in
//! `modules/engines/src/acp/engine.ts`: a `--version` spawn whose output must
//! parse via [`parse_grok_version`](super::discovery::parse_grok_version),
//! then the `grok --no-auto-update models` auth probe. No prompts, no
//! inference, no session or account changes.
//!
//! Classification keeps every outcome distinct: installed/authenticated,
//! installed-but-absent (not signed in), unavailable (the CLI ran but its
//! answer was unusable), timeout, malformed (output bound exceeded), and
//! invalid binary / not installed at the top level. Secrets never enter
//! errors: only [`redact_probe_excerpt`] excerpts (capped, environment-free)
//! are embedded, and the `XAI_API_KEY` value itself is never read into any
//! output — only its presence bit feeds [`preferred_auth_method`].
//!
//! The auth-method selection mirrors TypeScript `AuthMethod`
//! (`xai.api_key` when `XAI_API_KEY` is present and advertised, else
//! `cached_token`) and is recorded as data for the later ACP runtime packet,
//! not executed here.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::discovery::{GROK_AUTH_PROBE_ARGS, GROK_VERSION_ARGS, parse_grok_version, resolve_live};

/// Environment variable whose non-empty presence selects the API-key method.
/// Only the presence bit is ever read; the value never enters probe output.
pub const XAI_API_KEY_ENV: &str = "XAI_API_KEY";

/// ACP `authMethods` id used when `XAI_API_KEY` is present (TypeScript).
pub const GROK_AUTH_METHOD_API_KEY: &str = "xai.api_key";

/// ACP `authMethods` id used otherwise (TypeScript fallback).
pub const GROK_AUTH_METHOD_CACHED_TOKEN: &str = "cached_token";

/// Per-stream output bound mirroring the shared ACP probe budget (1 MiB).
pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 1_048_576;

/// Bounded deadline for the `--version` spawn.
pub const DEFAULT_VERSION_TIMEOUT: Duration = Duration::from_secs(30);

/// Bounded deadline for the `models` auth spawn.
pub const DEFAULT_AUTH_TIMEOUT: Duration = Duration::from_secs(30);

/// Cap for redacted excerpts embedded in error reasons.
pub const MAX_PROBE_EXCERPT_CHARS: usize = 256;

/// Auth method recorded for the later ACP runtime packet. Mirrors the
/// TypeScript `AuthMethod` ids exactly.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GrokAuthMethod {
    XaiApiKey,
    CachedToken,
}

impl GrokAuthMethod {
    /// Returns the ACP `authMethods` id for this method.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::XaiApiKey => GROK_AUTH_METHOD_API_KEY,
            Self::CachedToken => GROK_AUTH_METHOD_CACHED_TOKEN,
        }
    }
}

impl fmt::Display for GrokAuthMethod {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Which probe spawn a timeout or output bound applies to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GrokProbePhase {
    Version,
    Auth,
}

impl fmt::Display for GrokProbePhase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Version => "version",
            Self::Auth => "auth",
        })
    }
}

/// Auth readiness of an installed Grok CLI. `Absent` (answered: not signed
/// in) stays distinct from `Unavailable` (ran but unusable answer),
/// `Timeout`, and `Malformed` (output bound exceeded).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GrokAuthState {
    Authenticated,
    Absent,
    Unavailable,
    Timeout,
    Malformed,
}

/// Readiness facts for one installed Grok CLI. Data only: this never marks
/// the engine runnable; the controller decides that from a later packet.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GrokProbe {
    pub executable: PathBuf,
    pub version: String,
    pub auth: GrokAuthState,
    pub preferred_auth_method: GrokAuthMethod,
}

impl GrokProbe {
    /// Returns whether the probe found an authenticated CLI. Data only.
    #[must_use]
    pub const fn ready(&self) -> bool {
        matches!(self.auth, GrokAuthState::Authenticated)
    }
}

/// Top-level probe failure. Auth-spawn transport problems surface as
/// [`GrokAuthState`] inside [`GrokProbe`] instead; only version-phase
/// problems (and a missing binary) become errors, keeping invalid-binary
/// distinct from auth states.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GrokProbeError {
    NotInstalled,
    InvalidBinary { reason: String },
    Unavailable { reason: String },
    Timeout { phase: GrokProbePhase },
    OutputTooLarge { phase: GrokProbePhase },
}

impl fmt::Display for GrokProbeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInstalled => formatter.write_str("grok CLI is not installed"),
            Self::InvalidBinary { reason } => {
                write!(formatter, "grok CLI is not a valid binary: {reason}")
            }
            Self::Unavailable { reason } => {
                write!(formatter, "grok probe is unavailable: {reason}")
            }
            Self::Timeout { phase } => write!(formatter, "grok probe {phase} timed out"),
            Self::OutputTooLarge { phase } => {
                write!(formatter, "grok probe {phase} exceeded its output bound")
            }
        }
    }
}

impl std::error::Error for GrokProbeError {}

/// TypeScript `AuthMethod`: `xai.api_key` when `XAI_API_KEY` is present and
/// advertised, else `cached_token` when advertised, else `None`. Pure fixture
/// inputs; the later ACP runtime packet calls this with the live
/// `initialize.authMethods` list.
#[must_use]
pub fn select_auth_method(
    available_method_ids: &[&str],
    xai_api_key_present: bool,
) -> Option<GrokAuthMethod> {
    if xai_api_key_present && available_method_ids.contains(&GROK_AUTH_METHOD_API_KEY) {
        return Some(GrokAuthMethod::XaiApiKey);
    }
    if available_method_ids.contains(&GROK_AUTH_METHOD_CACHED_TOKEN) {
        return Some(GrokAuthMethod::CachedToken);
    }
    None
}

/// Preference recorded at probe time, when no `initialize.authMethods` list
/// exists yet: `xai.api_key` when `XAI_API_KEY` is present, else
/// `cached_token`. Data for the later ACP runtime packet, not executed here.
#[must_use]
pub const fn preferred_auth_method(xai_api_key_present: bool) -> GrokAuthMethod {
    if xai_api_key_present {
        GrokAuthMethod::XaiApiKey
    } else {
        GrokAuthMethod::CachedToken
    }
}

/// TypeScript `Authenticated`: rejects outputs matching
/// not-authenticated/not-logged-in semantics (case-insensitive).
#[must_use]
pub fn is_authenticated_output(output: &str) -> bool {
    let lowered = output.to_ascii_lowercase();
    !lowered.contains("not authenticated") && !lowered.contains("not logged in")
}

/// Caps an excerpt for error reasons. Never carries environment values;
/// callers must not append any.
#[must_use]
pub fn redact_probe_excerpt(output: &str) -> String {
    output
        .chars()
        .take(MAX_PROBE_EXCERPT_CHARS)
        .collect::<String>()
        .trim_end()
        .to_owned()
}

/// Classifies a completed `--version` spawn. Non-zero exits and unparsable
/// output are both `InvalidBinary`, distinct from `NotInstalled`.
/// Mirrors the shared ACP version gate (`exit.code !== 0 || version ===
/// undefined` ⇒ unavailable).
///
/// # Errors
///
/// Returns [`GrokProbeError::InvalidBinary`] for non-zero exits or output
/// without a parseable version.
pub fn classify_version_output(exit_code: i32, output: &str) -> Result<String, GrokProbeError> {
    if exit_code != 0 {
        let excerpt = redact_probe_excerpt(output);
        let detail = if excerpt.is_empty() {
            "no version output".to_owned()
        } else {
            excerpt
        };
        return Err(GrokProbeError::InvalidBinary {
            reason: format!("grok --version exited {exit_code}: {detail}"),
        });
    }
    parse_grok_version(output).ok_or_else(|| {
        let excerpt = redact_probe_excerpt(output);
        let detail = if excerpt.is_empty() {
            "no version output".to_owned()
        } else {
            excerpt
        };
        GrokProbeError::InvalidBinary {
            reason: format!("grok --version did not report a valid version: {detail}"),
        }
    })
}

/// Classifies a completed `models` auth spawn. Output matching
/// not-authenticated/not-logged-in semantics is `Absent` regardless of exit
/// code (the CLI answered: not signed in); otherwise exit `0` is
/// `Authenticated` and any other exit is `Unavailable`.
#[must_use]
pub fn classify_auth_result(exit_code: i32, output: &str) -> GrokAuthState {
    if !is_authenticated_output(output) {
        return GrokAuthState::Absent;
    }
    if exit_code == 0 {
        GrokAuthState::Authenticated
    } else {
        GrokAuthState::Unavailable
    }
}

/// Maps an auth-spawn outcome (including transport failures) onto
/// [`GrokAuthState`], keeping timeout and output-bound (malformed) distinct
/// from absent and unavailable.
#[must_use]
pub fn classify_auth_spawn(result: &Result<BoundedChildOutput, GrokProbeError>) -> GrokAuthState {
    match result {
        Ok(output) => classify_auth_result(output.exit_code, &output.combined_output),
        Err(GrokProbeError::Timeout { .. }) => GrokAuthState::Timeout,
        Err(GrokProbeError::OutputTooLarge { .. }) => GrokAuthState::Malformed,
        Err(_) => GrokAuthState::Unavailable,
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

fn map_spawn_error(error: &std::io::Error, phase: GrokProbePhase) -> GrokProbeError {
    if error.kind() == std::io::ErrorKind::NotFound {
        return GrokProbeError::NotInstalled;
    }
    GrokProbeError::Unavailable {
        reason: format!("grok probe {phase} spawn failed: {error}"),
    }
}

/// Runs one bounded child process: concurrent stdout/stderr drains (no pipe
/// deadlock), a hard deadline, a per-stream byte bound, and child cleanup on
/// every path (kill + reap on timeout, bound breach, or wait failure — no
/// unbounded waits, no orphaned children).
///
/// This is the testable seam: tests drive it with fixture processes.
///
/// # Errors
///
/// Returns [`GrokProbeError::NotInstalled`] when the program is absent,
/// [`GrokProbeError::Timeout`] past the deadline,
/// [`GrokProbeError::OutputTooLarge`] past the byte bound, or
/// [`GrokProbeError::Unavailable`] for other spawn/wait failures.
pub fn run_bounded_command(
    program: &Path,
    args: &[&str],
    timeout: Duration,
    max_bytes: usize,
    phase: GrokProbePhase,
) -> Result<BoundedChildOutput, GrokProbeError> {
    use std::process::Stdio;

    let mut command = std::process::Command::new(program);
    crate::engine_core::apply_managed_environment(&mut command, program);
    let mut child = command
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
                    let _ = stdout_reader.join();
                    let _ = stderr_reader.join();
                    return Err(GrokProbeError::Timeout { phase });
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(error) => {
                reap(&mut child);
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(GrokProbeError::Unavailable {
                    reason: format!("grok probe {phase} wait failed: {error}"),
                });
            }
        }
    };

    let stdout_bytes = match stdout_reader.join() {
        Ok(CappedRead::Done(bytes)) => bytes,
        Ok(CappedRead::TooLarge) | Err(_) => {
            reap(&mut child);
            return Err(GrokProbeError::OutputTooLarge { phase });
        }
    };
    let stderr_bytes = match stderr_reader.join() {
        Ok(CappedRead::Done(bytes)) => bytes,
        Ok(CappedRead::TooLarge) | Err(_) => {
            reap(&mut child);
            return Err(GrokProbeError::OutputTooLarge { phase });
        }
    };

    let Some(exit_code) = status.code() else {
        return Err(GrokProbeError::Unavailable {
            reason: format!("grok probe {phase} ended without an exit code"),
        });
    };
    let stdout_text = String::from_utf8_lossy(&stdout_bytes);
    let stderr_text = String::from_utf8_lossy(&stderr_bytes);
    Ok(BoundedChildOutput {
        exit_code,
        combined_output: format!("{stdout_text}\n{stderr_text}").trim().to_owned(),
    })
}

/// Bounds for [`probe_grok_readiness`].
pub struct GrokProbeLimits {
    pub version_timeout: Duration,
    pub auth_timeout: Duration,
    pub max_output_bytes: usize,
}

impl Default for GrokProbeLimits {
    fn default() -> Self {
        Self {
            version_timeout: DEFAULT_VERSION_TIMEOUT,
            auth_timeout: DEFAULT_AUTH_TIMEOUT,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
        }
    }
}

/// Runs the full readiness probe: bounded `--version` spawn (top-level
/// errors) then the bounded `models` auth spawn (recorded as
/// [`GrokAuthState`], so auth timeout/malformed stay distinct from absent,
/// unavailable, and invalid-binary).
///
/// # Errors
///
/// Returns [`GrokProbeError::NotInstalled`] when the binary is absent,
/// [`GrokProbeError::InvalidBinary`] when the version spawn fails or its
/// output is unparsable, or the version spawn's timeout/bound/unavailable
/// failure. Auth-spawn failures do not error; they become `auth` states.
pub fn probe_grok_readiness(
    executable: &Path,
    limits: &GrokProbeLimits,
    xai_api_key_present: bool,
) -> Result<GrokProbe, GrokProbeError> {
    let version_output = run_bounded_command(
        executable,
        GROK_VERSION_ARGS,
        limits.version_timeout,
        limits.max_output_bytes,
        GrokProbePhase::Version,
    )?;
    let version =
        classify_version_output(version_output.exit_code, &version_output.combined_output)?;
    let auth_output = run_bounded_command(
        executable,
        GROK_AUTH_PROBE_ARGS,
        limits.auth_timeout,
        limits.max_output_bytes,
        GrokProbePhase::Auth,
    );
    Ok(GrokProbe {
        executable: executable.to_path_buf(),
        version,
        auth: classify_auth_spawn(&auth_output),
        preferred_auth_method: preferred_auth_method(xai_api_key_present),
    })
}

/// Probes the live environment: resolves via [`resolve_live`], reads only the
/// presence bit of `XAI_API_KEY` (never its value), and runs
/// [`probe_grok_readiness`]. Reports `NotInstalled` honestly when no `grok`
/// CLI is on `PATH`; never fabricates authentication.
///
/// # Errors
///
/// Same as [`probe_grok_readiness`], plus [`GrokProbeError::NotInstalled`]
/// when resolution finds no binary.
pub fn probe_live_readiness() -> Result<GrokProbe, GrokProbeError> {
    let Some(resolved) = resolve_live() else {
        return Err(GrokProbeError::NotInstalled);
    };
    let xai_api_key_present =
        std::env::var(XAI_API_KEY_ENV).is_ok_and(|value| !value.trim().is_empty());
    probe_grok_readiness(
        resolved.path(),
        &GrokProbeLimits::default(),
        xai_api_key_present,
    )
}
