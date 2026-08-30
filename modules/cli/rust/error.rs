use std::{fmt, io, path::PathBuf, process::ExitStatus};

/// The stable numeric exit categories emitted by the native Forge binary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ForgeExitCode {
    ConfigurationCredentials,
    ApplicationStartup,
    ServerReadiness,
    Service,
    RuntimeShutdown,
    ProcessCustody,
    Unknown(i32),
}

impl ForgeExitCode {
    pub const fn from_code(code: i32) -> Self {
        match code {
            64 => Self::ConfigurationCredentials,
            70 => Self::ApplicationStartup,
            71 => Self::ServerReadiness,
            72 => Self::Service,
            73 => Self::RuntimeShutdown,
            75 => Self::ProcessCustody,
            code => Self::Unknown(code),
        }
    }

    pub const fn code(self) -> i32 {
        match self {
            Self::ConfigurationCredentials => 64,
            Self::ApplicationStartup => 70,
            Self::ServerReadiness => 71,
            Self::Service => 72,
            Self::RuntimeShutdown => 73,
            Self::ProcessCustody => 75,
            Self::Unknown(code) => code,
        }
    }

    pub const fn is_known(self) -> bool {
        !matches!(self, Self::Unknown(_))
    }
}

/// The process-level outcome observed while starting native Forge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ForgeTermination {
    Exited(ForgeExitCode),
    Signaled,
}

impl ForgeTermination {
    /// Converts an operating-system exit code without losing unknown values or
    /// the absence of a code used for signal termination.
    pub const fn from_code(code: Option<i32>) -> Self {
        match code {
            Some(code) => Self::Exited(ForgeExitCode::from_code(code)),
            None => Self::Signaled,
        }
    }

    pub fn from_exit_status(status: &ExitStatus) -> Self {
        Self::from_code(status.code())
    }

    pub const fn exit_code(self) -> Option<i32> {
        match self {
            Self::Exited(code) => Some(code.code()),
            Self::Signaled => None,
        }
    }
}

impl fmt::Display for ForgeExitCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConfigurationCredentials => formatter.write_str("64 (configuration/credentials)"),
            Self::ApplicationStartup => formatter.write_str("70 (application startup)"),
            Self::ServerReadiness => formatter.write_str("71 (server/readiness)"),
            Self::Service => formatter.write_str("72 (service)"),
            Self::RuntimeShutdown => formatter.write_str("73 (runtime/shutdown)"),
            Self::ProcessCustody => formatter.write_str("75 (process custody)"),
            Self::Unknown(code) => write!(formatter, "unknown numeric code {code}"),
        }
    }
}

impl fmt::Display for ForgeTermination {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Exited(code) => write!(formatter, "exited with {code}"),
            Self::Signaled => formatter.write_str("terminated by signal without an exit code"),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("Artisan is not installed correctly: {0}")]
    Installation(String),
    #[error("Forge is not configured in this Artisan home; run `ae setup`")]
    MissingInstance,
    #[error("Forge is not running")]
    NotRunning,
    #[error(
        "Forge is doing work ({active_work_count} active model run(s)); refusing idle-only shutdown"
    )]
    ForgeBusy { active_work_count: usize },
    #[error("Forge cannot report active work; refusing idle-only shutdown")]
    ForgeActivityUnavailable,
    #[error("invalid native Forge instance configuration: {0}")]
    NativeInstance(#[from] crate::instance::NativeInstanceError),
    #[error("native Forge credentials are unavailable: {0}")]
    Credentials(#[from] crate::credentials::ForgeCredentialError),
    #[error(
        "native Forge credential manifest does not match the configured instance manifest: {configured} != {credentials}"
    )]
    CredentialManifestMismatch {
        configured: PathBuf,
        credentials: PathBuf,
    },
    #[error("invalid Forge readiness receipt ({reason})")]
    InvalidForgeReadiness { reason: &'static str },
    #[error("Forge did not establish readiness before the startup deadline")]
    ForgeReadinessTimeout,
    #[error("Forge terminated during startup: {termination}")]
    ForgeTerminated { termination: ForgeTermination },
    #[error("native Forge lifecycle control is unavailable until L1")]
    UnsupportedLifecycleControl,
    #[error("Forge control request failed: {0}")]
    Control(String),
    #[error("unsupported operation: {0}")]
    Unsupported(String),
    #[error("development build guard: {0}")]
    DebugBuildGuard(String),
    #[error("refusing unsafe filesystem operation on {0}")]
    UnsafePath(PathBuf),
    #[error("{context}: {source}")]
    Io {
        context: &'static str,
        #[source]
        source: io::Error,
    },
    #[error("invalid JSON in {path}: {source}")]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

impl CliError {
    pub const fn exit_code(&self) -> i32 {
        match self {
            Self::MissingInstance | Self::NotRunning => 3,
            Self::Installation(_) => 4,
            Self::ForgeBusy { .. } => 5,
            Self::ForgeActivityUnavailable => 6,
            Self::ForgeReadinessTimeout => 71,
            Self::ForgeTerminated { termination } => termination
                .exit_code()
                .filter(|code| *code > 0)
                .unwrap_or(1),
            _ => 1,
        }
    }
}

pub type Result<T> = std::result::Result<T, CliError>;

pub(crate) fn io(context: &'static str) -> impl FnOnce(io::Error) -> CliError {
    move |source| CliError::Io { context, source }
}

#[cfg(test)]
mod tests {
    use super::{CliError, ForgeExitCode, ForgeTermination};

    #[test]
    fn known_forge_exit_codes_remain_known_and_unchanged() {
        for (code, known) in [
            (64, ForgeExitCode::ConfigurationCredentials),
            (70, ForgeExitCode::ApplicationStartup),
            (71, ForgeExitCode::ServerReadiness),
            (72, ForgeExitCode::Service),
            (73, ForgeExitCode::RuntimeShutdown),
            (75, ForgeExitCode::ProcessCustody),
        ] {
            let termination = ForgeTermination::from_code(Some(code));
            assert_eq!(termination, ForgeTermination::Exited(known));
            assert_eq!(termination.exit_code(), Some(code));
            assert_eq!(CliError::ForgeTerminated { termination }.exit_code(), code);
            assert!(known.is_known());
        }
    }

    #[test]
    fn unknown_numeric_and_signal_outcomes_stay_typed() {
        let unknown = ForgeTermination::from_code(Some(63));
        assert_eq!(
            unknown,
            ForgeTermination::Exited(ForgeExitCode::Unknown(63))
        );
        assert_eq!(unknown.exit_code(), Some(63));
        assert_eq!(
            CliError::ForgeTerminated {
                termination: unknown,
            }
            .exit_code(),
            63
        );

        let signaled = ForgeTermination::from_code(None);
        assert_eq!(signaled, ForgeTermination::Signaled);
        assert_eq!(signaled.exit_code(), None);
        assert_eq!(
            CliError::ForgeTerminated {
                termination: signaled,
            }
            .exit_code(),
            1
        );

        let zero = ForgeTermination::from_code(Some(0));
        assert_eq!(zero, ForgeTermination::Exited(ForgeExitCode::Unknown(0)));
        assert_eq!(
            CliError::ForgeTerminated { termination: zero }.exit_code(),
            1
        );
    }

    #[test]
    fn readiness_timeout_uses_the_frozen_server_readiness_exit_code() {
        assert_eq!(CliError::ForgeReadinessTimeout.exit_code(), 71);
        assert_eq!(CliError::UnsupportedLifecycleControl.exit_code(), 1);
    }
}
