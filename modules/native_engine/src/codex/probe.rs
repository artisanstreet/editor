//! Finite native Codex readiness probe.
//!
//! Bounded, non-billable readiness only: run `codex --version` with byte and
//! time bounds and classify an `account/read` result into authenticated
//! versus unauthenticated readiness. No prompt is sent, no model inference
//! runs, and no account or session state is changed.
//!
//! CLI text such as a login-status message is never account evidence. Auth
//! absence (`Unauthenticated`) is distinct from `Unavailable`, `Timeout`,
//! and `InvalidBinary` failures, and every error is path- and secret-free.

use std::fmt;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use super::process::{
    PROBE_POLL_INTERVAL, PipeDrain, PipeEvent, probe_deadline, reap_or_kill, spawn_probe_child,
    stop_child, take_child_stderr, take_child_stdout,
};
use super::version::{
    CODEX_CONTINUATION_CLI_VERSION, CODEX_MINIMUM_CLI_VERSION, meets_minimum_version,
    parse_codex_version,
};

/// Byte bound for one captured `--version` stream.
///
/// Matches the TypeScript `ReadBoundedStream` bound of `64 * 1024`.
pub const CODEX_VERSION_OUTPUT_BOUND_BYTES: usize = 64 * 1024;

/// Byte bound for one decoded `account/read` document.
pub const CODEX_ACCOUNT_OUTPUT_BOUND_BYTES: usize = 64 * 1024;

/// Default deadline for the `--version` spawn.
pub const CODEX_VERSION_TIMEOUT: Duration = Duration::from_secs(10);

/// Bounded, path-free failures from the Codex readiness probe.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodexProbeError {
    /// The executable could not be spawned.
    InvalidBinary,
    /// The executable exited without usable readiness output.
    Unavailable,
    /// The bounded probe exceeded its deadline.
    Timeout,
    /// A captured stream exceeded its byte bound.
    OutputTooLarge,
    /// `--version` output contained no semantic version.
    VersionUnparseable,
    /// The installed version is older than the minimum supported version.
    VersionTooOld,
    /// An `account/read` result did not match the account schema.
    AccountInvalid,
    /// A protocol envelope or handshake result was malformed or unexpected.
    Protocol,
}

impl CodexProbeError {
    /// Returns the stable CLI classification for this failure.
    #[must_use]
    pub const fn cli_reason(self) -> &'static str {
        match self {
            Self::InvalidBinary => "invalid_binary",
            Self::Unavailable => "unavailable",
            Self::Timeout => "timeout",
            Self::OutputTooLarge => "output_too_large",
            Self::VersionUnparseable => "version_unparseable",
            Self::VersionTooOld => "version_too_old",
            Self::AccountInvalid => "account_invalid",
            Self::Protocol => "protocol",
        }
    }
}

impl fmt::Display for CodexProbeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidBinary => "Codex executable is invalid",
            Self::Unavailable => "Codex probe is unavailable",
            Self::Timeout => "Codex probe timed out",
            Self::OutputTooLarge => "Codex probe output exceeds its bound",
            Self::VersionUnparseable => "Codex version output did not contain a semantic version",
            Self::VersionTooOld => "Codex version is older than the minimum supported version",
            Self::AccountInvalid => "Codex account result is invalid",
            Self::Protocol => "Codex protocol exchange failed",
        })
    }
}

impl std::error::Error for CodexProbeError {}

/// The account type carried by a valid `account/read` result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodexAccountType {
    /// API-key backed account.
    ApiKey,
    /// ChatGPT-backed account.
    ChatGpt,
    /// Amazon Bedrock-backed account.
    AmazonBedrock,
}

impl CodexAccountType {
    /// Returns the stable wire spelling for this account type.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ApiKey => "apiKey",
            Self::ChatGpt => "chatgpt",
            Self::AmazonBedrock => "amazonBedrock",
        }
    }
}

/// A decoded `account/read` result.
///
/// Identifying material (email, plan metadata, credential sources) is not
/// retained; only the account kind and the `requiresOpenaiAuth` flag survive.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CodexAccountRead {
    account: Option<CodexAccountType>,
    requires_openai_auth: bool,
}

impl CodexAccountRead {
    /// Returns the decoded account type, when one is active.
    #[must_use]
    pub const fn account(self) -> Option<CodexAccountType> {
        self.account
    }

    /// Returns whether the server requires OpenAI authentication.
    #[must_use]
    pub const fn requires_openai_auth(self) -> bool {
        self.requires_openai_auth
    }
}

/// Authentication readiness derived from a decoded `account/read` result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodexAuthState {
    /// An account is active; the payload is the stable account-type label.
    Authenticated {
        /// Stable account-type label (`apiKey`, `chatgpt`, `amazonBedrock`).
        account_type: &'static str,
    },
    /// No account is active; the payload is a human-readable reason.
    Unauthenticated {
        /// Human-readable reason without secrets or paths.
        reason: &'static str,
    },
}

impl CodexAuthState {
    /// Returns whether this state counts as authenticated.
    #[must_use]
    pub const fn is_authenticated(self) -> bool {
        matches!(self, Self::Authenticated { .. })
    }
}

/// Non-billable Codex readiness: installed version plus auth state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodexReadiness {
    authentication: CodexAuthState,
    ready: bool,
    version: CodexVersion,
}

impl CodexReadiness {
    /// Returns the authentication readiness.
    #[must_use]
    pub const fn authentication(&self) -> CodexAuthState {
        self.authentication
    }

    /// Returns whether the engine is ready (authenticated with a supported version).
    #[must_use]
    pub const fn ready(&self) -> bool {
        self.ready
    }

    /// Returns the parsed installed version.
    #[must_use]
    pub fn version(&self) -> &str {
        self.version.as_str()
    }
}

/// A parsed `major.minor.patch` Codex version with its bound checked.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodexVersion {
    value: String,
}

impl CodexVersion {
    /// Returns the `major.minor.patch` text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.value
    }

    /// Returns the minimum supported CLI version.
    #[must_use]
    pub const fn minimum_supported() -> &'static str {
        CODEX_MINIMUM_CLI_VERSION
    }

    /// Returns the verified continuation CLI version.
    #[must_use]
    pub const fn continuation_verified() -> &'static str {
        CODEX_CONTINUATION_CLI_VERSION
    }
}

/// Decodes one bounded `account/read` result document.
///
/// Mirrors `CodexAccountReadSchema`: the `account` key must be present with a
/// null or one of the `apiKey` / `chatgpt` / `amazonBedrock` shapes, and
/// `requiresOpenaiAuth` must be a boolean. Required fields follow the Effect
/// schema; unknown fields are ignored so forward-compatible additions keep
/// decoding, matching the schema's default excess-field policy.
///
/// # Errors
///
/// Returns [`CodexProbeError::AccountInvalid`] when the document exceeds
/// [`CODEX_ACCOUNT_OUTPUT_BOUND_BYTES`], is not JSON, or does not match the
/// account schema. [`CodexProbeError::OutputTooLarge`] when the byte bound is
/// exceeded. No secrets or paths are retained in the error.
pub fn parse_codex_account_read(bytes: &[u8]) -> Result<CodexAccountRead, CodexProbeError> {
    if bytes.len() > CODEX_ACCOUNT_OUTPUT_BOUND_BYTES {
        return Err(CodexProbeError::OutputTooLarge);
    }
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| CodexProbeError::AccountInvalid)?;
    decode_account_value(&value)
}

/// Decodes an `account/read` result value.
///
/// # Errors
///
/// Returns [`CodexProbeError::AccountInvalid`] when the value does not match
/// the account schema.
pub(crate) fn decode_account_value(
    value: &serde_json::Value,
) -> Result<CodexAccountRead, CodexProbeError> {
    let object = value.as_object().ok_or(CodexProbeError::AccountInvalid)?;
    let requires_openai_auth = object
        .get("requiresOpenaiAuth")
        .and_then(serde_json::Value::as_bool)
        .ok_or(CodexProbeError::AccountInvalid)?;
    let account = match object.get("account") {
        Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::Object(account)) => Some(decode_account_object(account)?),
        Some(_) | None => return Err(CodexProbeError::AccountInvalid),
    };
    Ok(CodexAccountRead {
        account,
        requires_openai_auth,
    })
}

fn decode_account_object(
    account: &serde_json::Map<String, serde_json::Value>,
) -> Result<CodexAccountType, CodexProbeError> {
    let account_type = account
        .get("type")
        .and_then(serde_json::Value::as_str)
        .ok_or(CodexProbeError::AccountInvalid)?;
    match account_type {
        "apiKey" => Ok(CodexAccountType::ApiKey),
        "chatgpt" => {
            match account.get("email") {
                Some(serde_json::Value::Null) | Some(serde_json::Value::String(_)) => {}
                Some(_) | None => return Err(CodexProbeError::AccountInvalid),
            }
            if !account.contains_key("planType") {
                return Err(CodexProbeError::AccountInvalid);
            }
            Ok(CodexAccountType::ChatGpt)
        }
        "amazonBedrock" => {
            if !account.contains_key("credentialSource") {
                return Err(CodexProbeError::AccountInvalid);
            }
            Ok(CodexAccountType::AmazonBedrock)
        }
        _ => Err(CodexProbeError::AccountInvalid),
    }
}

/// Classifies authentication readiness from a decoded account result.
///
/// Mirrors the TypeScript probe: any active account type counts as
/// authenticated; otherwise the reason follows `requiresOpenaiAuth`.
#[must_use]
pub const fn classify_codex_auth(account: CodexAccountRead) -> CodexAuthState {
    match account.account {
        Some(CodexAccountType::ApiKey) => CodexAuthState::Authenticated {
            account_type: "apiKey",
        },
        Some(CodexAccountType::ChatGpt) => CodexAuthState::Authenticated {
            account_type: "chatgpt",
        },
        Some(CodexAccountType::AmazonBedrock) => CodexAuthState::Authenticated {
            account_type: "amazonBedrock",
        },
        None => {
            if account.requires_openai_auth {
                CodexAuthState::Unauthenticated {
                    reason: "OpenAI authentication required",
                }
            } else {
                CodexAuthState::Unauthenticated {
                    reason: "No ChatGPT or API-key account is active",
                }
            }
        }
    }
}

/// Validates parsed `--version` bytes into a supported [`CodexVersion`].
///
/// # Errors
///
/// Returns [`CodexProbeError::VersionUnparseable`] when no semantic version
/// is present and [`CodexProbeError::VersionTooOld`] when the version is
/// below the minimum. Captured output is never echoed in the error.
pub fn validate_codex_version_output(output: &[u8]) -> Result<CodexVersion, CodexProbeError> {
    let version = parse_codex_version(output).ok_or(CodexProbeError::VersionUnparseable)?;
    if !meets_minimum_version(&version) {
        return Err(CodexProbeError::VersionTooOld);
    }
    Ok(CodexVersion { value: version })
}

/// Builds non-billable readiness from a validated version and account.
///
/// `ready` is true only for an authenticated account; an absent account
/// yields `Unauthenticated`, never a failure.
#[must_use]
pub fn codex_readiness(version: CodexVersion, account: CodexAccountRead) -> CodexReadiness {
    let authentication = classify_codex_auth(account);
    let ready = authentication.is_authenticated();
    CodexReadiness {
        authentication,
        ready,
        version,
    }
}

/// Runs the bounded, non-billable `codex --version` probe.
///
/// Spawns `executable [...extra_args] --version` with no shell and piped
/// stdio, drains both streams concurrently with a per-stream byte bound, and
/// enforces one end-to-end deadline. The child is killed and reaped on every
/// error path, so pipe-flooding or long-running children surface as
/// [`CodexProbeError::OutputTooLarge`] or [`CodexProbeError::Timeout`]
/// instead of hanging. No prompt, session, or account mutation is performed.
///
/// # Errors
///
/// Returns [`CodexProbeError::InvalidBinary`] when the executable cannot
/// start, [`CodexProbeError::Timeout`] when the deadline passes,
/// [`CodexProbeError::OutputTooLarge`] when either stream exceeds
/// `max_bytes`, and [`CodexProbeError::Unavailable`] for spawn-handle or
/// stream failures and non-zero exits. Captured output is never echoed.
pub fn run_codex_version(
    executable: &Path,
    extra_args: &[String],
    timeout: Duration,
    max_bytes: usize,
) -> Result<Vec<u8>, CodexProbeError> {
    let deadline = probe_deadline(timeout)?;
    let mut argv = extra_args.to_vec();
    argv.push("--version".to_owned());
    let mut child = spawn_probe_child(executable, &argv, None)?;
    drop(child.stdin.take());
    let stdout_pipe = take_child_stdout(&mut child)?;
    let stderr_pipe = take_child_stderr(&mut child)?;
    let mut stdout_drain = PipeDrain::spawn_pipe(stdout_pipe, max_bytes);
    let mut stderr_drain = PipeDrain::spawn_pipe(stderr_pipe, max_bytes);
    let outcome = drive_version_drain(&mut child, &mut stdout_drain, &mut stderr_drain, deadline);
    stdout_drain.join_or_detach();
    stderr_drain.join_or_detach();
    outcome
}

fn drive_version_drain(
    child: &mut std::process::Child,
    stdout_drain: &mut PipeDrain,
    stderr_drain: &mut PipeDrain,
    deadline: Instant,
) -> Result<Vec<u8>, CodexProbeError> {
    let mut output = Vec::new();
    loop {
        match pump_version_stdout(stdout_drain, &mut output) {
            Err(error) => {
                stop_child(child);
                return Err(error);
            }
            Ok(true) => {
                let status = reap_or_kill(child, deadline)?;
                return map_version_exit(status, output);
            }
            Ok(false) => {}
        }
        match pump_version_stderr(stderr_drain) {
            Err(error) => {
                stop_child(child);
                return Err(error);
            }
            Ok(()) => {}
        }
        if Instant::now() >= deadline {
            stop_child(child);
            return Err(CodexProbeError::Timeout);
        }
        thread::sleep(PROBE_POLL_INTERVAL);
    }
}

/// Pumps available stdout events; returns true once the stream ends.
///
/// # Errors
///
/// Returns [`CodexProbeError::OutputTooLarge`] when the stream exceeds its
/// bound and [`CodexProbeError::Unavailable`] when the stream fails. The
/// caller kills and reaps the child.
fn pump_version_stdout(
    drain: &mut PipeDrain,
    output: &mut Vec<u8>,
) -> Result<bool, CodexProbeError> {
    while let Ok(event) = drain.events().try_recv() {
        match event {
            PipeEvent::Line(line) => output.extend_from_slice(&line),
            PipeEvent::Eof => return Ok(true),
            PipeEvent::TooLarge => return Err(CodexProbeError::OutputTooLarge),
            PipeEvent::Io => return Err(CodexProbeError::Unavailable),
        }
    }
    Ok(false)
}

/// Pumps and discards available stderr events while enforcing its bound.
///
/// # Errors
///
/// Returns [`CodexProbeError::OutputTooLarge`] when the stream exceeds its
/// bound and [`CodexProbeError::Unavailable`] when the stream fails.
fn pump_version_stderr(drain: &mut PipeDrain) -> Result<(), CodexProbeError> {
    while let Ok(event) = drain.events().try_recv() {
        match event {
            PipeEvent::Line(_) => {}
            PipeEvent::Eof => {}
            PipeEvent::TooLarge => return Err(CodexProbeError::OutputTooLarge),
            PipeEvent::Io => return Err(CodexProbeError::Unavailable),
        }
    }
    Ok(())
}

fn map_version_exit(
    status: std::process::ExitStatus,
    output: Vec<u8>,
) -> Result<Vec<u8>, CodexProbeError> {
    if status.success() {
        Ok(output)
    } else {
        Err(CodexProbeError::Unavailable)
    }
}
