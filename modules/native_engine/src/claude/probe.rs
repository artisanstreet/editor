//! Bounded, non-billable Claude Code readiness probe.
//!
//! Ports `MakeClaudeProbe` and `ReadClaudeAuthentication` from
//! `modules/engines/src/claude/probe.ts`: run `claude --version`, parse the
//! first semantic version, then run `claude auth status` and classify the
//! saved-session state. Only these two spawns exist here; billable runs,
//! usage reads, and credential changes are out of scope.
//!
//! State discipline mirrors the TypeScript source:
//!
//! - `authenticated` only when `auth status` exits successfully **and** its
//!   whole stdout decodes as `{"loggedIn": true}`;
//! - `unauthenticated` when it exits successfully with `{"loggedIn": false}`;
//! - `unknown` when the spawn fails the protocol contract (nonzero exit,
//!   malformed JSON), carrying a bounded, secret-free reason;
//! - timeouts and output-limit violations fail the probe itself, exactly like
//!   `EngineProbeTimeoutError` and the bounded reads do.
//!
//! A ready result therefore always implies credential-present evidence; no
//! path here can report ready from absent or undecidable auth. Diagnostics
//! never embed process output, paths, or environment values.

use std::fmt;
use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use super::discovery::{ClaudeExecutable, MAX_EXECUTABLE_ARG_BYTES, MAX_EXECUTABLE_ARGS};

/// Engine identifier matching the TypeScript descriptor.
pub const CLAUDE_ENGINE_ID: &str = "claude";

/// Transport name from `modules/engines/src/claude/descriptor.ts`.
pub const CLAUDE_TRANSPORT: &str = "claude-cli-stream-json";

/// Protocol version from `modules/engines/src/claude/descriptor.ts`.
pub const CLAUDE_PROTOCOL_VERSION: &str = "claude-stream-json-v1";

/// Claude Code release whose native resume behavior Artisan verified.
///
/// Mirrors `claude_native_continuation_version` in
/// `modules/engines/src/claude/probe.ts`. Recorded here so readiness evidence
/// can be gated on it without re-reading TypeScript.
pub const NATIVE_CONTINUATION_VERSION: &str = "2.1.220";

/// Default per-phase deadline for `claude --version`.
pub const DEFAULT_VERSION_TIMEOUT: Duration = Duration::from_secs(15);

/// Default per-phase deadline for `claude auth status`.
pub const DEFAULT_AUTH_TIMEOUT: Duration = Duration::from_secs(15);

/// Upper bound accepted for any single probe deadline.
pub const MAX_PROBE_TIMEOUT: Duration = Duration::from_secs(300);

/// Default bound for one probe stream's stdout, matching the adapter.
pub const DEFAULT_MAX_STDOUT_BYTES: usize = 16 * 1024 * 1024;

/// Default bound for one probe stream's stderr, matching the adapter.
pub const DEFAULT_MAX_STDERR_BYTES: usize = 1024 * 1024;

/// Minimum accepted per-stream output bound.
pub const MIN_OUTPUT_BOUND_BYTES: usize = 1024;

/// Maximum accepted per-stream output bound.
pub const MAX_OUTPUT_BOUND_BYTES: usize = 64 * 1024 * 1024;

/// Maximum retained auth-reason length in bytes.
pub const MAX_AUTH_REASON_BYTES: usize = 512;

/// Fallback reason when `auth status` leaves no usable stderr text.
pub const DEFAULT_AUTH_REASON_UNAVAILABLE: &str = "Claude auth status is unavailable";

/// Maximum accepted semantic-version length in bytes.
const MAX_VERSION_BYTES: usize = 128;

/// How often a running probe child is polled for exit.
const POLL_INTERVAL: Duration = Duration::from_millis(5);

/// Which probe phase a failure belongs to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClaudeProbePhase {
    /// The `claude --version` spawn.
    Version,
    /// The `claude auth status` spawn.
    Authentication,
}

impl fmt::Display for ClaudeProbePhase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Version => "version",
            Self::Authentication => "authentication",
        })
    }
}

/// Saved-session authentication state of the Claude CLI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClaudeAuthState {
    /// `auth status` proved a signed-in session.
    Authenticated,
    /// `auth status` proved no signed-in session.
    Unauthenticated,
    /// `auth status` did not answer the protocol contract.
    Unknown,
}

/// Classified authentication evidence for one probe.
///
/// The optional reason exists only for [`ClaudeAuthState::Unknown`]. It is
/// derived from stderr, trimmed and bounded, and never contains stdout bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClaudeAuthentication {
    state: ClaudeAuthState,
    reason: Option<String>,
}

impl ClaudeAuthentication {
    /// Returns the classified state.
    #[must_use]
    pub const fn state(&self) -> ClaudeAuthState {
        self.state
    }

    /// Returns the bounded unavailable reason, if this state is unknown.
    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }

    /// Returns true only for protocol-proven signed-in state.
    #[must_use]
    pub const fn is_ready(&self) -> bool {
        matches!(self.state, ClaudeAuthState::Authenticated)
    }
}

/// Per-engine typed readiness result, pending registry integration.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClaudeProbeResult {
    version: String,
    authentication: ClaudeAuthentication,
    ready: bool,
}

impl ClaudeProbeResult {
    /// Returns the parsed CLI version.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Returns the classified authentication evidence.
    #[must_use]
    pub const fn authentication(&self) -> &ClaudeAuthentication {
        &self.authentication
    }

    /// Returns true only when authentication is protocol-proven present.
    #[must_use]
    pub const fn ready(&self) -> bool {
        self.ready
    }
}

/// Bounded, secret-free failures from the Claude readiness probe.
///
/// Variants carry only phases, exit codes, and deadline lengths. Process
/// output, executable paths, and environment values never enter diagnostics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClaudeProbeError {
    /// Probe options fail validation (deadlines, bounds, or argument shape).
    InvalidConfiguration,
    /// The CLI could not be spawned or its pipes could not be read.
    SpawnFailed { phase: ClaudeProbePhase },
    /// `claude --version` exited nonzero or under a signal.
    VersionExit { code: Option<i32> },
    /// `claude --version` output held no semantic version.
    VersionMalformed,
    /// One probe stream exceeded its byte bound.
    OutputTooLarge { phase: ClaudeProbePhase },
    /// One probe phase exceeded its deadline; the child was killed and reaped.
    Timeout {
        phase: ClaudeProbePhase,
        timeout: Duration,
    },
}

impl ClaudeProbeError {
    /// Returns the stable classification for this failure.
    #[must_use]
    pub const fn cli_reason(self) -> &'static str {
        match self {
            Self::InvalidConfiguration => "invalid_configuration",
            Self::SpawnFailed { .. } => "spawn_failed",
            Self::VersionExit { .. } => "version_exit",
            Self::VersionMalformed => "version_malformed",
            Self::OutputTooLarge { .. } => "output_too_large",
            Self::Timeout { .. } => "timeout",
        }
    }
}

impl fmt::Display for ClaudeProbeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidConfiguration => {
                formatter.write_str("Claude probe configuration is invalid")
            }
            Self::SpawnFailed { phase } => {
                write!(formatter, "Claude {phase} probe could not start")
            }
            Self::VersionExit { code } => match code {
                Some(code) => {
                    write!(formatter, "Claude --version exited with code {code}")
                }
                None => formatter.write_str("Claude --version terminated under a signal"),
            },
            Self::VersionMalformed => {
                formatter.write_str("Claude --version did not contain a semantic version")
            }
            Self::OutputTooLarge { phase } => {
                write!(formatter, "Claude {phase} probe output exceeded its bound")
            }
            Self::Timeout { phase, timeout } => {
                write!(
                    formatter,
                    "Claude {phase} probe timed out after {}ms",
                    timeout.as_millis()
                )
            }
        }
    }
}

impl std::error::Error for ClaudeProbeError {}

/// Configures the bounded Claude readiness probe.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClaudeProbeOptions {
    executable_args: Vec<String>,
    version_timeout: Duration,
    auth_timeout: Duration,
    max_stdout_bytes: usize,
    max_stderr_bytes: usize,
}

impl Default for ClaudeProbeOptions {
    fn default() -> Self {
        Self {
            executable_args: Vec::new(),
            version_timeout: DEFAULT_VERSION_TIMEOUT,
            auth_timeout: DEFAULT_AUTH_TIMEOUT,
            max_stdout_bytes: DEFAULT_MAX_STDOUT_BYTES,
            max_stderr_bytes: DEFAULT_MAX_STDERR_BYTES,
        }
    }
}

impl ClaudeProbeOptions {
    /// Overrides the extra arguments prepended to every probe spawn.
    #[must_use]
    pub fn with_executable_args(mut self, args: Vec<String>) -> Self {
        self.executable_args = args;
        self
    }

    /// Overrides the `claude --version` deadline.
    #[must_use]
    pub const fn with_version_timeout(mut self, timeout: Duration) -> Self {
        self.version_timeout = timeout;
        self
    }

    /// Overrides the `claude auth status` deadline.
    #[must_use]
    pub const fn with_auth_timeout(mut self, timeout: Duration) -> Self {
        self.auth_timeout = timeout;
        self
    }

    /// Overrides the per-stream stdout bound.
    #[must_use]
    pub const fn with_max_stdout_bytes(mut self, bound: usize) -> Self {
        self.max_stdout_bytes = bound;
        self
    }

    /// Overrides the per-stream stderr bound.
    #[must_use]
    pub const fn with_max_stderr_bytes(mut self, bound: usize) -> Self {
        self.max_stderr_bytes = bound;
        self
    }

    /// Returns the configured extra arguments.
    #[must_use]
    pub fn executable_args(&self) -> &[String] {
        &self.executable_args
    }

    /// Validates deadlines, bounds, and argument shape.
    ///
    /// # Errors
    ///
    /// Returns [`ClaudeProbeError::InvalidConfiguration`] when a deadline is
    /// zero or above [`MAX_PROBE_TIMEOUT`], a stream bound is outside
    /// `MIN_OUTPUT_BOUND_BYTES..=MAX_OUTPUT_BOUND_BYTES`, too many arguments
    /// are configured, or an argument is empty or overlong.
    pub fn validate(&self) -> Result<(), ClaudeProbeError> {
        if self.version_timeout.is_zero()
            || self.version_timeout > MAX_PROBE_TIMEOUT
            || self.auth_timeout.is_zero()
            || self.auth_timeout > MAX_PROBE_TIMEOUT
        {
            return Err(ClaudeProbeError::InvalidConfiguration);
        }
        for bound in [self.max_stdout_bytes, self.max_stderr_bytes] {
            if !(MIN_OUTPUT_BOUND_BYTES..=MAX_OUTPUT_BOUND_BYTES).contains(&bound) {
                return Err(ClaudeProbeError::InvalidConfiguration);
            }
        }
        if self.executable_args.len() > MAX_EXECUTABLE_ARGS
            || self
                .executable_args
                .iter()
                .any(|arg| arg.is_empty() || arg.len() > MAX_EXECUTABLE_ARG_BYTES)
        {
            return Err(ClaudeProbeError::InvalidConfiguration);
        }
        Ok(())
    }
}

const fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

const fn is_version_suffix_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'.' || byte == b'-'
}

/// Parses a dotted triple at `start`, returning the version and end offset.
///
/// Suffix groups are optional exactly like the adapter regex: a bare `-` or
/// `+` with no suffix characters does not participate, so `1.2.3-` still
/// yields `1.2.3` ending before the marker.
fn match_version_at(bytes: &[u8], start: usize) -> Option<(String, usize)> {
    let mut index = start;
    for part in 0..3 {
        let digits = index;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        if index == digits {
            return None;
        }
        if part < 2 {
            if bytes.get(index) != Some(&b'.') {
                return None;
            }
            index += 1;
        }
    }
    for marker in *b"-+" {
        if bytes.get(index) == Some(&marker) {
            let mut end = index + 1;
            while bytes
                .get(end)
                .is_some_and(|byte| is_version_suffix_byte(*byte))
            {
                end += 1;
            }
            if end == index + 1 {
                break;
            }
            index = end;
        }
    }
    let version = str::from_utf8(&bytes[start..index]).ok()?;
    if version.len() > MAX_VERSION_BYTES {
        return None;
    }
    Some((version.to_owned(), index))
}

/// Extracts the first semantic version with word boundaries.
///
/// Mirrors the TypeScript
/// `/\b\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?\b/` match over
/// `--version` stdout, including its word-boundary behavior.
#[must_use]
pub fn parse_claude_version(output: &str) -> Option<String> {
    let bytes = output.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index].is_ascii_digit()
            && (index == 0 || !is_word_byte(bytes[index - 1]))
            && let Some((version, end)) = match_version_at(bytes, index)
            && (end >= bytes.len() || !is_word_byte(bytes[end]))
        {
            return Some(version);
        }
        index += 1;
    }
    None
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthStatusDocument {
    #[serde(rename = "loggedIn")]
    logged_in: bool,
}

/// Decodes one `auth status` stdout document.
///
/// Returns `None` for empty, truncated, or mistyped JSON, including trailing
/// garbage and unknown fields. Strictness is intentional: only a whole,
/// exact `{"loggedIn": bool}` document counts as protocol evidence.
#[must_use]
pub fn parse_auth_logged_in(stdout: &[u8]) -> Option<bool> {
    if stdout.len() > DEFAULT_MAX_STDOUT_BYTES {
        return None;
    }
    serde_json::from_slice::<AuthStatusDocument>(stdout)
        .ok()
        .map(|document| document.logged_in)
}

/// Trims stderr into a bounded unavailable reason.
///
/// Falls back to [`DEFAULT_AUTH_REASON_UNAVAILABLE`] for empty output.
/// Truncation respects UTF-8 character boundaries and never retains stdout.
#[must_use]
pub fn sanitize_auth_reason(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return DEFAULT_AUTH_REASON_UNAVAILABLE.to_owned();
    }
    if trimmed.len() <= MAX_AUTH_REASON_BYTES {
        return trimmed.to_owned();
    }
    let mut end = MAX_AUTH_REASON_BYTES;
    while !trimmed.is_char_boundary(end) {
        end -= 1;
    }
    trimmed[..end].to_owned()
}

/// Classifies `auth status` evidence into authenticated states.
///
/// A nonzero exit or a malformed document yields `Unknown` even when the
/// other channel looks signed in: only the full protocol contract proves
/// credential presence. `Authenticated` additionally implies readiness.
#[must_use]
pub fn classify_authentication(
    exit_success: bool,
    stdout: &[u8],
    stderr: &[u8],
) -> ClaudeAuthentication {
    if !exit_success {
        return ClaudeAuthentication {
            state: ClaudeAuthState::Unknown,
            reason: Some(sanitize_auth_reason(stderr)),
        };
    }
    match parse_auth_logged_in(stdout) {
        Some(true) => ClaudeAuthentication {
            state: ClaudeAuthState::Authenticated,
            reason: None,
        },
        Some(false) => ClaudeAuthentication {
            state: ClaudeAuthState::Unauthenticated,
            reason: None,
        },
        None => ClaudeAuthentication {
            state: ClaudeAuthState::Unknown,
            reason: Some(sanitize_auth_reason(stderr)),
        },
    }
}

struct BoundedStream {
    bytes: Vec<u8>,
    overflowed: bool,
    failed: bool,
}

fn spawn_bounded_reader(
    pipe: impl Read + Send + 'static,
    limit: usize,
) -> thread::JoinHandle<BoundedStream> {
    thread::spawn(move || {
        let take = u64::try_from(limit.saturating_add(1)).unwrap_or(u64::MAX);
        let mut reader = pipe.take(take);
        let mut bytes = Vec::new();
        match reader.read_to_end(&mut bytes) {
            Ok(_) => BoundedStream {
                overflowed: bytes.len() > limit,
                failed: false,
                bytes,
            },
            Err(_) => BoundedStream {
                bytes: Vec::new(),
                overflowed: false,
                failed: true,
            },
        }
    })
}

struct ChildOutput {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    success: bool,
    code: Option<i32>,
}

/// Runs one probe spawn with bounded output, a deadline, and child cleanup.
///
/// Stdin is null so a one-shot probe observes EOF immediately, mirroring the
/// adapter's explicit `EndInput`. Stdout and stderr drain on dedicated threads
/// so a full pipe cannot deadlock the child. On deadline expiry the child is
/// killed and reaped before the timeout is reported.
fn run_bounded(
    program: &std::ffi::OsStr,
    args: &[String],
    timeout: Duration,
    max_stdout_bytes: usize,
    max_stderr_bytes: usize,
    phase: ClaudeProbePhase,
) -> Result<ChildOutput, ClaudeProbeError> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| ClaudeProbeError::SpawnFailed { phase })?;
    let stdout_reader = child
        .stdout
        .take()
        .map(|pipe| spawn_bounded_reader(pipe, max_stdout_bytes));
    let stderr_reader = child
        .stderr
        .take()
        .map(|pipe| spawn_bounded_reader(pipe, max_stderr_bytes));
    let (Some(stdout_reader), Some(stderr_reader)) = (stdout_reader, stderr_reader) else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(ClaudeProbeError::SpawnFailed { phase });
    };

    let deadline = Instant::now()
        .checked_add(timeout)
        .unwrap_or_else(|| Instant::now() + MAX_PROBE_TIMEOUT);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = stdout_reader
                    .join()
                    .map_err(|_| ClaudeProbeError::SpawnFailed { phase })?;
                let stderr = stderr_reader
                    .join()
                    .map_err(|_| ClaudeProbeError::SpawnFailed { phase })?;
                if stdout.failed || stderr.failed {
                    return Err(ClaudeProbeError::SpawnFailed { phase });
                }
                if stdout.overflowed || stderr.overflowed {
                    return Err(ClaudeProbeError::OutputTooLarge { phase });
                }
                return Ok(ChildOutput {
                    stdout: stdout.bytes,
                    stderr: stderr.bytes,
                    success: status.success(),
                    code: status.code(),
                });
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = stdout_reader.join();
                    let _ = stderr_reader.join();
                    return Err(ClaudeProbeError::Timeout { phase, timeout });
                }
                thread::sleep(POLL_INTERVAL);
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ClaudeProbeError::SpawnFailed { phase });
            }
        }
    }
}

/// Bounded Claude `--version` plus `auth status` readiness probe.
///
/// Owns no configuration or credential state: it spawns the already-resolved
/// [`ClaudeExecutable`] twice and classifies the evidence.
pub struct ClaudeProbeRunner {
    executable: ClaudeExecutable,
    options: ClaudeProbeOptions,
}

impl ClaudeProbeRunner {
    /// Builds a runner for one resolved executable and validated options.
    ///
    /// # Errors
    ///
    /// Returns [`ClaudeProbeError::InvalidConfiguration`] when the options
    /// fail [`ClaudeProbeOptions::validate`].
    pub fn new(
        executable: ClaudeExecutable,
        options: ClaudeProbeOptions,
    ) -> Result<Self, ClaudeProbeError> {
        options.validate()?;
        Ok(Self {
            executable,
            options,
        })
    }

    /// Returns the resolved executable this runner spawns.
    #[must_use]
    pub const fn executable(&self) -> &ClaudeExecutable {
        &self.executable
    }

    /// Runs `--version` then `auth status` and classifies readiness.
    ///
    /// The version phase fails the probe on spawn, exit, deadline, bound, or
    /// malformed-output errors. The auth phase fails only on spawn, deadline,
    /// or bound errors; exit and document failures become `Unknown`
    /// authentication instead. The result is ready only when the version
    /// parsed and authentication is protocol-proven present.
    ///
    /// # Errors
    ///
    /// Returns [`ClaudeProbeError`] for version-phase failures and for
    /// auth-phase spawn, deadline, or bound failures.
    pub fn run(&self) -> Result<ClaudeProbeResult, ClaudeProbeError> {
        let mut version_args = self.options.executable_args.clone();
        version_args.push("--version".to_owned());
        let version_output = run_bounded(
            self.executable.command(),
            &version_args,
            self.options.version_timeout,
            self.options.max_stdout_bytes,
            self.options.max_stderr_bytes,
            ClaudeProbePhase::Version,
        )?;
        if !version_output.success {
            return Err(ClaudeProbeError::VersionExit {
                code: version_output.code,
            });
        }
        let version_text = String::from_utf8_lossy(&version_output.stdout);
        let Some(version) = parse_claude_version(&version_text) else {
            return Err(ClaudeProbeError::VersionMalformed);
        };

        let mut auth_args = self.options.executable_args.clone();
        auth_args.push("auth".to_owned());
        auth_args.push("status".to_owned());
        let auth_output = run_bounded(
            self.executable.command(),
            &auth_args,
            self.options.auth_timeout,
            self.options.max_stdout_bytes,
            self.options.max_stderr_bytes,
            ClaudeProbePhase::Authentication,
        )?;
        let authentication = classify_authentication(
            auth_output.success,
            &auth_output.stdout,
            &auth_output.stderr,
        );
        let ready = authentication.is_ready();
        Ok(ClaudeProbeResult {
            version,
            authentication,
            ready,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_parsing_matches_adapter_semantics() {
        assert_eq!(
            parse_claude_version("2.1.220 (Claude Code)"),
            Some("2.1.220".to_owned())
        );
        assert_eq!(
            parse_claude_version("claude 1.2.3-beta.1+build.5 done"),
            Some("1.2.3-beta.1+build.5".to_owned())
        );
        assert_eq!(parse_claude_version("no version here"), None);
        assert_eq!(parse_claude_version("1.2 trailing"), None);
        assert_eq!(parse_claude_version("v1.2.3"), None);
        assert_eq!(parse_claude_version("1.2.3abc"), None);
    }

    #[test]
    fn auth_classification_never_reports_ready_without_proof() {
        let authenticated = classify_authentication(true, br#"{"loggedIn": true}"#, b"");
        assert_eq!(authenticated.state(), ClaudeAuthState::Authenticated);
        assert!(authenticated.is_ready());
        assert_eq!(authenticated.reason(), None);

        let denied = classify_authentication(true, br#"{"loggedIn": false}"#, b"");
        assert_eq!(denied.state(), ClaudeAuthState::Unauthenticated);
        assert!(!denied.is_ready());

        let cases: &[(bool, &[u8], &[u8])] = &[
            (false, br#"{"loggedIn": true}"#, b"boom"),
            (true, b"not json", b""),
            (true, b"", b""),
            (true, br#"{"loggedIn": true} trailing"#, b""),
            (true, br#"{"loggedIn": true, "extra": 1}"#, b""),
            (false, b"", b""),
        ];
        for (success, stdout, stderr) in cases {
            let unknown = classify_authentication(*success, stdout, stderr);
            assert_eq!(unknown.state(), ClaudeAuthState::Unknown);
            assert!(!unknown.is_ready());
            assert!(unknown.reason().is_some_and(|reason| !reason.is_empty()));
        }
        assert_eq!(
            classify_authentication(false, b"", b"").reason(),
            Some(DEFAULT_AUTH_REASON_UNAVAILABLE)
        );
        assert_eq!(
            sanitize_auth_reason(b"  not logged in  "),
            "not logged in".to_owned()
        );
    }

    #[test]
    fn probe_options_reject_invalid_bounds_and_shapes() {
        ClaudeProbeOptions::default().validate().unwrap();
        assert_eq!(
            ClaudeProbeOptions::default()
                .with_version_timeout(Duration::ZERO)
                .validate(),
            Err(ClaudeProbeError::InvalidConfiguration)
        );
        assert_eq!(
            ClaudeProbeOptions::default()
                .with_auth_timeout(MAX_PROBE_TIMEOUT + Duration::from_secs(1))
                .validate(),
            Err(ClaudeProbeError::InvalidConfiguration)
        );
        assert_eq!(
            ClaudeProbeOptions::default()
                .with_max_stdout_bytes(0)
                .validate(),
            Err(ClaudeProbeError::InvalidConfiguration)
        );
        assert_eq!(
            ClaudeProbeOptions::default()
                .with_executable_args(vec![String::new()])
                .validate(),
            Err(ClaudeProbeError::InvalidConfiguration)
        );
        assert_eq!(
            ClaudeProbeError::Timeout {
                phase: ClaudeProbePhase::Version,
                timeout: DEFAULT_VERSION_TIMEOUT,
            }
            .cli_reason(),
            "timeout"
        );
    }
}
