//! Dependency-free policy for enumerating execution machines.
//!
//! The runtime adapter owns platform and environment reads, hostname lookup,
//! command execution, and the five-second wait. This module only describes
//! the command the adapter may run, parses already-captured output, and
//! projects the supplied observations into protocol-shaped machine values.
//! It never reads process state, starts a process, applies a timeout, or
//! depends on an asynchronous runtime.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use std::time::Duration;

/// The exact bounded wait used by the TypeScript WSL enumeration adapter, in
/// milliseconds.
pub const WSL_ENUMERATION_TIMEOUT_MS: u64 = 5_000;

/// The exact bounded wait used by the TypeScript WSL enumeration adapter.
pub const WSL_ENUMERATION_TIMEOUT: Duration = Duration::from_millis(WSL_ENUMERATION_TIMEOUT_MS);

/// The executable used to enumerate WSL distributions.
pub const WSL_ENUMERATION_COMMAND: &str = "wsl.exe";

/// The shell-free arguments used to enumerate WSL distributions.
pub const WSL_ENUMERATION_ARGS: [&str; 2] = ["-l", "-q"];

const UTILITY_DISTRIBUTIONS: [&str; 4] = [
    "docker-desktop",
    "docker-desktop-data",
    "rancher-desktop",
    "rancher-desktop-data",
];

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

/// A shell-free command descriptor for one WSL distribution enumeration.
///
/// The command runner owns process execution and timeout enforcement. The
/// descriptor is only a typed action for the adapter to carry out.
#[must_use]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WslEnumerationCommand {
    /// Executable passed to the process runner.
    pub command: String,
    /// Individual arguments passed without shell parsing.
    pub args: Vec<String>,
    /// Maximum wait for command completion, in milliseconds.
    pub timeout_ms: u64,
}

impl WslEnumerationCommand {
    /// Creates the exact `wsl.exe -l -q` enumeration descriptor.
    #[must_use = "retain the WSL enumeration command"]
    pub fn new() -> Self {
        Self {
            args: WSL_ENUMERATION_ARGS
                .iter()
                .map(|argument| (*argument).to_owned())
                .collect(),
            command: WSL_ENUMERATION_COMMAND.to_owned(),
            timeout_ms: WSL_ENUMERATION_TIMEOUT_MS,
        }
    }

    /// Returns the command wait as a standard-library duration.
    #[must_use]
    pub const fn timeout(&self) -> Duration {
        Duration::from_millis(self.timeout_ms)
    }
}

impl Default for WslEnumerationCommand {
    fn default() -> Self {
        Self::new()
    }
}

/// Builds the exact shell-free WSL enumeration command descriptor.
#[must_use = "retain the WSL enumeration command"]
pub fn build_wsl_enumeration_command() -> WslEnumerationCommand {
    WslEnumerationCommand::new()
}

/// The typed host operation selected from one supplied platform fact.
#[must_use]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HostMachinesAction {
    /// No WSL command is needed on this platform.
    SkipWslEnumeration,
    /// Ask the platform adapter to run the exact WSL enumeration command.
    EnumerateWsl {
        /// Command and bounded wait to hand to the adapter.
        command: WslEnumerationCommand,
    },
}

impl HostMachinesAction {
    /// Selects whether this platform needs WSL enumeration.
    #[must_use = "handle the host-machine action"]
    pub fn for_platform(platform: HostPlatform) -> Self {
        if platform == HostPlatform::Win32 {
            Self::EnumerateWsl {
                command: build_wsl_enumeration_command(),
            }
        } else {
            Self::SkipWslEnumeration
        }
    }

    /// Returns the command to execute, if this action enumerates WSL.
    #[must_use]
    pub const fn command(&self) -> Option<&WslEnumerationCommand> {
        match self {
            Self::SkipWslEnumeration => None,
            Self::EnumerateWsl { command } => Some(command),
        }
    }
}

/// Selects the typed command action for one already-observed platform.
#[must_use = "handle the host-machine action"]
pub fn enumeration_action(platform: HostPlatform) -> HostMachinesAction {
    HostMachinesAction::for_platform(platform)
}

/// The completion observed from the caller-owned command runner.
///
/// Failure and timeout intentionally carry no platform error. Both are
/// equivalent to an empty WSL list at this policy boundary.
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

/// Extracts distribution names from already-captured `wsl.exe -l -q` output.
///
/// The command emits UTF-16LE, which a UTF-8 capture can render as
/// interleaved NUL bytes. NULs, BOMs, and carriage returns are removed before
/// lines are trimmed. Empty lines are dropped, utility distributions are
/// removed case-insensitively, and every other line remains in its original
/// order with duplicates intact.
#[must_use = "use the parsed WSL distributions"]
pub fn parse_wsl_distributions(raw_output: &str) -> Vec<String> {
    raw_output
        .replace(['\0', '\u{feff}', '\r'], "")
        .split('\n')
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| {
            !UTILITY_DISTRIBUTIONS
                .iter()
                .any(|utility| line.eq_ignore_ascii_case(utility))
        })
        .map(str::to_owned)
        .collect()
}

/// Resolves the retained WSL list from one platform and one command outcome.
///
/// WSL entries are a Windows-only surface. A successful Windows completion is
/// parsed; a failed or timed-out completion, and every non-Windows platform,
/// produce an empty list. The caller remains responsible for supplying the
/// completion fact, so this function never starts or waits for a process.
#[must_use = "use the resolved WSL distributions"]
pub fn resolve_wsl_distributions(
    platform: HostPlatform,
    runner_outcome: &CommandRunnerOutcome,
) -> Vec<String> {
    if platform != HostPlatform::Win32 {
        return Vec::new();
    }

    match runner_outcome {
        CommandRunnerOutcome::Succeeded(raw_output) => parse_wsl_distributions(raw_output),
        CommandRunnerOutcome::Failed | CommandRunnerOutcome::TimedOut => Vec::new(),
    }
}

/// Distinguishes the execution-machine kinds exposed by the protocol.
#[must_use]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum HostMachineKind {
    /// The machine hosting the currently connected Forge.
    Local,
    /// A WSL2 distribution that can host another Forge.
    Wsl,
}

impl HostMachineKind {
    /// Returns the exact protocol spelling of this machine kind.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Wsl => "wsl",
        }
    }
}

/// One machine available to execute threads on the connected Forge host.
#[must_use]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct HostMachineSnapshot {
    /// Secondary display text, such as a hostname or WSL distribution name.
    pub detail: Option<String>,
    /// Stable machine identifier used by a connect request.
    pub id: String,
    /// Whether this is the current local Forge host or a WSL peer.
    pub kind: HostMachineKind,
    /// Primary human-readable display text.
    pub label: String,
}

impl HostMachineSnapshot {
    /// Creates a machine with no secondary detail.
    #[must_use = "retain the machine snapshot"]
    pub fn new(id: impl Into<String>, kind: HostMachineKind, label: impl Into<String>) -> Self {
        Self {
            detail: None,
            id: id.into(),
            kind,
            label: label.into(),
        }
    }

    /// Adds the optional secondary display detail to a machine value.
    #[must_use = "retain the machine snapshot"]
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

/// Machines available to execute threads, with the local machine first.
#[must_use]
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct HostMachinesSnapshot {
    /// Machines in protocol order; the local machine is always first.
    pub machines: Vec<HostMachineSnapshot>,
}

impl HostMachinesSnapshot {
    /// Creates a snapshot from one ordered machine list.
    #[must_use = "retain the machine snapshot"]
    pub fn new(machines: Vec<HostMachineSnapshot>) -> Self {
        Self { machines }
    }

    /// Returns the retained machine list without copying it.
    pub fn as_slice(&self) -> &[HostMachineSnapshot] {
        &self.machines
    }
}

/// Builds the protocol-shaped machine list from supplied host facts.
///
/// A Linux Forge with `WSL_DISTRO_NAME` uses that distribution as the local
/// detail and receives the WSL2 label. All other local hosts use the supplied
/// hostname and the ordinary label. WSL entries are projected only for
/// Windows, in the supplied order and with supplied duplicates intact.
#[must_use = "retain the machine snapshot"]
pub fn build_machines_snapshot<I, S>(
    platform: HostPlatform,
    hostname: impl Into<String>,
    wsl_distribution: Option<&str>,
    distributions: I,
) -> HostMachinesSnapshot
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let hostname = hostname.into();
    let local = if platform == HostPlatform::Linux && wsl_distribution.is_some() {
        HostMachineSnapshot {
            detail: wsl_distribution.map(str::to_owned),
            id: "local".to_owned(),
            kind: HostMachineKind::Local,
            label: "This computer on WSL2".to_owned(),
        }
    } else {
        HostMachineSnapshot {
            detail: Some(hostname),
            id: "local".to_owned(),
            kind: HostMachineKind::Local,
            label: "This computer".to_owned(),
        }
    };

    let mut machines = vec![local];
    if platform == HostPlatform::Win32 {
        machines.extend(distributions.into_iter().map(|distribution| {
            let detail = distribution.as_ref().to_owned();
            HostMachineSnapshot {
                id: format!("wsl:{detail}"),
                detail: Some(detail),
                kind: HostMachineKind::Wsl,
                label: "This computer on WSL2".to_owned(),
            }
        }));
    }

    HostMachinesSnapshot::new(machines)
}

/// Projects a snapshot from already-resolved distributions.
#[must_use = "retain the machine snapshot"]
pub fn project_snapshot<I, S>(
    hostname: impl Into<String>,
    platform: HostPlatform,
    wsl_distribution: Option<&str>,
    distributions: I,
) -> HostMachinesSnapshot
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    build_machines_snapshot(platform, hostname, wsl_distribution, distributions)
}

/// Projects a snapshot from one contained command completion.
///
/// This is the complete process-free counterpart of the runtime's query
/// construction. It preserves the supplied hostname and WSL environment
/// value exactly, while command failure and timeout become an empty WSL list.
#[must_use = "retain the machine snapshot"]
pub fn project_snapshot_from_runner(
    hostname: impl Into<String>,
    platform: HostPlatform,
    wsl_distribution: Option<&str>,
    runner_outcome: &CommandRunnerOutcome,
) -> HostMachinesSnapshot {
    let distributions = resolve_wsl_distributions(platform, runner_outcome);
    project_snapshot(hostname, platform, wsl_distribution, distributions)
}

/// Stateless entry point for the host-machines policy.
#[must_use]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HostMachinesPolicy;

impl HostMachinesPolicy {
    /// Creates the stateless host-machines policy value.
    #[must_use = "retain the stateless host-machines policy"]
    pub const fn new() -> Self {
        Self
    }

    /// Selects the host operation for one already-observed platform.
    #[must_use = "handle the host-machine action"]
    pub fn enumeration_action(platform: HostPlatform) -> HostMachinesAction {
        enumeration_action(platform)
    }

    /// Maps supplied runtime facts into a machine snapshot without I/O.
    #[must_use = "retain the evaluated machine snapshot"]
    pub fn evaluate(
        hostname: impl Into<String>,
        runtime_platform: &str,
        wsl_distribution: Option<&str>,
        runner_outcome: &CommandRunnerOutcome,
    ) -> HostMachinesSnapshot {
        project_snapshot_from_runner(
            hostname,
            map_host_platform(runtime_platform),
            wsl_distribution,
            runner_outcome,
        )
    }
}
