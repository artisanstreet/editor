//! Reconciling a stale Forge readiness receipt before a start.
//!
//! The Forge publishes its receipt with a no-clobber install and removes it
//! only on graceful shutdown. A Forge that was killed leaves the receipt
//! behind, and the next Forge then refuses to publish and dies. Whoever
//! starts the Forge of a home (a service supervisor, a lifecycle command)
//! may clear the way, under tight rules:
//!
//! - A receipt identifying a **live** Forge running the expected binary is
//!   refused, never touched ([`CliError::ForgeAlreadyRunning`]).
//! - Otherwise the safe regular parent chain is required, the receipt must
//!   parse as valid Forge readiness (anything malformed, oversized, or
//!   non-regular is preserved and refused), and the home's Forge custody
//!   lock must be acquirable nonblocking — proving no live Forge owns this
//!   home even when pid queries are unavailable. The probe is retained while
//!   the receipt is rechecked against the exact bytes validated before, and
//!   only those bytes are removed.
//! - Publish temporaries are never swept: a stale temporary cannot block the
//!   next publish, and deleting files by pattern would violate the
//!   preservation contract.
//!
//! Custody proves no live Forge holds this home, and pid-executable identity
//! is rechecked on top; neither is cryptographic ownership. The dangerous
//! case — a live Forge — is refused twice: once by the pid-identity check
//! and once by custody contention, either of which preserves the receipt.

use std::{
    fs::{self, File, Metadata, OpenOptions},
    path::Path,
};

use crate::{CliError, Result};

use super::{super::spec::ForgeReadinessStatus, ForgeReadiness, readiness_status};

/// Outcome of pre-start readiness reconciliation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadinessReconcile {
    /// No receipt file exists.
    Absent,
    /// A stale receipt naming a dead Forge was removed.
    CleanedStale {
        /// Dead Forge identity the stale receipt named.
        pid: u32,
    },
}

/// Bound for one readiness receipt read, matching the receipt reader.
const READINESS_MAX_BYTES: u64 = 4_096;

/// Windows reparse-point attribute.
#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

/// Windows flag that opens a reparse point itself instead of following it.
#[cfg(windows)]
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

fn preserved(reason: String) -> CliError {
    CliError::StaleReadiness(reason)
}

/// Reconciles the readiness receipt at `readiness` of the home whose Forge
/// holds `custody`, for a start of `forge_exe`.
///
/// # Errors
///
/// Returns [`CliError::ForgeAlreadyRunning`] for a live Forge,
/// [`CliError::ForgeCustodyHeld`] when custody is occupied, and
/// [`CliError::StaleReadiness`] when an unsafe or unreadable receipt blocks
/// the start.
pub fn reconcile_stale_readiness(
    readiness: &Path,
    custody: &Path,
    forge_exe: &Path,
) -> Result<ReadinessReconcile> {
    match readiness_status(readiness, forge_exe) {
        ForgeReadinessStatus::Ready(live) => Err(CliError::ForgeAlreadyRunning { pid: live.pid() }),
        ForgeReadinessStatus::Missing => Ok(ReadinessReconcile::Absent),
        ForgeReadinessStatus::Invalid => reconcile_invalid_readiness(readiness, custody, forge_exe),
    }
}

/// Only a regular file that parses as valid Forge readiness, under a safe
/// parent chain, with acquirable home custody, rechecked byte-identical, is
/// stale and removable; everything else is preserved and refused.
fn reconcile_invalid_readiness(
    readiness: &Path,
    custody: &Path,
    forge_exe: &Path,
) -> Result<ReadinessReconcile> {
    let Some(readiness_dir) = readiness.parent() else {
        return Err(preserved(format!(
            "readiness path has no parent: {}",
            readiness.display()
        )));
    };
    validate_parent_chain(readiness_dir)?;
    let metadata = match fs::symlink_metadata(readiness) {
        Ok(metadata) => metadata,
        // Raced away between the status check and now: nothing to clean.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ReadinessReconcile::Absent);
        }
        Err(_) => return Err(preserved(format!("cannot inspect {}", readiness.display()))),
    };
    if !is_plain_file(&metadata) {
        return Err(preserved(format!(
            "stale readiness at {} is not a regular file; remove it by hand",
            readiness.display()
        )));
    }
    if metadata.len() > READINESS_MAX_BYTES {
        return Err(preserved(format!(
            "stale readiness at {} exceeds its size bound; remove it by hand",
            readiness.display()
        )));
    }
    let validated = fs::read(readiness)
        .map_err(|_| preserved(format!("cannot read {}", readiness.display())))?;
    let receipt = ForgeReadiness::from_json(&validated).map_err(|_| {
        preserved(format!(
            "stale readiness at {} is malformed; remove it by hand",
            readiness.display()
        ))
    })?;
    // No live Forge may own this home while the receipt is removed: the
    // custody probe proves it even when pid queries are unavailable, and is
    // retained through the recheck so no Forge can start in between.
    let _custody = acquire_custody_probe(custody)?;
    if let ForgeReadinessStatus::Ready(live) = readiness_status(readiness, forge_exe) {
        return Err(CliError::ForgeAlreadyRunning { pid: live.pid() });
    }
    let current = fs::read(readiness)
        .map_err(|_| preserved(format!("cannot re-read {}", readiness.display())))?;
    if current != validated {
        return Err(preserved(format!(
            "readiness at {} changed during reconciliation; retry",
            readiness.display()
        )));
    }
    fs::remove_file(readiness).map_err(|_| {
        preserved(format!(
            "cannot remove stale readiness at {}",
            readiness.display()
        ))
    })?;
    Ok(ReadinessReconcile::CleanedStale { pid: receipt.pid() })
}

/// Whether metadata describes a plain regular file: symlinks, reparse
/// points, directories, and anything else fail closed.
fn is_plain_file(metadata: &Metadata) -> bool {
    !metadata.file_type().is_symlink() && metadata.is_file() && !is_reparse_point(metadata)
}

/// Validates every ancestor of a removal target without resolving links:
/// removal must never operate through a redirected parent.
fn validate_parent_chain(path: &Path) -> Result<()> {
    for current in path.ancestors() {
        if current.as_os_str().is_empty() {
            break;
        }
        let metadata = match fs::symlink_metadata(current) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(preserved(format!(
                    "readiness parent is missing: {}",
                    current.display()
                )));
            }
            Err(_) => return Err(preserved(format!("cannot inspect {}", current.display()))),
        };
        if metadata.file_type().is_symlink() {
            return Err(preserved(format!(
                "readiness parent is a symbolic link; remove it by hand: {}",
                current.display()
            )));
        }
        if is_reparse_point(&metadata) {
            return Err(preserved(format!(
                "readiness parent is a reparse point; remove it by hand: {}",
                current.display()
            )));
        }
        if !metadata.is_dir() {
            return Err(preserved(format!(
                "readiness parent is not a directory: {}",
                current.display()
            )));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn is_reparse_point(metadata: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_reparse_point(_: &Metadata) -> bool {
    false
}

/// A nonblocking exclusive lock on the home's custody file, mirroring the
/// Forge's own custody acquisition. It is never held across a spawn: the
/// new Forge must acquire custody itself.
fn acquire_custody_probe(custody: &Path) -> Result<File> {
    let Some(parent) = custody.parent() else {
        return Err(preserved(format!(
            "custody path has no parent: {}",
            custody.display()
        )));
    };
    validate_parent_chain(parent)?;
    let file = open_or_create_custody_file(custody)?;
    let metadata = file
        .metadata()
        .map_err(|_| preserved(format!("cannot inspect {}", custody.display())))?;
    validate_custody_metadata(custody, &metadata)?;
    match fs2::FileExt::try_lock_exclusive(&file) {
        Ok(()) => Ok(file),
        Err(source) if is_lock_contention(&source) => Err(CliError::ForgeCustodyHeld {
            path: custody.to_path_buf(),
        }),
        Err(_) => Err(preserved(format!("cannot lock {}", custody.display()))),
    }
}

/// Opens the pre-existing regular custody file, or creates it atomically
/// without truncating a concurrent creator's file.
fn open_or_create_custody_file(custody: &Path) -> Result<File> {
    let inspect = || match fs::symlink_metadata(custody) {
        Ok(metadata) => validate_custody_metadata(custody, &metadata),
        Err(_) => Err(preserved(format!("cannot inspect {}", custody.display()))),
    };
    if fs::symlink_metadata(custody).is_ok() {
        inspect()?;
        return open_custody(custody, false)
            .map_err(|_| preserved(format!("cannot open {}", custody.display())));
    }
    match open_custody(custody, true) {
        Ok(file) => Ok(file),
        Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {
            inspect()?;
            open_custody(custody, false)
                .map_err(|_| preserved(format!("cannot open {}", custody.display())))
        }
        Err(_) => Err(preserved(format!("cannot create {}", custody.display()))),
    }
}

fn open_custody(custody: &Path, create: bool) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(create);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.mode(0o600);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;

        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options.open(custody)
}

fn validate_custody_metadata(custody: &Path, metadata: &Metadata) -> Result<()> {
    if is_plain_file(metadata) {
        Ok(())
    } else {
        Err(preserved(format!(
            "custody path is not a regular file: {}",
            custody.display()
        )))
    }
}

/// `WouldBlock` everywhere, plus `ERROR_LOCK_VIOLATION` on Windows where
/// `fs2` preserves it instead of mapping it.
fn is_lock_contention(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::WouldBlock
        || (cfg!(windows) && error.raw_os_error() == Some(33))
}
