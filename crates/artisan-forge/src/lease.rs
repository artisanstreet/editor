//! Exclusive database ownership for one Forge instance.
//!
//! Mirrors `modules/forge/src/database-lease.ts`: ownership is an
//! exclusive-create lock file next to the database holding the owner as JSON.
//! A live owner blocks startup; a dead one may be reclaimed once liveness
//! checks exist (that lands with the audited Windows platform module). Until
//! then this crate fails closed: any existing lease is a conflict naming its
//! owner, never silently stolen.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The lock file lives beside the database it guards.
#[must_use]
pub fn lease_lock_path(database_path: &str) -> PathBuf {
    PathBuf::from(format!("{database_path}.artisan-forge.lock"))
}

/// Identifies the process that owns the database.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseOwner {
    instance_id: String,
    pid: u32,
}

impl LeaseOwner {
    /// Creates the owner record this process will write.
    #[must_use]
    pub fn new(instance_id: impl Into<String>, pid: u32) -> Self {
        Self {
            instance_id: instance_id.into(),
            pid,
        }
    }

    /// Instance identifier of the recorded owner.
    #[must_use]
    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    /// Process id of the recorded owner.
    #[must_use]
    pub fn pid(&self) -> u32 {
        self.pid
    }
}

/// A held lease; releasing removes the lock file.
#[derive(Debug)]
pub struct ForgeDatabaseLease {
    lock_path: PathBuf,
    released: bool,
}

impl ForgeDatabaseLease {
    /// Path of the underlying lock file.
    #[must_use]
    pub fn lock_path(&self) -> &Path {
        &self.lock_path
    }

    /// Removes the lock file. Deleting a missing file is tolerated so shutdown
    /// stays idempotent under concurrent cleanup.
    ///
    /// # Errors
    /// Returns the I/O error when deletion fails for reasons other than a
    /// missing file.
    pub fn release(&mut self) -> std::io::Result<()> {
        if self.released {
            return Ok(());
        }
        self.released = true;
        match std::fs::remove_file(&self.lock_path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
}

impl Drop for ForgeDatabaseLease {
    fn drop(&mut self) {
        let _unused = self.release();
    }
}

/// Why a lease could not be acquired.
#[derive(Debug, thiserror::Error)]
pub enum LeaseError {
    /// Another (possibly crashed-but-alive) Forge owns this database.
    #[error("database at {database_path} is already owned by instance {owner_instance_id} (pid {owner_pid}); lock: {}", lock_path.display())]
    AlreadyOwned {
        /// Database path from the configuration.
        database_path: String,
        /// Owner recorded in the conflicting lease.
        owner_instance_id: String,
        /// Owner process id recorded in the conflicting lease.
        owner_pid: u32,
        /// Lock file path.
        lock_path: PathBuf,
    },

    /// The lock could not be created or written.
    #[error("failed to acquire forge lease at {}: {source}", lock_path.display())]
    Io {
        /// Lock file path.
        lock_path: PathBuf,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },

    /// An existing lock file exists but does not parse as an owner record.
    #[error("corrupt forge lease at {}: {source}", lock_path.display())]
    Corrupt {
        /// Lock file path.
        lock_path: PathBuf,
        /// Underlying error.
        #[source]
        source: serde_json::Error,
    },
}

fn read_owner(lock_path: &Path) -> Result<Option<LeaseOwner>, LeaseError> {
    let bytes = match std::fs::read(lock_path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(LeaseError::Io {
                lock_path: lock_path.to_path_buf(),
                source: error,
            });
        }
    };
    serde_json::from_slice::<LeaseOwner>(&bytes)
        .map(Some)
        .map_err(|source| LeaseError::Corrupt {
            lock_path: lock_path.to_path_buf(),
            source,
        })
}

/// Acquires exclusive ownership of `database_path`, failing closed when any
/// lease file exists (reclaim-by-liveness arrives with the platform module).
///
/// # Errors
/// [`LeaseError::AlreadyOwned`] when a lease exists (its recorded owner is
/// reported), [`LeaseError::Corrupt`] for unparseable leases, and
/// [`LeaseError::Io`] when creation or writing fails.
pub fn acquire_lease(
    database_path: &str,
    owner: &LeaseOwner,
) -> Result<ForgeDatabaseLease, LeaseError> {
    let lock_path = lease_lock_path(database_path);
    if let Some(existing) = read_owner(&lock_path)? {
        return Err(LeaseError::AlreadyOwned {
            database_path: database_path.to_string(),
            owner_instance_id: existing.instance_id().to_string(),
            owner_pid: existing.pid(),
            lock_path,
        });
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
        .map_err(|error| LeaseError::Io {
            lock_path: lock_path.clone(),
            source: error,
        })?;
    let encoded = serde_json::to_vec(owner).map_err(|error| LeaseError::Io {
        lock_path: lock_path.clone(),
        source: std::io::Error::other(error),
    })?;
    file.write_all(&encoded).map_err(|error| LeaseError::Io {
        lock_path: lock_path.clone(),
        source: error,
    })?;
    Ok(ForgeDatabaseLease {
        lock_path,
        released: false,
    })
}

#[cfg(test)]
mod tests {
    use super::{LeaseError, LeaseOwner, acquire_lease, lease_lock_path};

    struct TempDir(std::path::PathBuf);
    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("artisan-forge-test-{name}-{}", std::process::id()));
            let _unused = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("temp dir creates");
            Self(dir)
        }
        fn path(&self, name: &str) -> String {
            self.0.join(name).to_string_lossy().into_owned()
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _unused = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn acquire_release_reacquire_round_trips() {
        let temp = TempDir::new("roundtrip");
        let db = temp.path("artisan.db");
        let mut lease = acquire_lease(&db, &LeaseOwner::new("forge-a", 42)).expect("first acquire");
        assert!(lease.lock_path().exists());
        assert_eq!(lease.lock_path(), lease_lock_path(&db));
        lease.release().expect("release");
        assert!(!lease.lock_path().exists());
        let _second = acquire_lease(&db, &LeaseOwner::new("forge-b", 43)).expect("reacquire");
    }

    #[test]
    fn double_release_is_idempotent() {
        let temp = TempDir::new("double");
        let db = temp.path("artisan.db");
        let mut lease = acquire_lease(&db, &LeaseOwner::new("forge-a", 1)).expect("acquire");
        lease.release().expect("first release");
        lease.release().expect("second release");
    }

    #[test]
    fn conflict_reports_the_recorded_owner() {
        let temp = TempDir::new("conflict");
        let db = temp.path("artisan.db");
        let _holder = acquire_lease(&db, &LeaseOwner::new("forge-live", 4242)).expect("hold");
        let error = acquire_lease(&db, &LeaseOwner::new("forge-next", 7))
            .expect_err("conflicting acquire must fail");
        match error {
            LeaseError::AlreadyOwned {
                owner_instance_id,
                owner_pid,
                ..
            } => {
                assert_eq!(owner_instance_id, "forge-live");
                assert_eq!(owner_pid, 4242);
            }
            other => panic!("expected AlreadyOwned, got {other}"),
        }
    }

    #[test]
    fn dropping_the_lease_releases_it() {
        let temp = TempDir::new("drop");
        let db = temp.path("artisan.db");
        {
            let _lease = acquire_lease(&db, &LeaseOwner::new("forge-drop", 9)).expect("acquire");
        }
        assert!(!lease_lock_path(&db).exists());
    }
}
