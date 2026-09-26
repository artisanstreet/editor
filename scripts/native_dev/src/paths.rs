//! Owned paths of the per-user development installation.
//!
//! Local builds install into their own side-by-side installation root,
//! `Artisan Street Dev`, next to (never inside) the real installation, so a
//! dev build can never touch the daily driver's binaries or data. The root
//! is an ordinary installation managed by `artisan-install` and doubles as
//! the dev Forge home; the runner keeps only its lock and startup receipts
//! in a private subdirectory.

use std::path::{Component, Path, PathBuf, Prefix};

use artisan_editor_cli::manifest::InstallationManifest;

use crate::error::DevError;

/// Directory name of the development installation root.
pub const DEV_ROOT_NAME: &str = "Artisan Street Dev";

/// Environment variable overriding the development installation root.
pub const DEV_ROOT_ENV: &str = "ARTISAN_DEV_ROOT";

/// Environment variable pointing the Editor at its installation root.
///
/// This is the same variable the shipping CLI discovery honors; the dev root
/// is explicit, never a platform default, so the real installation is
/// unreachable from a dev Editor.
pub const DEV_HOME_ENV: &str = "ARTISAN_HOME";

/// Legacy dev escape hatch that must not leak into the dev Editor.
///
/// When set, the Editor would attach to a manually started Forge instead of
/// starting its owned Forge. The runner strips it so every run exercises the
/// owned custody path.
pub const STRIPPED_DEV_HOME_ENV: &str = "ARTISAN_DEV_FORGE_HOME";

/// Companion override stripped alongside [`STRIPPED_DEV_HOME_ENV`].
pub const STRIPPED_DEV_READY_ENV: &str = "ARTISAN_DEV_FORGE_READY_FILE";

/// Asks the dev Editor to start and own the dev installation's Forge when it
/// has no registered host. The shipping Editor has no built-in host and
/// never starts a Forge itself; only this runner opts into one.
///
/// Must match `forge_dev_endpoint::OWNED_DEV_FORGE_ENV` in
/// `modules/frontend`; the contract tests pin the literal on both sides.
pub const OWNED_DEV_FORGE_ENV: &str = "ARTISAN_DEV_OWNED_FORGE";

/// Runner-private directory inside the root.
const RUNNER_DIRECTORY: &str = ".dev-runner";

/// Every path the runner owns in one development installation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DevPaths {
    /// Installation root and Forge home the dev Editor runs on.
    pub home: PathBuf,
    /// Installation pointer (`<home>/installation.json`).
    pub manifest_path: PathBuf,
    /// Permanent launcher (`<home>/bin/ae[.exe]`).
    pub permanent_ae: PathBuf,
}

impl DevPaths {
    /// Derives every owned path from one absolute, local installation root.
    ///
    /// # Errors
    ///
    /// Returns [`DevError::NotAbsolute`] when `home` is not absolute and
    /// [`DevError::NetworkShare`] when it lies on a network share.
    pub fn new(home: &Path) -> Result<Self, DevError> {
        if !home.is_absolute() {
            return Err(DevError::NotAbsolute {
                path: home.to_path_buf(),
            });
        }
        if is_network_share(home) {
            return Err(DevError::NetworkShare {
                path: home.to_path_buf(),
            });
        }
        Ok(Self {
            home: home.to_path_buf(),
            manifest_path: home.join("installation.json"),
            permanent_ae: home.join("bin").join(exe_name("ae")),
        })
    }

    /// Runner-private directory holding the lock and startup receipts.
    #[must_use]
    pub fn runner_dir(&self) -> PathBuf {
        self.home.join(RUNNER_DIRECTORY)
    }

    /// Inter-run lock serializing installs and launches on this root.
    #[must_use]
    pub fn lock_path(&self) -> PathBuf {
        self.runner_dir().join("runner.lock")
    }

    /// Version root the installation pointer names as active.
    ///
    /// # Errors
    ///
    /// Returns [`DevError::Stage`] when the root has no valid installation.
    pub fn active_version_root(&self) -> Result<PathBuf, DevError> {
        InstallationManifest::load(&self.manifest_path)
            .map(|manifest| manifest.version_root())
            .map_err(|error| DevError::Stage {
                stage: "resolve",
                reason: format!("no active dev installation: {error}"),
            })
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

/// Per-user default development root: the platform's user data directory
/// (`%LOCALAPPDATA%`, `~/Library/Application Support`, or
/// `$XDG_DATA_HOME`/`~/.local/share`) joined with [`DEV_ROOT_NAME`].
///
/// # Errors
///
/// Returns [`DevError::Stage`] when the user data directory is unavailable.
pub fn default_dev_root() -> Result<PathBuf, DevError> {
    let variable = |name: &str| {
        std::env::var_os(name)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    };
    let base = if cfg!(windows) {
        variable("LOCALAPPDATA")
    } else if cfg!(target_os = "macos") {
        variable("HOME").map(|home| home.join("Library").join("Application Support"))
    } else {
        variable("XDG_DATA_HOME")
            .or_else(|| variable("HOME").map(|home| home.join(".local").join("share")))
    };
    base.map(|base| base.join(DEV_ROOT_NAME))
        .ok_or_else(|| DevError::Stage {
            stage: "resolve",
            reason: "the user data directory is unavailable; pass --root".to_owned(),
        })
}

/// Resolves the development root: `--root`, then [`DEV_ROOT_ENV`], then the
/// per-user default.
///
/// # Errors
///
/// Returns [`DevError::NotAbsolute`], [`DevError::NetworkShare`], or
/// [`DevError::Stage`] when no usable root can be resolved.
pub fn resolve_dev_root(explicit: Option<&Path>) -> Result<PathBuf, DevError> {
    let root = match explicit {
        Some(path) => path.to_path_buf(),
        None => match std::env::var_os(DEV_ROOT_ENV) {
            Some(path) => PathBuf::from(path),
            None => default_dev_root()?,
        },
    };
    DevPaths::new(&root).map(|paths| paths.home)
}

/// Whether `path` lives on a Windows network share (`\\server\share\…`,
/// including `\\wsl.localhost\…`). Byte-range locks and hardlinks are
/// unreliable there, so the dev root must stay on local disk. Paths never
/// carry a UNC prefix on other platforms.
#[must_use]
pub fn is_network_share(path: &Path) -> bool {
    matches!(
        path.components().next(),
        Some(Component::Prefix(prefix))
            if matches!(prefix.kind(), Prefix::UNC(..) | Prefix::VerbatimUNC(..))
    )
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
