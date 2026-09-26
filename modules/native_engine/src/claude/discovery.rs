//! Claude Code executable selection for the readiness probe.
//!
//! An explicit caller-supplied command wins; otherwise the executable is the
//! Forge-managed Claude generation or the absolute `ARTISAN_CLAUDE_EXECUTABLE`
//! developer override (see `crate::engine_core`). `PATH` is never searched.
//!
//! Discovery never splits a value on spaces: an override naming a path such
//! as `C:\Program Files\Claude\claude.exe` is preserved as one command.
//! Discovery also never proves the binary runs; the bounded probe spawn in
//! `super::probe` is the authority for executability.

use std::ffi::{OsStr, OsString};
use std::fmt;

use crate::engine_core::{LaunchSource, ManagedEngine, resolve_launch_target};

/// The Claude Code developer override variable (an absolute path).
pub const CLAUDE_EXECUTABLE_ENV_VAR: &str = ManagedEngine::Claude.override_env();

/// Maximum accepted override value in bytes, sized for long Windows paths.
pub const MAX_EXECUTABLE_VALUE_BYTES: usize = 1024;

/// Maximum number of extra executable arguments accepted by the probe runner.
pub const MAX_EXECUTABLE_ARGS: usize = 16;

/// Maximum length of one extra executable argument in bytes.
pub const MAX_EXECUTABLE_ARG_BYTES: usize = 1024;

/// Where a discovered Claude executable came from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClaudeExecutableSource {
    /// Caller-supplied managed override.
    ExplicitOverride,
    /// The absolute `ARTISAN_CLAUDE_EXECUTABLE` developer override.
    Environment,
    /// The Forge-managed Claude generation.
    Managed,
}

/// A resolved Claude Code executable command.
///
/// The command is one opaque filesystem path. It is never shell-parsed, so
/// paths containing spaces survive verbatim.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClaudeExecutable {
    command: OsString,
    source: ClaudeExecutableSource,
}

impl ClaudeExecutable {
    /// Returns the exact command to hand to the process spawner.
    #[must_use]
    pub fn command(&self) -> &OsStr {
        &self.command
    }

    /// Returns where this executable was resolved from.
    #[must_use]
    pub const fn source(&self) -> ClaudeExecutableSource {
        self.source
    }

    /// Consumes the executable and returns the owned spawn command.
    #[must_use]
    pub fn into_command(self) -> OsString {
        self.command
    }
}

/// Bounded, path-free failures from executable override selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClaudeDiscoveryError {
    /// An explicit or environment override exceeds its byte bound.
    ValueTooLong,
    /// No managed Claude generation or developer override resolves.
    NotInstalled,
}

impl ClaudeDiscoveryError {
    /// Returns the stable classification for this failure.
    #[must_use]
    pub const fn cli_reason(self) -> &'static str {
        match self {
            Self::ValueTooLong => "executable_override_too_long",
            Self::NotInstalled => "not_installed",
        }
    }
}

impl fmt::Display for ClaudeDiscoveryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ValueTooLong => "Claude executable override exceeds its bound",
            Self::NotInstalled => "Claude is not installed on this Forge",
        })
    }
}

impl std::error::Error for ClaudeDiscoveryError {}

/// Selects the winning override value without touching the real environment.
///
/// The explicit value wins when it is non-empty after trimming; otherwise the
/// environment value applies. Empty and whitespace-only values fall through to
/// `None` rather than failing, so an unset override behaves like no override.
///
/// # Errors
///
/// Returns [`ClaudeDiscoveryError::ValueTooLong`] when the winning value
/// exceeds [`MAX_EXECUTABLE_VALUE_BYTES`] bytes.
pub fn select_override_value(
    explicit: Option<&str>,
    environment: Option<&str>,
) -> Result<Option<String>, ClaudeDiscoveryError> {
    let selected = explicit
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| environment.map(str::trim).filter(|value| !value.is_empty()));
    match selected {
        None => Ok(None),
        Some(value) if value.len() > MAX_EXECUTABLE_VALUE_BYTES => {
            Err(ClaudeDiscoveryError::ValueTooLong)
        }
        Some(value) => Ok(Some(value.to_owned())),
    }
}

/// Discovers the Claude Code executable for the readiness probe.
///
/// The explicit value is a caller-supplied command (tests and fixtures) and
/// wins; otherwise the Forge-managed Claude generation, or the absolute
/// `ARTISAN_CLAUDE_EXECUTABLE` developer override, is used. `PATH` and bare
/// command fallbacks are never consulted.
///
/// # Errors
///
/// Returns [`ClaudeDiscoveryError::ValueTooLong`] for an overlong explicit
/// value and [`ClaudeDiscoveryError::NotInstalled`] when no managed Claude
/// generation or override resolves.
pub fn discover_claude_executable(
    explicit: Option<&str>,
) -> Result<ClaudeExecutable, ClaudeDiscoveryError> {
    if let Some(command) = select_override_value(explicit, None)? {
        return Ok(ClaudeExecutable {
            command: OsString::from(command),
            source: ClaudeExecutableSource::ExplicitOverride,
        });
    }
    let target = resolve_launch_target(ManagedEngine::Claude)
        .map_err(|_| ClaudeDiscoveryError::NotInstalled)?;
    Ok(ClaudeExecutable {
        command: target.executable().as_os_str().to_owned(),
        source: match target.source() {
            LaunchSource::Managed => ClaudeExecutableSource::Managed,
            LaunchSource::Override => ClaudeExecutableSource::Environment,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_selection_prefers_explicit_and_ignores_blanks() {
        assert_eq!(
            select_override_value(Some("  managed  "), Some("env")).unwrap(),
            Some("managed".to_owned())
        );
        assert_eq!(
            select_override_value(Some("   "), Some("env")).unwrap(),
            Some("env".to_owned())
        );
        assert_eq!(select_override_value(None, None).unwrap(), None);
        assert_eq!(select_override_value(Some(""), Some("  ")).unwrap(), None);
    }

    #[test]
    fn override_selection_rejects_overlong_values() {
        let long = "x".repeat(MAX_EXECUTABLE_VALUE_BYTES + 1);
        assert_eq!(
            select_override_value(Some(&long), None),
            Err(ClaudeDiscoveryError::ValueTooLong)
        );
        assert_eq!(
            ClaudeDiscoveryError::ValueTooLong.cli_reason(),
            "executable_override_too_long"
        );
    }

    #[test]
    fn discovery_keeps_spaced_override_as_one_command() {
        let spaced = if cfg!(windows) {
            "C:\\Program Files\\Claude\\claude.exe"
        } else {
            "/opt/claude code/claude"
        };
        let discovered = discover_claude_executable(Some(spaced)).unwrap();
        assert_eq!(
            discovered.source(),
            ClaudeExecutableSource::ExplicitOverride
        );
        assert_eq!(discovered.command(), OsStr::new(spaced));
    }
}
