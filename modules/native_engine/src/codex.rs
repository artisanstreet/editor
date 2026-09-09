//! Certified Codex CLI launch authority.
//!
//! Resolves the installed `codex` binary, enforces the minimum CLI version
//! (`0.142.5`, mirroring `CodexTransportMetadata.minimum_cli_version` in
//! `modules/engines/src/codex/protocol.ts`) at probe time, and produces the
//! non-cloneable [`VerifiedCodexLaunch`] capability carrying the exact
//! executable plus the probed version. This mirrors the `opencode2`
//! `VerifiedOpenCode2ProfileLaunch` shape but is concrete to Codex: no
//! certified generation, no install lock, no profile registry. The capability
//! is intentionally neither `Clone` nor serializable.

use std::{
    fmt,
    path::{Component, Path, PathBuf},
};

use artisan_domain::EngineProfileId;

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
/// Carries the resolved executable plus the probed CLI version. It is
/// intentionally neither `Clone` nor serializable; retain it until the
/// protected spawn completes.
#[must_use = "retain the capability until the protected launch is complete"]
pub struct VerifiedCodexLaunch {
    database_path: PathBuf,
    profile_id: EngineProfileId,
    executable: PathBuf,
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
        &self.executable
    }

    /// Returns the probed CLI version (`X.Y.Z`).
    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Returns the private Codex home derived from the database parent.
    ///
    /// Codex itself owns `CODEX_HOME`; Artisan seats the managed launch at
    /// `<database-parent>/toolchain/codex/home` so the ambient user home is
    /// never mutated by a managed turn.
    #[must_use]
    pub fn codex_home(&self) -> PathBuf {
        codex_home_for_database(&self.database_path)
    }

    /// Rechecks that the verified executable is still the same regular file.
    ///
    /// # Errors
    ///
    /// Returns [`NativeCodexLaunchError`] when the executable is no longer a
    /// verifiable regular file.
    pub fn revalidate(&self) -> Result<(), NativeCodexLaunchError> {
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
    ) -> Result<Self, NativeCodexLaunchError> {
        verify_regular_executable(&program)?;
        let version = parse_codex_version(version_stdout)?;
        check_minimum_version(&version)?;
        Ok(Self {
            database_path: PathBuf::from("/test/codex.sqlite"),
            profile_id,
            executable: program,
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
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Resolves the installed Codex executable without probing its version.
    ///
    /// Honors `ARTISAN_CODEX_EXECUTABLE` when it names an existing regular
    /// file, otherwise searches `PATH` for `codex` (`codex.exe` on Windows).
    ///
    /// # Errors
    ///
    /// Returns [`NativeCodexLaunchError`] when no verifiable executable is
    /// available.
    pub fn resolve_executable(&self) -> Result<PathBuf, NativeCodexLaunchError> {
        if let Ok(configured) = std::env::var("ARTISAN_CODEX_EXECUTABLE") {
            let trimmed = configured.trim();
            if !trimmed.is_empty() {
                let path = PathBuf::from(trimmed);
                verify_regular_executable(&path)?;
                return Ok(path);
            }
        }
        let file_name = if cfg!(windows) { "codex.exe" } else { "codex" };
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
        Err(NativeCodexLaunchError::ExecutableUnavailable)
    }

    /// Resolves one profile into a verified launch capability.
    ///
    /// The database path is validated as absolute without `..` segments; the
    /// executable is resolved and verified as a regular file; `version_stdout`
    /// is the exact `codex --version` output captured at probe time and is
    /// parsed plus enforced against [`CODEX_MINIMUM_CLI_VERSION`]. No process
    /// is spawned here: the caller performs the bounded `--version` probe and
    /// hands its bytes in, so this stays a pure verification boundary.
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
        let executable = self.resolve_executable()?;
        let version = parse_codex_version(version_stdout)?;
        check_minimum_version(&version)?;
        Ok(VerifiedCodexLaunch {
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
        Ok(VerifiedCodexLaunch {
            database_path: database_path.to_path_buf(),
            profile_id: profile_id.clone(),
            executable: executable.to_path_buf(),
            version: parsed.to_string(),
        })
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
/// Accepts trailing pre-release/build metadata on the triple (for example
/// `0.142.5-alpha`) but records only the numeric core, matching the
/// TypeScript `ParseCodexVersion` behaviour.
///
/// # Errors
///
/// Returns [`NativeCodexLaunchError::VersionUnparseable`] when no triple is
/// present or a component overflows `u64`.
fn parse_codex_version(stdout: &str) -> Result<CodexVersion, NativeCodexLaunchError> {
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
    Err(NativeCodexLaunchError::VersionUnparseable)
}

fn parse_triple_at(bytes: &[u8]) -> Option<CodexVersion> {
    let (major, rest) = parse_component(bytes)?;
    let rest = rest.strip_prefix(b".")?;
    let (minor, rest) = parse_component(rest)?;
    let rest = rest.strip_prefix(b".")?;
    let (patch, _) = parse_component(rest)?;
    Some(CodexVersion {
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
/// Returns [`NativeCodexLaunchError::VersionTooOld`] when the probed version
/// predates [`CODEX_MINIMUM_CLI_VERSION`].
fn check_minimum_version(version: &CodexVersion) -> Result<(), NativeCodexLaunchError> {
    let minimum = parse_codex_version(CODEX_MINIMUM_CLI_VERSION)
        .map_err(|_| NativeCodexLaunchError::VersionTooOld)?;
    if *version < minimum {
        return Err(NativeCodexLaunchError::VersionTooOld);
    }
    Ok(())
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

fn codex_home_for_database(database_path: &Path) -> PathBuf {
    match database_path.parent() {
        Some(parent) => parent.join("toolchain").join("codex").join("home"),
        None => PathBuf::from("toolchain/codex/home"),
    }
}

/// Compares two `X.Y.Z` version spellings numerically.
///
/// Returns a negative value when `left` predates `right`, zero when equal,
/// and a positive value when `left` is newer. Unparseable inputs compare as
/// equal so callers must parse first for fallible decisions.
#[must_use]
pub fn compare_codex_versions(left: &str, right: &str) -> i64 {
    match (parse_codex_version(left), parse_codex_version(right)) {
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
