//! Hermes executable resolution mirroring TypeScript precedence exactly.
//!
//! TypeScript evidence (`modules/engines/src/hermes/service.ts`, `HermesExecutable`):
//! explicit `HERMES_EXECUTABLE` override (trimmed, empty falls through), then
//! `%LOCALAPPDATA%/hermes/hermes-agent/bin/hermes.exe` when it exists
//! (Windows only), then `hermes` on `PATH`.

use std::path::{Path, PathBuf};

/// Environment variable overriding the Hermes executable, checked first.
pub const HERMES_EXECUTABLE_ENV: &str = "HERMES_EXECUTABLE";

/// Executable file name used for `PATH` lookup on Windows.
pub const HERMES_PATH_BINARY_WINDOWS: &str = "hermes.exe";

/// Executable file name used for `PATH` lookup on other platforms.
pub const HERMES_PATH_BINARY: &str = "hermes";

/// Join sequence under `%LOCALAPPDATA%` holding the installed Windows executable.
pub const INSTALLED_WINDOWS_PARTS: [&str; 4] = ["hermes", "hermes-agent", "bin", "hermes.exe"];

/// Where a resolved Hermes executable came from, in TypeScript precedence order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HermesExecutableSource {
    /// Explicit `HERMES_EXECUTABLE` override.
    ExplicitOverride,
    /// Installed `%LOCALAPPDATA%/hermes/hermes-agent/bin/hermes.exe` (Windows).
    InstalledLocalAppData,
    /// `hermes` found on `PATH`.
    PathLookup,
}

impl HermesExecutableSource {
    /// Returns the stable spelling for this resolution source.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ExplicitOverride => "explicit-override",
            Self::InstalledLocalAppData => "installed-local-appdata",
            Self::PathLookup => "path-lookup",
        }
    }
}

/// A Hermes executable resolved through TypeScript precedence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedHermesExecutable {
    path: PathBuf,
    source: HermesExecutableSource,
}

impl ResolvedHermesExecutable {
    /// Returns the resolved executable path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns where the executable was resolved from.
    #[must_use]
    pub const fn source(&self) -> HermesExecutableSource {
        self.source
    }
}

/// Pure precedence mirror of TypeScript `HermesExecutable`.
///
/// A trimmed empty override falls through exactly like the TypeScript falsy
/// check after `.trim()`. Paths containing spaces are preserved verbatim; no
/// existence check happens here so tests can use fixture inputs.
#[must_use]
pub fn resolve_hermes_executable_from_parts(
    explicit_override: Option<&str>,
    installed: Option<PathBuf>,
    on_path: Option<PathBuf>,
) -> Option<ResolvedHermesExecutable> {
    if let Some(configured) = explicit_override {
        let trimmed = configured.trim();
        if !trimmed.is_empty() {
            return Some(ResolvedHermesExecutable {
                path: PathBuf::from(trimmed),
                source: HermesExecutableSource::ExplicitOverride,
            });
        }
    }
    if let Some(path) = installed {
        return Some(ResolvedHermesExecutable {
            path,
            source: HermesExecutableSource::InstalledLocalAppData,
        });
    }
    on_path.map(|path| ResolvedHermesExecutable {
        path,
        source: HermesExecutableSource::PathLookup,
    })
}

/// Joins the installed Windows executable path under the given local-app-data
/// directory: `hermes/hermes-agent/bin/hermes.exe`.
#[must_use]
pub fn installed_hermes_path(local_app_data: &Path) -> PathBuf {
    let mut path = local_app_data.to_path_buf();
    for part in INSTALLED_WINDOWS_PARTS {
        path.push(part);
    }
    path
}

fn find_hermes_on_path() -> Option<PathBuf> {
    let name = if cfg!(windows) {
        HERMES_PATH_BINARY_WINDOWS
    } else {
        HERMES_PATH_BINARY
    };
    let search = std::env::var_os("PATH")?;
    std::env::split_paths(&search)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

/// Live resolution against the process environment and filesystem.
///
/// Reads `HERMES_EXECUTABLE`, checks the installed Windows path only on
/// Windows (matching the TypeScript `process.platform === "win32"` gate), and
/// falls back to `PATH` lookup. A non-Unicode override is treated as absent.
pub fn resolve_hermes_executable() -> Option<ResolvedHermesExecutable> {
    let explicit =
        std::env::var_os(HERMES_EXECUTABLE_ENV).and_then(|value| value.into_string().ok());
    #[cfg(windows)]
    let installed = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|base| installed_hermes_path(&base))
        .filter(|candidate| candidate.is_file());
    #[cfg(not(windows))]
    let installed: Option<PathBuf> = None;
    resolve_hermes_executable_from_parts(explicit.as_deref(), installed, find_hermes_on_path())
}
