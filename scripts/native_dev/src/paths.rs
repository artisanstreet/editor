//! Owned paths for one isolated development installation.
//!
//! The central isolation invariant is that every path derives from the dev
//! directory alone. No platform default and no real installation path may
//! appear in it.

use std::path::{Path, PathBuf};

use crate::error::DevError;

/// Active version selected for every development installation.
pub const DEV_VERSION: &str = "dev";

/// Home directory name inside the dev directory.
pub const DEV_HOME_NAME: &str = "home";

/// Environment variable pointing the staged Editor at the dev home.
///
/// This is the same variable the shipping CLI discovery honors; the dev
/// home is explicit, never a platform default, so the real installation is
/// unreachable from a staged Editor.
pub const DEV_HOME_ENV: &str = "ARTISAN_HOME";

/// Legacy dev escape hatch that must not leak into the staged Editor.
///
/// When set, the Editor would attach to a manually started Forge instead of
/// starting its owned Forge. The dev launcher strips it so every run
/// exercises the owned custody path.
pub const STRIPPED_DEV_HOME_ENV: &str = "ARTISAN_DEV_FORGE_HOME";

/// Companion override stripped alongside [`STRIPPED_DEV_HOME_ENV`].
pub const STRIPPED_DEV_READY_ENV: &str = "ARTISAN_DEV_FORGE_READY_FILE";

/// Bazel workspace directory marker honored for the default dev directory.
pub const WORKSPACE_ENV: &str = "BUILD_WORKSPACE_DIRECTORY";

/// Dev-directory leaf holding the isolated installation.
pub const DIST_DEV_LEAF: &str = ".dist/dev";

/// Every path owned by one development installation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DevPaths {
    /// Isolated installation root (`<dev-dir>` itself is not the home).
    pub dev_dir: PathBuf,
    /// Forge home the staged Editor discovers through `ARTISAN_HOME`.
    pub home: PathBuf,
    /// Staged version root (`<home>/versions/dev`).
    pub version_root: PathBuf,
    /// Staged binary directory (`<version-root>/bin`).
    pub version_bin: PathBuf,
    /// Installation manifest (`<home>/installation.json`).
    pub manifest_path: PathBuf,
    /// Permanent launcher (`<home>/bin/ae[.exe]`).
    pub permanent_ae: PathBuf,
}

impl DevPaths {
    /// Derives every owned path from one absolute dev directory.
    ///
    /// # Errors
    ///
    /// Returns [`DevError::NotAbsolute`] when `dev_dir` is not absolute.
    pub fn new(dev_dir: &Path) -> Result<Self, DevError> {
        if !dev_dir.is_absolute() {
            return Err(DevError::NotAbsolute {
                path: dev_dir.to_path_buf(),
            });
        }
        let home = dev_dir.join(DEV_HOME_NAME);
        let version_root = home.join("versions").join(DEV_VERSION);
        Ok(Self {
            dev_dir: dev_dir.to_path_buf(),
            home: home.clone(),
            version_root: version_root.clone(),
            version_bin: version_root.join("bin"),
            manifest_path: home.join("installation.json"),
            permanent_ae: home.join("bin").join(exe_name("ae")),
        })
    }

    /// Scratch staging version root activated only after verification.
    ///
    /// A failed update never touches the active version: staging is
    /// verified first and swapped in atomically, so the previous binaries
    /// keep serving a running dev session until activation.
    #[must_use]
    pub fn staging_root(&self) -> PathBuf {
        self.home.join("versions").join("dev.staging")
    }

    /// Backup of the previously active version, removed after activation.
    #[must_use]
    pub fn previous_root(&self) -> PathBuf {
        self.home.join("versions").join("dev.previous")
    }

    /// Inter-run staging lock.
    #[must_use]
    pub fn lock_path(&self) -> PathBuf {
        self.dev_dir.join(".lock")
    }

    /// Startup receipt the staged Editor writes on launch.
    #[must_use]
    pub fn receipt_path(&self) -> PathBuf {
        self.dev_dir.join("startup-receipt.json")
    }

    /// Native instance database path owned by this home.
    #[must_use]
    pub fn database_path(&self) -> PathBuf {
        self.home.join("data").join("forge.sqlite3")
    }

    /// Native process-custody lock path owned by this home.
    #[must_use]
    pub fn custody_path(&self) -> PathBuf {
        self.home.join("custody").join("forge.lock")
    }

    /// Forge readiness receipt path owned by this home.
    #[must_use]
    pub fn readiness_path(&self) -> PathBuf {
        self.home.join("readiness").join("forge.json")
    }
}

/// Resolves the dev directory: explicit flag, then `<workspace>/.dist/dev`
/// from [`WORKSPACE_ENV`], then `<current-dir>/.dist/dev`.
#[must_use]
pub fn default_base_dir(workspace: Option<&Path>, current_dir: &Path) -> PathBuf {
    workspace.map_or_else(
        || current_dir.join(DIST_DEV_LEAF),
        |root| root.join(DIST_DEV_LEAF),
    )
}

/// Resolves and validates the effective dev directory.
///
/// # Errors
///
/// Returns [`DevError::NotAbsolute`] when the resolved directory is not
/// absolute.
pub fn resolve_dev_dir(explicit: Option<&Path>) -> Result<PathBuf, DevError> {
    let dev_dir = match explicit {
        Some(path) => path.to_path_buf(),
        None => {
            let workspace = std::env::var_os(WORKSPACE_ENV).map(PathBuf::from);
            let current = std::env::current_dir().map_err(|_| DevError::Stage {
                stage: "resolve",
                reason: "working directory is unavailable".to_owned(),
            })?;
            default_base_dir(workspace.as_deref(), &current)
        }
    };
    if !dev_dir.is_absolute() {
        return Err(DevError::NotAbsolute { path: dev_dir });
    }
    Ok(dev_dir)
}

/// Platform binary file name.
#[must_use]
pub fn exe_name(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.exe")
    } else {
        stem.to_owned()
    }
}
