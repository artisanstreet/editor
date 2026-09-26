//! Certified Claude Code CLI launch authority.
//!
//! Resolves the Forge-managed `claude` generation (see
//! [`crate::engine_core`]), enforces the minimum CLI version (`2.1.220`, the
//! release whose native resume behavior Artisan verified, mirroring
//! `claude_native_continuation_version` in
//! `modules/engines/src/claude/probe.ts`) at probe time, and produces the
//! non-cloneable [`VerifiedClaudeLaunch`] capability carrying the exact
//! executable, its explicit environment, the probed version, and the engine
//! use lease. The capability is intentionally neither `Clone` nor
//! serializable.
//!
//! Version parsing is owned once by the sibling [`crate::claude`] module.

use std::{
    ffi::OsString,
    fmt,
    path::{Component, Path, PathBuf},
};

use artisan_domain::EngineProfileId;

use crate::engine_core::{ManagedEngine, ManagedEngineError, SeatedLaunch, resolve_launch_target};

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

/// First Claude Code release whose `--thinking-display summarized` launch
/// Artisan captured and verified (2026-09-25 capture).
///
/// This is a known-good floor, not the first release that accepted the
/// hidden flag: older CLIs keep their existing arguments. It is independent
/// of [`CLAUDE_MINIMUM_CLI_VERSION`] and never gates launch or continuation.
pub const CLAUDE_THINKING_DISPLAY_VERSION: &str = "2.1.282";

/// Verified `--thinking-display` support of one probed CLI.
///
/// Distinguishes support for the flag from eligibility for hosted
/// `highlights`: no variant implies highlights eligibility, which is an
/// observed property of an execution context rather than of a CLI version.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClaudeThinkingDisplaySupport {
    /// Below the verified display floor or unparseable: omit the flag.
    Unsupported,
    /// `--thinking-display summarized` is verified for this release.
    Summarized,
}

/// Resolves the verified thinking-display support for one CLI version.
///
/// Fails closed: an unparseable version is [`Unsupported`], so an unknown
/// CLI keeps its existing arguments.
///
/// [`Unsupported`]: ClaudeThinkingDisplaySupport::Unsupported
#[must_use]
pub fn claude_thinking_display_support(version: &str) -> ClaudeThinkingDisplaySupport {
    match (
        parse_claude_version(version),
        parse_claude_version(CLAUDE_THINKING_DISPLAY_VERSION),
    ) {
        (Ok(version), Ok(floor)) if version >= floor => ClaudeThinkingDisplaySupport::Summarized,
        _ => ClaudeThinkingDisplaySupport::Unsupported,
    }
}

/// Stream-JSON transport label shared with the TypeScript adapter.
pub const CLAUDE_TRANSPORT: &str = "claude-cli-stream-json";

/// Stream-JSON protocol version shared with the TypeScript adapter.
pub const CLAUDE_PROTOCOL_VERSION: &str = "claude-stream-json-v1";

/// Environment variable naming the Claude Code developer override: an
/// absolute executable path, reported as an override in engine status.
pub const CLAUDE_EXECUTABLE_ENV_VAR: &str = ManagedEngine::Claude.override_env();

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
/// Carries the managed executable, its complete child environment, the
/// probed CLI version, and the engine use lease that defers generation
/// switches while the run is live. It is intentionally neither `Clone` nor
/// serializable; retain it until the protected run completes.
#[must_use = "retain the capability until the protected launch is complete"]
pub struct VerifiedClaudeLaunch {
    profile_id: EngineProfileId,
    seat: SeatedLaunch,
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
        self.seat.executable()
    }

    /// Returns the probed CLI version (`X.Y.Z` with optional suffix).
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Returns the verified `--thinking-display` support of this CLI.
    #[must_use]
    pub fn thinking_display(&self) -> ClaudeThinkingDisplaySupport {
        claude_thinking_display_support(&self.version)
    }

    /// Returns the complete child environment (`CLAUDE_CONFIG_DIR` and
    /// `HOME` in the Forge-owned engine home). Spawn with `env_clear` first.
    #[must_use]
    pub fn environment(&self) -> &[(OsString, OsString)] {
        self.seat.environment()
    }

    /// Rechecks that the verified executable is still the same regular file.
    ///
    /// # Errors
    ///
    /// Returns [`NativeClaudeLaunchError`] when the executable is no longer a
    /// verifiable regular file.
    pub fn revalidate(&self) -> Result<(), NativeClaudeLaunchError> {
        verify_regular_executable(self.seat.executable())
    }

    /// Test-only construction over an explicitly supplied fixture program.
    ///
    /// # Errors
    ///
    /// Returns [`NativeClaudeLaunchError`] when the fixture program is not a
    /// verifiable regular file or carries a version below the minimum.
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
            profile_id,
            seat: SeatedLaunch::fixture(program),
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
    pub const fn new() -> Self {
        Self
    }

    /// Resolves the Forge-managed Claude executable without probing its
    /// version: the verified active generation, or the absolute developer
    /// override. `PATH` and npm shims are never consulted.
    ///
    /// # Errors
    ///
    /// Returns [`NativeClaudeLaunchError`] when Claude is not installed, not
    /// supported here, or fails verification.
    pub fn resolve_executable(&self) -> Result<PathBuf, NativeClaudeLaunchError> {
        resolve_launch_target(ManagedEngine::Claude)
            .map(|target| target.executable().to_path_buf())
            .map_err(map_managed_error)
    }

    /// Resolves one profile into a verified launch capability.
    ///
    /// The database path is validated as absolute without `..` segments; the
    /// managed executable is resolved, verified, and seated with its explicit
    /// environment and use lease; `version_stdout` is the exact
    /// `claude --version` output captured at probe time and is parsed plus
    /// enforced against [`CLAUDE_MINIMUM_CLI_VERSION`].
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
        let version = parse_claude_version(version_stdout)?;
        check_minimum_version(&version)?;
        let seat =
            SeatedLaunch::seat(ManagedEngine::Claude, database_path).map_err(map_managed_error)?;
        verify_regular_executable(seat.executable())?;
        Ok(VerifiedClaudeLaunch {
            profile_id: profile_id.clone(),
            seat,
            version: version.to_string(),
        })
    }
}

fn map_managed_error(error: ManagedEngineError) -> NativeClaudeLaunchError {
    match error {
        ManagedEngineError::UnsupportedPlatform => NativeClaudeLaunchError::UnsupportedPlatform,
        ManagedEngineError::StateMissing | ManagedEngineError::ExecutableUnavailable => {
            NativeClaudeLaunchError::ExecutableUnavailable
        }
        _ => NativeClaudeLaunchError::ExecutableUnsafe,
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
/// Delegates to the shared [`crate::claude::probe`] parser (word-boundary
/// semantic version, trailing pre-release/build metadata ignored) and
/// reports it as the authority's numeric triple.
///
/// # Errors
///
/// Returns [`NativeClaudeLaunchError::VersionUnparseable`] when no triple is
/// present or a component overflows `u64`.
fn parse_claude_version(stdout: &str) -> Result<ClaudeVersion, NativeClaudeLaunchError> {
    let version = crate::claude::probe::parse_claude_version(stdout)
        .ok_or(NativeClaudeLaunchError::VersionUnparseable)?;
    let core = version
        .split(['-', '+'])
        .next()
        .ok_or(NativeClaudeLaunchError::VersionUnparseable)?;
    let mut components = core.split('.');
    match (
        components.next(),
        components.next(),
        components.next(),
        components.next(),
    ) {
        (Some(major), Some(minor), Some(patch), None) => Ok(ClaudeVersion {
            major: major
                .parse()
                .map_err(|_| NativeClaudeLaunchError::VersionUnparseable)?,
            minor: minor
                .parse()
                .map_err(|_| NativeClaudeLaunchError::VersionUnparseable)?,
            patch: patch
                .parse()
                .map_err(|_| NativeClaudeLaunchError::VersionUnparseable)?,
        }),
        _ => Err(NativeClaudeLaunchError::VersionUnparseable),
    }
}

/// Enforces the minimum CLI version at probe time.
///
/// Delegates parsing to the shared [`crate::claude::probe`] parser and gates
/// the numeric triple against [`CLAUDE_MINIMUM_CLI_VERSION`].
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
/// Both spellings are normalized through the shared
/// [`crate::claude::probe`] parser before comparison. Unparseable inputs
/// compare as equal so callers must parse first for fallible decisions.
#[must_use]
pub fn compare_claude_versions(left: &str, right: &str) -> i64 {
    match (parse_claude_version(left), parse_claude_version(right)) {
        (Ok(left), Ok(right)) => match left.cmp(&right) {
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Greater => 1,
        },
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
    fn thinking_display_support_starts_at_the_captured_release() {
        assert_eq!(CLAUDE_THINKING_DISPLAY_VERSION, "2.1.282");
        for (version, support) in [
            (
                "2.1.282 (Claude Code)",
                ClaudeThinkingDisplaySupport::Summarized,
            ),
            ("2.2.0", ClaudeThinkingDisplaySupport::Summarized),
            ("2.1.281", ClaudeThinkingDisplaySupport::Unsupported),
            ("2.1.220", ClaudeThinkingDisplaySupport::Unsupported),
            ("no version", ClaudeThinkingDisplaySupport::Unsupported),
        ] {
            assert_eq!(
                claude_thinking_display_support(version),
                support,
                "{version}"
            );
        }
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
