//! `OpenCode2` view of the managed engine authority.
//!
//! `OpenCode2` is one catalog entry like every other engine; this façade only
//! fixes the engine so the profile registry and profile launch keep their
//! engine-specific API.

use std::path::{Path, PathBuf};

use super::{
    authority::{EngineInspection, ManagedEngineAuthority, ResolvedGeneration},
    catalog::ManagedEngine,
    spec::{
        ManagedInstallLock, ManagedInstallLockError, ManagedInstallPathError, ManagedInstallPaths,
    },
    state::ManagedEngineError,
};

/// The managed engine authority fixed to `OpenCode2`.
#[must_use = "use the authority for managed OpenCode2 operations"]
#[derive(Clone, Copy, Debug)]
pub struct NativeOpenCode2Authority {
    managed: ManagedEngineAuthority,
}

impl NativeOpenCode2Authority {
    /// Constructs the `OpenCode2` authority for this host.
    // No `Default`: callers opt into the managed authority explicitly.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self {
            managed: ManagedEngineAuthority::new(ManagedEngine::OpenCode2),
        }
    }

    /// Returns the engine-agnostic authority.
    pub const fn managed(&self) -> ManagedEngineAuthority {
        self.managed
    }

    /// Derives the installation paths for an absolute database path.
    ///
    /// # Errors
    ///
    /// Returns [`ManagedInstallPathError`] when the database path is unsafe.
    pub fn install_paths(
        &self,
        database_path: &Path,
    ) -> Result<ManagedInstallPaths, ManagedInstallPathError> {
        self.managed.install_paths(database_path)
    }

    /// Acquires and fences the exclusive installation lock.
    ///
    /// # Errors
    ///
    /// Returns [`ManagedInstallLockError`] when the lock cannot be acquired.
    pub fn acquire_install_lock(
        &self,
        database_path: &Path,
    ) -> Result<ManagedInstallLock, ManagedInstallLockError> {
        self.managed.acquire_install_lock(database_path)
    }

    /// Inspects the managed installation.
    ///
    /// # Errors
    ///
    /// Returns [`ManagedEngineError`] when the state or executable is invalid.
    pub fn inspect(&self, database_path: &Path) -> Result<EngineInspection, ManagedEngineError> {
        self.managed.inspect(database_path)
    }

    /// Resolves and verifies the active generation.
    ///
    /// # Errors
    ///
    /// Returns [`ManagedEngineError`] when the generation cannot be verified.
    pub fn resolve_active(
        &self,
        database_path: &Path,
    ) -> Result<ResolvedGeneration, ManagedEngineError> {
        self.managed.resolve_active(database_path)
    }

    /// Returns the engine root for an absolute database path.
    ///
    /// # Errors
    ///
    /// Returns [`ManagedEngineError`] when the database path is unsafe.
    pub fn managed_engine_root(&self, database_path: &Path) -> Result<PathBuf, ManagedEngineError> {
        self.managed.managed_engine_root(database_path)
    }

    /// Returns whether `OpenCode2` can be managed on this host.
    #[must_use]
    pub const fn platform_supported(&self) -> bool {
        self.managed.plan().is_ok()
    }
}
