//! Certified Claude Code CLI launch authority.
//!
//! Resolves the installed `claude` binary, enforces the minimum CLI version
//! (`2.1.220`, the release whose native resume behavior Artisan verified,
//! mirroring `claude_native_continuation_version` in
//! `modules/engines/src/claude/probe.ts`) at probe time, and produces the
//! non-cloneable [`VerifiedClaudeLaunch`] capability carrying the exact
//! executable plus the probed version. This mirrors the `codex`
//! `VerifiedCodexLaunch` shape but is concrete to Claude: no certified
//! generation, no install lock, no profile registry. The capability is
//! intentionally neither `Clone` nor serializable.
//!
//! This is deliberately NOT the sibling discovery/posture module: no
//! executable-source taxonomy, no bare-command fallback, no auth-state
//! classification lives here. Discovery answers "what could run"; this
//! authority answers "what is certified to spawn now", so an unverifiable
//! file never becomes a launch capability.

use std::{
    fmt,
    path::{Component, Path, PathBuf},
};

use artisan_domain::EngineProfileId;

/// Minimum supported Claude Code CLI version (`claude --version`).
///
/// This is the verified native-continuation release. The runtime records the
/// same value separately as data for the later continuation packet; this
/// constant is the spawn-time enforcement.
pub const CLAUDE_MINIMUM_CLI_VERSION: &str = "2.1.220";

/// Claude Code release whose native resume behavior Artisan has verified.
///
/// Mirrors `claude_native_continuation_version` in
/// `modules/engines/src/claude/probe.ts`. Recorded here as data so readiness
/// evidence can be gated on it without re-reading TypeScript; no continuation
/// gate is enforced in this packet.
pub const CLAUDE_NATIVE_CONTINUATION_VERSION: &str = "2.1.220";

/// Stream-JSON transport label shared with the TypeScript adapter.
pub const CLAUDE_TRANSPORT: &str = "claude-cli-stream-json";

/// Stream-JSON protocol version shared with the TypeScript adapter.
pub const CLAUDE_PROTOCOL_VERSION: &str = "claude-stream-json-v1";

/// Environment variable naming a Claude Code executable override.
///
/// Native-port convention mirroring `ARTISAN_CODEX_EXECUTABLE`. An explicitly
/// configured value takes precedence over `PATH` lookup.
pub const CLAUDE_EXECUTABLE_ENV_VAR: &str = "ARTISAN_CLAUDE_EXECUTABLE";

/// Payload-free failure while resolving or probing a Claude launch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeClaudeLaunchError {
    UnsupportedPlatform,
    ExecutableUnavailable,
    ExecutableUnsafe,
    DatabasePathUnsafe,
    VersionUnparseable,
    VersionTooOld,
}

impl NativeClaudeLaunchError {
    /// Returns the stable classification for this failure.
    #[must_use]
    pub const fn cli_reason(self) -> &'static str {
        match self {
            Self::UnsupportedPlatform => "unsupported_platform",
            Self::ExecutableUnavailable => "executable_unavailable",
            Self::ExecutableUnsafe => "executable_unsafe",
            Self::DatabasePathUnsafe => "database_path_unsafe",
            Self::VersionUnparseable => "version_unparseable",
            Self::VersionTooOld => "version_too_old",
        }
    }
}

impl fmt::Display for NativeClaudeLaunchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::UnsupportedPlatform => "Claude launch is unsupported on this platform",
            Self::ExecutableUnavailable => "Claude executable is unavailable",
            Self::ExecutableUnsafe => "Claude executable path is unsafe",
            Self::DatabasePathUnsafe => "Claude database path is unsafe",
            Self::VersionUnparseable => "Claude version output is unparseable",
            Self::VersionTooOld => "Claude version is older than the minimum supported",
        })
    }
}

impl std::error::Error for NativeClaudeLaunchError {}

/// A verified Claude launch capability for one exact profile.
///
/// Carries the resolved executable plus the probed CLI version. It is
/// intentionally neither `Clone` nor serializable; retain it until the
/// protected spawn completes.
#[must_use = "retain the capability until the protected launch is complete"]
pub struct VerifiedClaudeLaunch {
    database_path: PathBuf,
    profile_id: EngineProfileId,
    executable: PathBuf,
    version: String,
}

impl fmt::Debug for VerifiedClaudeLaunch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VerifiedClaudeLaunch")
            .finish_non_exhaustive()
    }
}

impl fmt::Display for VerifiedClaudeLaunch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("verified Claude launch capability")
    }
}

impl VerifiedClaudeLaunch {
    /// Returns the exact profile identity selected for this launch.
    #[must_use]
    pub const fn profile_id(&self) -> &EngineProfileId {
        &self.profile_id
    }

    /// Returns the exact verified executable path.
    #[must_use]
    pub fn executable_path(&self) -> &Path {
        &self.executable
    }

    /// Returns the probed CLI version (`X.Y.Z` with optional suffix).
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Rechecks that the verified executable is still the same regular file.
    ///
    /// # Errors
    ///
    /// Returns [`NativeClaudeLaunchError`] when the executable is no longer a
    /// verifiable regular file.
    pub fn revalidate(&self) -> Result<(), NativeClaudeLaunchError> {
        verify_regular_executable(&self.executable)
    }

    /// Test-only construction over an explicitly supplied fixture program.
    ///
    /// The fixture must already be a regular file; the version string is
    /// still parsed and still enforced against the minimum so fixture
    /// launches cannot smuggle an unsupported version into the capability.
    #[cfg(test)]
    pub fn for_tests(
        program: PathBuf,
        profile_id: EngineProfileId,
        version_stdout: &str,
    ) -> Result<Self, NativeClaudeLaunchError> {
        verify_regular_executable(&program)?;
        let version = parse_claude_version(version_stdout)?;
        check_minimum_version(&version)?;
        Ok(Self {
            database_path: PathBuf::from("/test/claude.sqlite"),
            profile_id,
            executable: program,
            version: version.to_string(),
        })
    }
}

/// Shared authority for Claude executable resolution and version probing.
#[must_use = "use the authority for certified Claude operations"]
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeClaudeAuthority;

impl NativeClaudeAuthority {
    /// Constructs the Claude launch authority.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Resolves the installed Claude executable without probing its version.
    ///
    /// Honors `ARTISAN_CLAUDE_EXECUTABLE` when it names an existing regular
    /// file, otherwise searches `PATH` for `claude` (`claude.exe` on
    /// Windows).
    ///
    /// # Errors
    ///
    /// Returns [`NativeClaudeLaunchError`] when no verifiable executable is
    /// available.
    pub fn resolve_executable(&self) -> Result<PathBuf, NativeClaudeLaunchError> {
        if let Ok(configured) = std::env::var(CLAUDE_EXECUTABLE_ENV_VAR) {
            let trimmed = configured.trim();
            if !trimmed.is_empty() {
                let path = PathBuf::from(trimmed);
                verify_regular_executable(&path)?;
                return Ok(path);
            }
        }
        let file_name = if cfg!(windows) {
            "claude.exe"
        } else {
            "claude"
        };
        if let Some(paths) = std::env::var_os("PATH") {
            for entry in std::env::split_paths(&paths) {
                if entry.as_os_str().is_empty() {
                    continue;
                }
                let candidate = entry.join(file_name);
                if verify_regular_executable(&candidate).is_ok() {
                    return Ok(candidate);
                }
            }
        }
        Err(NativeClaudeLaunchError::ExecutableUnavailable)
    }

    /// Resolves one profile into a verified launch capability.
    ///
    /// The database path is validated as absolute without `..` segments; the
    /// executable is resolved and verified as a regular file; `version_stdout`
    /// is the exact `claude --version` output captured at probe time and is
    /// parsed plus enforced against [`CLAUDE_MINIMUM_CLI_VERSION`]. No process
    /// is spawned here: the caller performs the bounded `--version` probe and
    /// hands its bytes in, so this stays a pure verification boundary.
    ///
    /// # Errors
    ///
    /// Returns [`NativeClaudeLaunchError`] when the database path, executable,
    /// or probed version cannot be certified.
    pub fn resolve_launch(
        &self,
        database_path: &Path,
        profile_id: &EngineProfileId,
        version_stdout: &str,
    ) -> Result<VerifiedClaudeLaunch, NativeClaudeLaunchError> {
        verify_database_path(database_path)?;
        let executable = self.resolve_executable()?;
        let version = parse_claude_version(version_stdout)?;
        check_minimum_version(&version)?;
        Ok(VerifiedClaudeLaunch {
            database_path: database_path.to_path_buf(),
            profile_id: profile_id.clone(),
            executable,
            version: version.to_string(),
        })
    }

    /// Resolves one profile against an explicitly supplied executable.
    ///
    /// Used by the owner at spawn time when the capability was already
    /// probed: re-verifies the executable file and re-enforces the carried
    /// version without PATH discovery.
    ///
    /// # Errors
    ///
    /// Returns [`NativeClaudeLaunchError`] when the path or version cannot be
    /// certified.
    pub fn resolve_launch_with_executable(
        &self,
        database_path: &Path,
        profile_id: &EngineProfileId,
        executable: &Path,
        version: &str,
    ) -> Result<VerifiedClaudeLaunch, NativeClaudeLaunchError> {
        verify_database_path(database_path)?;
        verify_regular_executable(executable)?;
        let parsed = parse_claude_version(version)?;
        check_minimum_version(&parsed)?;
        Ok(VerifiedClaudeLaunch {
            database_path: database_path.to_path_buf(),
            profile_id: profile_id.clone(),
            executable: executable.to_path_buf(),
            version: parsed.to_string(),
        })
    }
}

/// Parsed `X.Y.Z` Claude CLI version with optional pre-release/build suffix.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct ClaudeVersion {
    major: u64,
    minor: u64,
    patch: u64,
}

impl fmt::Display for ClaudeVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Parses the first `X.Y.Z` triple in `claude --version` output.
///
/// Accepts trailing pre-release/build metadata on the triple (for example
/// `2.1.220-alpha`) but records only the numeric core, matching the
/// TypeScript adapter's version match behaviour.
///
/// # Errors
///
/// Returns [`NativeClaudeLaunchError::VersionUnparseable`] when no triple is
/// present or a component overflows `u64`.
fn parse_claude_version(stdout: &str) -> Result<ClaudeVersion, NativeClaudeLaunchError> {
    let bytes = stdout.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index].is_ascii_digit() {
            if let Some(version) = parse_triple_at(&bytes[index..]) {
                return Ok(version);
            }
        }
        index += 1;
    }
    Err(NativeClaudeLaunchError::VersionUnparseable)
}

fn parse_triple_at(bytes: &[u8]) -> Option<ClaudeVersion> {
    let (major, rest) = parse_component(bytes)?;
    let rest = rest.strip_prefix(b".")?;
    let (minor, rest) = parse_component(rest)?;
    let rest = rest.strip_prefix(b".")?;
    let (patch, _) = parse_component(rest)?;
    Some(ClaudeVersion {
        major,
        minor,
        patch,
    })
}

fn parse_component(bytes: &[u8]) -> Option<(u64, &[u8])> {
    let mut end = 0;
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
    }
    if end == 0 || end > 19 {
        return None;
    }
    let value = std::str::from_utf8(&bytes[..end])
        .ok()?
        .parse::<u64>()
        .ok()?;
    Some((value, &bytes[end..]))
}

/// Enforces the minimum CLI version at probe time.
///
/// # Errors
///
/// Returns [`NativeClaudeLaunchError::VersionTooOld`] when the probed version
/// predates [`CLAUDE_MINIMUM_CLI_VERSION`].
fn check_minimum_version(version: &ClaudeVersion) -> Result<(), NativeClaudeLaunchError> {
    let minimum = parse_claude_version(CLAUDE_MINIMUM_CLI_VERSION)
        .map_err(|_| NativeClaudeLaunchError::VersionTooOld)?;
    if *version < minimum {
        return Err(NativeClaudeLaunchError::VersionTooOld);
    }
    Ok(())
}

fn verify_database_path(path: &Path) -> Result<(), NativeClaudeLaunchError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(NativeClaudeLaunchError::DatabasePathUnsafe);
    }
    if path.parent().is_none() {
        return Err(NativeClaudeLaunchError::DatabasePathUnsafe);
    }
    Ok(())
}

fn verify_regular_executable(path: &Path) -> Result<(), NativeClaudeLaunchError> {
    if path.as_os_str().is_empty() {
        return Err(NativeClaudeLaunchError::ExecutableUnsafe);
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(NativeClaudeLaunchError::ExecutableUnsafe);
    }
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(NativeClaudeLaunchError::ExecutableUnsafe);
            }
            Ok(())
        }
        Err(_) => Err(NativeClaudeLaunchError::ExecutableUnavailable),
    }
}

/// Compares two `X.Y.Z` version spellings numerically.
///
/// Returns a negative value when `left` predates `right`, zero when equal,
/// and a positive value when `left` is newer. Unparseable inputs compare as
/// equal so callers must parse first for fallible decisions.
#[must_use]
pub fn compare_claude_versions(left: &str, right: &str) -> i64 {
    match (parse_claude_version(left), parse_claude_version(right)) {
        (Ok(left), Ok(right)) => {
            if left == right {
                0
            } else if left < right {
                -1
            } else {
                1
            }
        }
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimum_version_constant_matches_verified_release() {
        assert_eq!(CLAUDE_MINIMUM_CLI_VERSION, "2.1.220");
        assert_eq!(CLAUDE_NATIVE_CONTINUATION_VERSION, "2.1.220");
        assert_eq!(CLAUDE_TRANSPORT, "claude-cli-stream-json");
        assert_eq!(CLAUDE_PROTOCOL_VERSION, "claude-stream-json-v1");
    }

    #[test]
    fn version_parsing_accepts_embedded_triples() {
        assert_eq!(
            parse_claude_version("2.1.220 (Claude Code)\n"),
            Ok(ClaudeVersion {
                major: 2,
                minor: 1,
                patch: 220,
            })
        );
        assert_eq!(
            parse_claude_version("claude 2.2.0-alpha+001"),
            Ok(ClaudeVersion {
                major: 2,
                minor: 2,
                patch: 0,
            })
        );
        assert_eq!(
            parse_claude_version("no version here"),
            Err(NativeClaudeLaunchError::VersionUnparseable)
        );
    }

    #[test]
    fn minimum_enforcement_rejects_older_releases() {
        let old = parse_claude_version("claude 2.1.219").unwrap();
        assert_eq!(
            check_minimum_version(&old),
            Err(NativeClaudeLaunchError::VersionTooOld)
        );
        let current = parse_claude_version("claude 2.1.220").unwrap();
        assert_eq!(check_minimum_version(&current), Ok(()));
        let newer = parse_claude_version("claude 2.2.0").unwrap();
        assert_eq!(check_minimum_version(&newer), Ok(()));
    }

    #[test]
    fn unsafe_paths_reject_without_filesystem_effects() {
        assert_eq!(
            verify_database_path(Path::new("relative.sqlite")),
            Err(NativeClaudeLaunchError::DatabasePathUnsafe)
        );
        assert_eq!(
            verify_database_path(Path::new("/data/../evil.sqlite")),
            Err(NativeClaudeLaunchError::DatabasePathUnsafe)
        );
        assert_eq!(
            verify_regular_executable(Path::new("/definitely/missing/claude-xyz")),
            Err(NativeClaudeLaunchError::ExecutableUnavailable)
        );
    }
}
