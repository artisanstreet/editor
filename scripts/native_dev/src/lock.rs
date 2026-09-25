//! Inter-run lock for one development root.
//!
//! Serializes installs and launches of concurrent `dev` runs on the same
//! root. The OS lock (`fs2`) releases automatically when the holder dies,
//! so a crashed run never wedges later runs. It is held only while a run
//! installs and prepares a launch, never for the Editor's lifetime, so a
//! new run can always relaunch over a running dev Editor.

use std::fs;

use fs2::FileExt;

use crate::{error::DevError, paths::DevPaths};

/// Held runner lock; released on drop.
pub struct DevLock {
    _file: fs::File,
}

impl DevLock {
    /// Acquires the runner lock of one development root.
    ///
    /// # Errors
    ///
    /// Returns [`DevError::StagingLocked`] when another run holds it, or
    /// [`DevError::Stage`] when the lock file cannot be created.
    pub fn acquire(paths: &DevPaths) -> Result<Self, DevError> {
        let lock = paths.lock_path();
        if let Some(parent) = lock.parent() {
            fs::create_dir_all(parent).map_err(|_| DevError::Stage {
                stage: "lock",
                reason: format!("cannot create {}", parent.display()),
            })?;
        }
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock)
            .map_err(|_| DevError::Stage {
                stage: "lock",
                reason: format!("cannot open {}", lock.display()),
            })?;
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Self { _file: file }),
            Err(_) => Err(DevError::StagingLocked { lock }),
        }
    }
}
