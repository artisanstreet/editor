//! Finite native Codex readiness probe.
//!
//! Bounded, non-billable readiness only: run `codex --version` with byte and
//! time bounds, parse the installed version, and classify a supplied
//! `account/read` result into authenticated versus unauthenticated readiness.
//! No prompt is sent, no model inference runs, and no persistent
//! account/session state is changed.
//!
//! CLI text (such as a login-status message) is never treated as account
//! evidence. Only a decoded `account/read` result counts; everything else
//! stays `Unauthenticated` or a typed probe failure. Auth absence
//! (`Unauthenticated`) is distinct from `Unavailable`, `Timeout`, and
//! `InvalidBinary` failures, and every error is path- and secret-free.

use std::fmt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

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
    /// An `account/read` document was malformed or used an unknown shape.
    AccountInvalid,
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

/// The decoded `account/read` result.
///
/// identifying material (email, plan metadata, credential sources) is
/// deliberately not retained; only the account kind and the
/// `requiresOpenaiAuth` flag survive decoding.
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CodexReadiness {
    authentication: CodexAuthState,
    ready: bool,
    version: CodexVersion,
}

impl CodexReadiness {
    /// Returns the authentication readiness.
    #[must_use]
    pub const fn authentication(self) -> CodexAuthState {
        self.authentication
    }

    /// Returns whether the engine is ready (authenticated with a supported version).
    #[must_use]
    pub const fn ready(self) -> bool {
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
/// Mirrors `CodexAccountReadSchema`: `account` is null or one of
/// `apiKey` / `chatgpt` / `amazonBedrock` with their required fields, and
/// `requiresOpenaiAuth` is a required boolean. Unknown top-level or
/// account fields, unknown account types, and trailing content are
/// rejected as [`CodexProbeError::AccountInvalid`].
///
/// # Errors
///
/// Returns [`CodexProbeError::AccountInvalid`] when the document exceeds
/// [`CODEX_ACCOUNT_OUTPUT_BOUND_BYTES`], is not JSON, or does not match the
/// account schema. No secrets or paths are retained in the error.
pub fn parse_codex_account_read(bytes: &[u8]) -> Result<CodexAccountRead, CodexProbeError> {
    if bytes.len() > CODEX_ACCOUNT_OUTPUT_BOUND_BYTES {
        return Err(CodexProbeError::OutputTooLarge);
    }
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| CodexProbeError::AccountInvalid)?;
    let object = value.as_object().ok_or(CodexProbeError::AccountInvalid)?;
    if object.len() != 2
        || !object.contains_key("account")
        || !object.contains_key("requiresOpenaiAuth")
    {
        return Err(CodexProbeError::AccountInvalid);
    }
    let requires_openai_auth = object
        .get("requiresOpenaiAuth")
        .and_then(serde_json::Value::as_bool)
        .ok_or(CodexProbeError::AccountInvalid)?;
    let account = match object.get("account") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::Object(account)) => Some(parse_account_object(account)?),
        Some(_) => return Err(CodexProbeError::AccountInvalid),
    };
    Ok(CodexAccountRead {
        account,
        requires_openai_auth,
    })
}

fn parse_account_object(
    account: &serde_json::Map<String, serde_json::Value>,
) -> Result<CodexAccountType, CodexProbeError> {
    let account_type = account
        .get("type")
        .and_then(serde_json::Value::as_str)
        .ok_or(CodexProbeError::AccountInvalid)?;
    match account_type {
        "apiKey" => {
            if account.len() != 1 {
                return Err(CodexProbeError::AccountInvalid);
            }
            Ok(CodexAccountType::ApiKey)
        }
        "chatgpt" => {
            if account.len() != 3
                || !account.contains_key("email")
                || !account.contains_key("planType")
            {
                return Err(CodexProbeError::AccountInvalid);
            }
            match account.get("email") {
                Some(serde_json::Value::Null) | Some(serde_json::Value::String(_)) => {}
                _ => return Err(CodexProbeError::AccountInvalid),
            }
            Ok(CodexAccountType::ChatGpt)
        }
        "amazonBedrock" => {
            if account.len() != 2 || !account.contains_key("credentialSource") {
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
/// below the minimum. Captured output itself is never echoed in the error.
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

/// Fixture outcome for the `--version` spawn, for tests and later runners.
///
/// `exit_code` is `None` when the child never reported a normal exit;
/// `stdout_truncated` records that the stream exceeded its byte bound before
/// `EOF`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodexVersionFixture {
    /// Normal process exit code, when one was reported.
    pub exit_code: Option<i32>,
    /// Captured stdout bytes (already bounded by the caller).
    pub stdout: Vec<u8>,
    /// Whether the stream exceeded its byte bound.
    pub stdout_truncated: bool,
    /// Whether the spawn exceeded its deadline.
    pub timed_out: bool,
}

/// Classifies a `--version` fixture into captured bytes or a typed failure.
///
/// Precedence mirrors the runner: timeout first, then output bound, then
/// non-zero exit. This keeps timeout/output-bound/readiness-failure tests
/// hermetic without spawning fixture processes.
///
/// # Errors
///
/// Returns [`CodexProbeError::Timeout`], [`CodexProbeError::OutputTooLarge`],
/// or [`CodexProbeError::Unavailable`] per the fixture. Output bytes are
/// never echoed in the error.
pub fn classify_version_fixture(fixture: CodexVersionFixture) -> Result<Vec<u8>, CodexProbeError> {
    if fixture.timed_out {
        return Err(CodexProbeError::Timeout);
    }
    if fixture.stdout_truncated || fixture.stdout.len() > CODEX_VERSION_OUTPUT_BOUND_BYTES {
        return Err(CodexProbeError::OutputTooLarge);
    }
    match fixture.exit_code {
        Some(0) => Ok(fixture.stdout),
        Some(_) | None => Err(CodexProbeError::Unavailable),
    }
}

/// Runs the bounded, non-billable `codex --version` probe.
///
/// Spawns `executable [--extra_args] --version` with piped stdio and a null
/// stdin, waits up to `timeout`, kills the child on deadline, enforces
/// [`CODEX_VERSION_OUTPUT_BOUND_BYTES`] via `max_bytes`, and maps a
/// non-zero exit to [`CodexProbeError::Unavailable`]. Paths containing
/// spaces are passed as an argv entry (never a shell string). No prompt,
/// session, or account mutation is performed.
///
/// # Errors
///
/// Returns [`CodexProbeError::InvalidBinary`] when the executable cannot be
/// spawned, [`CodexProbeError::Timeout`] on deadline,
/// [`CodexProbeError::OutputTooLarge`] when either stream exceeds `max_bytes`,
/// and [`CodexProbeError::Unavailable`] for a non-zero exit. Captured output
/// is never echoed in an error.
pub fn run_codex_version(
    executable: &Path,
    extra_args: &[String],
    timeout: Duration,
    max_bytes: usize,
) -> Result<Vec<u8>, CodexProbeError> {
    let mut command = Command::new(executable);
    command.args(extra_args);
    command.arg("--version");
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command
        .spawn()
        .map_err(|_| CodexProbeError::InvalidBinary)?;
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or(CodexProbeError::Timeout)?;
    loop {
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(CodexProbeError::Timeout);
        }
        match child.try_wait().map_err(|_| CodexProbeError::Unavailable)? {
            Some(_) => break,
            None => thread::sleep(Duration::from_millis(10)),
        }
    }
    let output = child
        .wait_with_output()
        .map_err(|_| CodexProbeError::Unavailable)?;
    if output.stdout.len() > max_bytes || output.stderr.len() > max_bytes {
        return Err(CodexProbeError::OutputTooLarge);
    }
    if !output.status.success() {
        return Err(CodexProbeError::Unavailable);
    }
    Ok(output.stdout)
}
