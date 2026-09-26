//! Certified Codex CLI launch authority.
//!
//! Resolves the Forge-managed `codex` generation (see
//! [`crate::engine_core`]), enforces the minimum CLI version (`0.142.5`,
//! mirroring `CodexTransportMetadata.minimum_cli_version` in
//! `modules/engines/src/codex/protocol.ts`) at probe time, and produces the
//! non-cloneable [`VerifiedCodexLaunch`] capability carrying the exact
//! executable, its explicit environment, the probed version, and the engine
//! use lease. The capability is intentionally neither `Clone` nor
//! serializable.
//!
//! Version parsing is owned once by the sibling [`crate::codex`] module.

use std::{
    ffi::OsString,
    fmt,
    path::{Component, Path, PathBuf},
};

use artisan_domain::EngineProfileId;

use crate::engine_core::{ManagedEngine, ManagedEngineError, SeatedLaunch, resolve_launch_target};

/// Minimum supported Codex CLI version (`codex --version`).
pub const CODEX_MINIMUM_CLI_VERSION: &str = "0.142.5";

/// Fixed app-server argv for every Codex turn.
pub const CODEX_APP_SERVER_ARGS: &[&str] = &["app-server", "--stdio"];

/// App-server transportlabel shared with the TypeScript adapter.
pub const CODEX_TRANSPORT: &str = "stdio-jsonl";

/// App-server protocol version shared with the TypeScript adapter.
pub const CODEX_PROTOCOL_VERSION: &str = "v1";

/// Notification methods Artisan opts out of at `initialize` time.
///
/// Mirrors `codex_opt_out_notification_methods` in
/// `modules/engines/src/codex/protocol.ts`. Opted-out bookkeeping frames
/// must never occupy the run observation stream.
pub const CODEX_OPT_OUT_NOTIFICATION_METHODS: &[&str] = &[
    "account/rateLimits/updated",
    "mcpServer/startupStatus/updated",
    "remoteControl/status/changed",
];

/// Payload-free failure while resolving or probing a Codex launch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeCodexLaunchError {
    UnsupportedPlatform,
    ExecutableUnavailable,
    ExecutableUnsafe,
    DatabasePathUnsafe,
    VersionUnparseable,
    VersionTooOld,
}

impl NativeCodexLaunchError {
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

impl fmt::Display for NativeCodexLaunchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::UnsupportedPlatform => "Codex launch is unsupported on this platform",
            Self::ExecutableUnavailable => "Codex executable is unavailable",
            Self::ExecutableUnsafe => "Codex executable path is unsafe",
            Self::DatabasePathUnsafe => "Codex database path is unsafe",
            Self::VersionUnparseable => "Codex version output is unparseable",
            Self::VersionTooOld => "Codex version is older than the minimum supported",
        })
    }
}

impl std::error::Error for NativeCodexLaunchError {}

/// A verified Codex launch capability for one exact profile.
///
/// Carries the managed executable, its complete child environment, the
/// probed CLI version, and the engine use lease that defers generation
/// switches while the run is live. It is intentionally neither `Clone` nor
/// serializable; retain it until the protected run completes.
#[must_use = "retain the capability until the protected launch is complete"]
pub struct VerifiedCodexLaunch {
    profile_id: EngineProfileId,
    seat: SeatedLaunch,
    version: String,
}

impl fmt::Debug for VerifiedCodexLaunch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VerifiedCodexLaunch")
            .finish_non_exhaustive()
    }
}

impl fmt::Display for VerifiedCodexLaunch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("verified Codex launch capability")
    }
}

impl VerifiedCodexLaunch {
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

    /// Returns the probed CLI version (`X.Y.Z`).
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Returns the complete child environment (`CODEX_HOME` and `HOME` in
    /// the Forge-owned engine home). Spawn with `env_clear` first.
    #[must_use]
    pub fn environment(&self) -> &[(OsString, OsString)] {
        self.seat.environment()
    }

    /// Rechecks that the verified executable is still the same regular file.
    ///
    /// # Errors
    ///
    /// Returns [`NativeCodexLaunchError`] when the executable is no longer a
    /// verifiable regular file.
    pub fn revalidate(&self) -> Result<(), NativeCodexLaunchError> {
        verify_regular_executable(self.seat.executable())
    }

    /// Test-only construction over an explicitly supplied fixture program.
    ///
    /// # Errors
    ///
    /// Returns [`NativeCodexLaunchError`] when the fixture program is not a
    /// verifiable regular file or carries a version below the minimum.
    #[cfg(test)]
    pub fn for_tests(
        program: PathBuf,
        profile_id: EngineProfileId,
        version_stdout: &str,
    ) -> Result<Self, NativeCodexLaunchError> {
        verify_regular_executable(&program)?;
        let version = parse_codex_version(version_stdout)?;
        check_minimum_version(&version)?;
        Ok(Self {
            profile_id,
            seat: SeatedLaunch::fixture(program),
            version: version.to_string(),
        })
    }
}

/// Shared authority for Codex executable resolution and version probing.
#[must_use = "use the authority for certified Codex operations"]
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeCodexAuthority;

impl NativeCodexAuthority {
    /// Constructs the Codex launch authority.
    pub const fn new() -> Self {
        Self
    }

    /// Resolves the Forge-managed Codex executable without probing its
    /// version: the verified active generation, or the absolute developer
    /// override. `PATH`, `LOCALAPPDATA`, and `WinGet` are never consulted.
    ///
    /// # Errors
    ///
    /// Returns [`NativeCodexLaunchError`] when Codex is not installed, not
    /// supported here, or fails verification.
    pub fn resolve_executable(&self) -> Result<PathBuf, NativeCodexLaunchError> {
        resolve_launch_target(ManagedEngine::Codex)
            .map(|target| target.executable().to_path_buf())
            .map_err(map_managed_error)
    }

    /// Resolves one profile into a verified launch capability.
    ///
    /// The database path is validated as absolute without `..` segments; the
    /// managed executable is resolved, verified, and seated with its explicit
    /// environment and use lease; `version_stdout` is the exact
    /// `codex --version` output captured at probe time and is parsed plus
    /// enforced against [`CODEX_MINIMUM_CLI_VERSION`].
    ///
    /// # Errors
    ///
    /// Returns [`NativeCodexLaunchError`] when the database path, executable,
    /// or probed version cannot be certified.
    pub fn resolve_launch(
        &self,
        database_path: &Path,
        profile_id: &EngineProfileId,
        version_stdout: &str,
    ) -> Result<VerifiedCodexLaunch, NativeCodexLaunchError> {
        verify_database_path(database_path)?;
        let version = parse_codex_version(version_stdout)?;
        check_minimum_version(&version)?;
        let seat =
            SeatedLaunch::seat(ManagedEngine::Codex, database_path).map_err(map_managed_error)?;
        verify_regular_executable(seat.executable())?;
        Ok(VerifiedCodexLaunch {
            profile_id: profile_id.clone(),
            seat,
            version: version.to_string(),
        })
    }
}

#[cfg(feature = "fixtures")]
impl NativeCodexAuthority {
    /// Fixture seam for owner tests: certifies an explicit program with the
    /// managed environment of `database_path`. Compiled only with the
    /// `fixtures` feature, never into production builds.
    ///
    /// # Errors
    ///
    /// Returns [`NativeCodexLaunchError`] when the path or version cannot be
    /// certified.
    pub fn resolve_launch_with_executable(
        &self,
        database_path: &Path,
        profile_id: &EngineProfileId,
        executable: &Path,
        version: &str,
    ) -> Result<VerifiedCodexLaunch, NativeCodexLaunchError> {
        verify_database_path(database_path)?;
        verify_regular_executable(executable)?;
        let parsed = parse_codex_version(version)?;
        check_minimum_version(&parsed)?;
        let seat = SeatedLaunch::explicit(ManagedEngine::Codex, database_path, executable)
            .map_err(map_managed_error)?;
        Ok(VerifiedCodexLaunch {
            profile_id: profile_id.clone(),
            seat,
            version: parsed.to_string(),
        })
    }
}

fn map_managed_error(error: ManagedEngineError) -> NativeCodexLaunchError {
    match error {
        ManagedEngineError::UnsupportedPlatform => NativeCodexLaunchError::UnsupportedPlatform,
        ManagedEngineError::StateMissing | ManagedEngineError::ExecutableUnavailable => {
            NativeCodexLaunchError::ExecutableUnavailable
        }
        _ => NativeCodexLaunchError::ExecutableUnsafe,
    }
}

/// Parsed `X.Y.Z` Codex CLI version.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct CodexVersion {
    major: u64,
    minor: u64,
    patch: u64,
}

impl fmt::Display for CodexVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Parses the first `X.Y.Z` triple in `codex --version` output.
///
/// Delegates to the shared [`crate::codex::version`] parser (word-boundary
/// semantic version, `v`-prefix tolerated, trailing pre-release/build
/// metadata ignored) and reports it as the authority's numeric triple.
///
/// # Errors
///
/// Returns [`NativeCodexLaunchError::VersionUnparseable`] when no triple is
/// present or a component overflows `u64`.
fn parse_codex_version(stdout: &str) -> Result<CodexVersion, NativeCodexLaunchError> {
    let version = crate::codex::version::parse_codex_version(stdout.as_bytes())
        .ok_or(NativeCodexLaunchError::VersionUnparseable)?;
    let mut components = version.split('.');
    match (
        components.next(),
        components.next(),
        components.next(),
        components.next(),
    ) {
        (Some(major), Some(minor), Some(patch), None) => Ok(CodexVersion {
            major: major
                .parse()
                .map_err(|_| NativeCodexLaunchError::VersionUnparseable)?,
            minor: minor
                .parse()
                .map_err(|_| NativeCodexLaunchError::VersionUnparseable)?,
            patch: patch
                .parse()
                .map_err(|_| NativeCodexLaunchError::VersionUnparseable)?,
        }),
        _ => Err(NativeCodexLaunchError::VersionUnparseable),
    }
}

/// Enforces the minimum CLI version at probe time.
///
/// Delegates to the shared [`crate::codex::version`] gate against
/// [`CODEX_MINIMUM_CLI_VERSION`].
///
/// # Errors
///
/// Returns [`NativeCodexLaunchError::VersionTooOld`] when the probed version
/// predates [`CODEX_MINIMUM_CLI_VERSION`].
fn check_minimum_version(version: &CodexVersion) -> Result<(), NativeCodexLaunchError> {
    if crate::codex::version::meets_minimum_version(&version.to_string()) {
        Ok(())
    } else {
        Err(NativeCodexLaunchError::VersionTooOld)
    }
}

fn verify_database_path(path: &Path) -> Result<(), NativeCodexLaunchError> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(NativeCodexLaunchError::DatabasePathUnsafe);
    }
    if path.parent().is_none() {
        return Err(NativeCodexLaunchError::DatabasePathUnsafe);
    }
    Ok(())
}

fn verify_regular_executable(path: &Path) -> Result<(), NativeCodexLaunchError> {
    if path.as_os_str().is_empty() {
        return Err(NativeCodexLaunchError::ExecutableUnsafe);
    }
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(NativeCodexLaunchError::ExecutableUnsafe);
    }
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(NativeCodexLaunchError::ExecutableUnsafe);
            }
            Ok(())
        }
        Err(_) => Err(NativeCodexLaunchError::ExecutableUnavailable),
    }
}

/// Compares two `X.Y.Z` version spellings numerically.
///
/// Both spellings are normalized through the shared
/// [`crate::codex::version`] parser before comparison, so `v`-prefixed and
/// suffixed spellings compare by their numeric core. Unparseable inputs
/// compare as equal so callers must parse first for fallible decisions.
#[must_use]
pub fn compare_codex_versions(left: &str, right: &str) -> i64 {
    use std::cmp::Ordering;
    let (Some(left), Some(right)) = (
        crate::codex::version::parse_codex_version(left.as_bytes()),
        crate::codex::version::parse_codex_version(right.as_bytes()),
    ) else {
        return 0;
    };
    match crate::codex::version::compare_semantic_versions(&left, &right) {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimum_version_constant_matches_adapter() {
        assert_eq!(CODEX_MINIMUM_CLI_VERSION, "0.142.5");
        assert_eq!(
            CODEX_OPT_OUT_NOTIFICATION_METHODS,
            &[
                "account/rateLimits/updated",
                "mcpServer/startupStatus/updated",
                "remoteControl/status/changed",
            ]
        );
    }

    #[test]
    fn version_parsing_accepts_embedded_triples() {
        assert_eq!(
            parse_codex_version("codex-cli 0.142.5\n"),
            Ok(CodexVersion {
                major: 0,
                minor: 142,
                patch: 5,
            })
        );
        assert_eq!(
            parse_codex_version("0.145.0-alpha+001"),
            Ok(CodexVersion {
                major: 0,
                minor: 145,
                patch: 0,
            })
        );
        assert_eq!(
            parse_codex_version("no version here"),
            Err(NativeCodexLaunchError::VersionUnparseable)
        );
    }

    #[test]
    fn minimum_enforcement_rejects_older_releases() {
        let old = parse_codex_version("codex 0.141.9").unwrap();
        assert_eq!(
            check_minimum_version(&old),
            Err(NativeCodexLaunchError::VersionTooOld)
        );
        let current = parse_codex_version("codex 0.142.5").unwrap();
        assert_eq!(check_minimum_version(&current), Ok(()));
        let newer = parse_codex_version("codex 0.145.0").unwrap();
        assert_eq!(check_minimum_version(&newer), Ok(()));
    }

    #[test]
    fn unsafe_paths_reject_without_filesystem_effects() {
        assert_eq!(
            verify_database_path(Path::new("relative.sqlite")),
            Err(NativeCodexLaunchError::DatabasePathUnsafe)
        );
        assert_eq!(
            verify_database_path(Path::new("/data/../evil.sqlite")),
            Err(NativeCodexLaunchError::DatabasePathUnsafe)
        );
        assert_eq!(
            verify_regular_executable(Path::new("/definitely/missing/codex-xyz")),
            Err(NativeCodexLaunchError::ExecutableUnavailable)
        );
    }
}
