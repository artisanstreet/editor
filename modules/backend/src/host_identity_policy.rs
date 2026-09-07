//! Dependency-free policy for resolving and projecting host identity.
//!
//! The runtime adapter owns platform and operating-system observations and the
//! command runner owns process execution. This module only maps the observed
//! platform, describes the exact display-name command, parses an already
//! captured result, and projects a protocol-shaped snapshot. It never reads
//! process state, starts a process, applies a timeout, caches a result, or
//! depends on an asynchronous runtime.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

/// The closed host-platform vocabulary used by the protocol schema.
#[must_use]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum HostPlatform {
    /// A Windows host reported by Node as `win32`.
    Win32,
    /// An Apple macOS host reported by Node as `darwin`.
    Darwin,
    /// A Linux host reported by Node as `linux`.
    Linux,
    /// Any platform string outside the closed protocol vocabulary.
    Unknown,
}

impl HostPlatform {
    /// Maps one runtime platform string to the closed protocol vocabulary.
    #[must_use = "use the mapped host platform"]
    pub fn from_runtime(value: &str) -> Self {
        map_host_platform(value)
    }

    /// Returns the exact protocol spelling for this platform.
    #[must_use = "use the protocol platform spelling"]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Win32 => "win32",
            Self::Darwin => "darwin",
            Self::Linux => "linux",
            Self::Unknown => "unknown",
        }
    }
}

/// Maps Node's runtime platform spelling to the protocol's closed literal.
///
/// Matching is exact and case-sensitive. Only `win32`, `darwin`, and `linux`
/// are supported; every other value becomes [`HostPlatform::Unknown`].
#[must_use = "use the mapped host platform"]
pub fn map_host_platform(value: &str) -> HostPlatform {
    match value {
        "win32" => HostPlatform::Win32,
        "darwin" => HostPlatform::Darwin,
        "linux" => HostPlatform::Linux,
        _ => HostPlatform::Unknown,
    }
}

/// An exact shell-free command descriptor for one display-name lookup.
#[must_use]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DisplayNameCommand {
    /// Executable passed to the process runner.
    pub command: String,
    /// Individual arguments passed without shell parsing.
    pub args: Vec<String>,
}

impl DisplayNameCommand {
    /// Creates a command descriptor from an executable and individual args.
    #[must_use = "retain the command descriptor"]
    pub fn new<I, S>(command: impl Into<String>, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            command: command.into(),
            args: args.into_iter().map(Into::into).collect(),
        }
    }
}

/// Reports whether a username satisfies the exact Windows interpolation
/// grammar from the TypeScript runtime: `[A-Za-z0-9 ._-]+`.
#[must_use = "use the username safety result"]
pub fn is_safe_windows_username(username: &str) -> bool {
    !username.is_empty()
        && username
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b' ' | b'.' | b'_' | b'-'))
}

/// Builds the platform-specific display-name command without executing it.
///
/// Windows embeds the username in a PowerShell CIM filter, so the exact
/// allow-list from [`is_safe_windows_username`] is required before the script
/// is constructed. Darwin does not interpolate the username. Linux passes the
/// username as one separate `getent` argument, retaining its exact spelling.
/// Missing usernames and unsupported platforms have no command.
#[must_use = "use the optional display-name command"]
pub fn build_display_name_command(
    platform: HostPlatform,
    username: Option<&str>,
) -> Option<DisplayNameCommand> {
    let username = username?;

    match platform {
        HostPlatform::Win32 => {
            if !is_safe_windows_username(username) {
                return None;
            }

            let script = format!(
                "(Get-CimInstance Win32_UserAccount -Filter \"Name='{username}'\" | Select-Object -First 1).FullName"
            );

            Some(DisplayNameCommand::new(
                "powershell.exe",
                ["-NoProfile", "-NonInteractive", "-Command", &script],
            ))
        }
        HostPlatform::Darwin => Some(DisplayNameCommand::new("id", ["-F"])),
        HostPlatform::Linux => Some(DisplayNameCommand::new("getent", ["passwd", username])),
        HostPlatform::Unknown => None,
    }
}

/// The bounded observation returned by a display-name command runner.
///
/// The runner adapter remains responsible for starting, stopping, and timing
/// out a process. A failure or timeout deliberately carries no platform error
/// across this policy boundary because both outcomes mean that the optional
/// display name is absent.
#[must_use]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CommandRunnerOutcome {
    /// The command completed and returned captured standard output.
    Succeeded(String),
    /// The command failed without usable output.
    Failed,
    /// The runner's bounded wait elapsed before command completion.
    TimedOut,
}

impl CommandRunnerOutcome {
    /// Creates a successful runner observation with exact captured output.
    #[must_use = "retain the contained runner outcome"]
    pub fn succeeded(output: impl Into<String>) -> Self {
        Self::Succeeded(output.into())
    }

    /// Creates a contained command-failure observation.
    #[must_use = "retain the contained failure outcome"]
    pub const fn failed() -> Self {
        Self::Failed
    }

    /// Creates a contained command-timeout observation.
    #[must_use = "retain the contained timeout outcome"]
    pub const fn timed_out() -> Self {
        Self::TimedOut
    }
}

/// Extracts a display name from already-captured command output.
///
/// Every output first receives Unicode whitespace trimming. Linux `getent`
/// output is a colon-separated passwd row, so the fifth field is selected and
/// then truncated at its first comma before a second trim. Other platforms
/// return the trimmed output directly. The command-resolution boundary keeps
/// unsupported platforms from accepting output because they have no command.
#[must_use = "use the optional parsed display name"]
pub fn parse_display_name(platform: HostPlatform, raw_output: &str) -> Option<String> {
    let trimmed = raw_output.trim();
    if trimmed.is_empty() {
        return None;
    }

    if platform == HostPlatform::Linux {
        let gecos = trimmed.split(':').nth(4)?.split(',').next()?.trim();
        return (!gecos.is_empty()).then(|| gecos.to_owned());
    }

    Some(trimmed.to_owned())
}

/// Resolves the optional display name from one contained runner observation.
///
/// This function only evaluates data supplied by the caller. It emits no
/// command, applies no timeout, and performs no process or operating-system
/// access. Missing or unsafe Windows usernames, unsupported platforms,
/// malformed/empty output, command failure, and timeout all produce `None`.
#[must_use = "use the optional resolved display name"]
pub fn resolve_display_name(
    platform: HostPlatform,
    username: Option<&str>,
    runner_outcome: &CommandRunnerOutcome,
) -> Option<String> {
    let _command = build_display_name_command(platform, username)?;

    match runner_outcome {
        CommandRunnerOutcome::Succeeded(raw_output) => parse_display_name(platform, raw_output),
        CommandRunnerOutcome::Failed | CommandRunnerOutcome::TimedOut => None,
    }
}

/// The protocol-shaped host identity snapshot projected by this policy.
///
/// `hostname` and the optional `username` are copied exactly as supplied.
/// The display name is optional because lookup is best-effort. This value is
/// only a projection; protocol validation and runtime acquisition stay outside
/// the dependency-free policy.
#[must_use]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HostIdentitySnapshot {
    /// Optional human profile name from the platform lookup.
    pub display_name: Option<String>,
    /// Exact machine identifier supplied by the caller.
    pub hostname: String,
    /// Closed protocol platform value.
    pub platform: HostPlatform,
    /// Optional exact OS account name supplied by the caller.
    pub username: Option<String>,
}

impl HostIdentitySnapshot {
    /// Projects exact host fields and omits a display name for an unknown
    /// platform.
    #[must_use = "retain the projected host identity snapshot"]
    pub fn new(
        hostname: impl Into<String>,
        platform: HostPlatform,
        username: Option<String>,
        display_name: Option<String>,
    ) -> Self {
        Self {
            display_name: if platform == HostPlatform::Unknown {
                None
            } else {
                display_name
            },
            hostname: hostname.into(),
            platform,
            username,
        }
    }
}

/// Projects exact snapshot fields already resolved by a caller-owned adapter.
#[must_use = "retain the projected host identity snapshot"]
pub fn project_snapshot(
    hostname: impl Into<String>,
    platform: HostPlatform,
    username: Option<String>,
    display_name: Option<String>,
) -> HostIdentitySnapshot {
    HostIdentitySnapshot::new(hostname, platform, username, display_name)
}

/// Resolves a contained runner outcome and projects the resulting snapshot.
///
/// This is the complete process-free counterpart of the runtime's snapshot
/// construction. It preserves the exact hostname and username while deriving
/// only the optional display name from the supplied runner observation.
#[must_use = "retain the projected host identity snapshot"]
pub fn project_snapshot_from_runner(
    hostname: impl Into<String>,
    platform: HostPlatform,
    username: Option<&str>,
    runner_outcome: &CommandRunnerOutcome,
) -> HostIdentitySnapshot {
    let display_name = resolve_display_name(platform, username, runner_outcome);
    project_snapshot(
        hostname,
        platform,
        username.map(str::to_owned),
        display_name,
    )
}

/// Stateless entry point for the host-identity policy.
#[must_use]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HostIdentityPolicy;

impl HostIdentityPolicy {
    /// Creates the stateless policy value.
    #[must_use = "retain the stateless host-identity policy"]
    pub const fn new() -> Self {
        Self
    }

    /// Maps a runtime platform, resolves one contained runner outcome, and
    /// projects a host identity snapshot without retaining any state.
    #[must_use = "retain the evaluated host identity snapshot"]
    pub fn evaluate(
        hostname: impl Into<String>,
        runtime_platform: &str,
        username: Option<&str>,
        runner_outcome: &CommandRunnerOutcome,
    ) -> HostIdentitySnapshot {
        project_snapshot_from_runner(
            hostname,
            map_host_platform(runtime_platform),
            username,
            runner_outcome,
        )
    }
}
