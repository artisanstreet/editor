//! Source-backed Claude Code executable discovery.
//!
//! Resolution order mirrors the TypeScript adapter's override semantics in
//! `modules/engines/src/process/spawn-override.ts`: an explicit managed
//! override wins, then a configured environment value, then `PATH` lookup,
//! then the bare default command. The environment variable name follows the
//! established `ARTISAN_CODEX_EXECUTABLE` convention from
//! `modules/engines/src/codex/executable.ts`.
//!
//! Discovery never splits a value on spaces: an override naming a path such
//! as `C:\Program Files\Claude\claude.exe` is preserved as one command.
//! Discovery also never proves the binary runs; the bounded probe spawn in
//! `super::probe` is the authority for executability.

use std::env;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::path::{Path, PathBuf};

/// Environment variable naming a Claude Code executable override.
///
/// Native-port convention mirroring `ARTISAN_CODEX_EXECUTABLE`. An explicit
/// managed override passed to [`discover_claude_executable`] takes precedence
/// over this value.
pub const CLAUDE_EXECUTABLE_ENV_VAR: &str = "ARTISAN_CLAUDE_EXECUTABLE";

/// Bare command used when no override or `PATH` entry resolves.
///
/// Returning the bare command keeps spawn-time `PATH` resolution as the
/// authority instead of freezing a miss into an error at discovery time.
pub const CLAUDE_DEFAULT_COMMAND: &str = "claude";

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
    /// The `ARTISAN_CLAUDE_EXECUTABLE` environment value.
    Environment,
    /// A `PATH` directory entry that exists on the filesystem.
    PathLookup,
    /// Fallback bare command; spawn-time `PATH` resolution decides.
    DefaultCommand,
}

/// A resolved Claude Code executable command.
///
/// The command is one opaque value: either a filesystem path or a bare name
/// resolved through `PATH` at spawn time. It is never shell-parsed, so paths
/// containing spaces survive verbatim.
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
}

impl ClaudeDiscoveryError {
    /// Returns the stable classification for this failure.
    #[must_use]
    pub const fn cli_reason(self) -> &'static str {
        match self {
            Self::ValueTooLong => "executable_override_too_long",
        }
    }
}

impl fmt::Display for ClaudeDiscoveryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ValueTooLong => "Claude executable override exceeds its bound",
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

/// Returns the executable file names probed inside each `PATH` directory.
///
/// Windows accepts the `claude.exe` console binary first and the bare `claude`
/// name second; other platforms probe only the bare name. Batch shims such as
/// `claude.cmd` are deliberately excluded because the native spawner does not
/// run through a shell.
#[must_use]
pub const fn candidate_file_names() -> &'static [&'static str] {
    #[cfg(windows)]
    {
        &["claude.exe", "claude"]
    }
    #[cfg(not(windows))]
    {
        &["claude"]
    }
}

/// Searches directories in order for the first existing candidate file.
///
/// Directory and file names are joined as paths, so entries containing spaces
/// are preserved verbatim. Existence is a plain regular-file check: this is
/// discovery of a user-installed CLI, not certification of a managed binary.
pub fn search_path_for(
    directories: impl IntoIterator<Item = PathBuf>,
    file_names: &[&str],
    exists: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    for directory in directories {
        if directory.as_os_str().is_empty() {
            continue;
        }
        for file_name in file_names {
            let candidate = directory.join(file_name);
            if exists(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

fn search_current_path() -> Option<PathBuf> {
    let path = env::var_os("PATH")?;
    search_path_for(
        env::split_paths(&path),
        candidate_file_names(),
        Path::is_file,
    )
}

/// Discovers the Claude Code executable from override, environment, or `PATH`.
///
/// The explicit value is the managed spawn override; it wins over the
/// `ARTISAN_CLAUDE_EXECUTABLE` environment value, which wins over `PATH`
/// lookup. When nothing resolves, the bare [`CLAUDE_DEFAULT_COMMAND`] is
/// returned so spawn-time `PATH` resolution remains the authority.
///
/// This performs no filesystem writes, no credential access, and no process
/// spawns. Values longer than [`MAX_EXECUTABLE_VALUE_BYTES`] are rejected.
///
/// # Errors
///
/// Returns [`ClaudeDiscoveryError`] when a configured override value exceeds
/// its bound.
pub fn discover_claude_executable(
    explicit: Option<&str>,
) -> Result<ClaudeExecutable, ClaudeDiscoveryError> {
    if let Some(command) = select_override_value(explicit, None)? {
        return Ok(ClaudeExecutable {
            command: OsString::from(command),
            source: ClaudeExecutableSource::ExplicitOverride,
        });
    }
    let environment =
        env::var_os(CLAUDE_EXECUTABLE_ENV_VAR).map(|value| value.to_string_lossy().into_owned());
    if let Some(command) = select_override_value(None, environment.as_deref())? {
        return Ok(ClaudeExecutable {
            command: OsString::from(command),
            source: ClaudeExecutableSource::Environment,
        });
    }
    if let Some(path) = search_current_path() {
        return Ok(ClaudeExecutable {
            command: path.into_os_string(),
            source: ClaudeExecutableSource::PathLookup,
        });
    }
    Ok(ClaudeExecutable {
        command: OsString::from(CLAUDE_DEFAULT_COMMAND),
        source: ClaudeExecutableSource::DefaultCommand,
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
    fn path_search_preserves_entries_with_spaces() {
        let root = PathBuf::from(if cfg!(windows) {
            "C:\\Program Files\\Claude Test"
        } else {
            "/opt/claude test"
        });
        let want = root.join("bin").join("claude.exe");
        let found = search_path_for([root.join("bin")], &["claude.exe"], |path| {
            path == want.as_path()
        });
        assert_eq!(found, Some(root.join("bin").join("claude.exe")));
        assert_eq!(
            search_path_for([PathBuf::new()], &["claude"], Path::is_file),
            None
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
