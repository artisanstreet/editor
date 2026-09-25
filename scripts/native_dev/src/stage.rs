//! Content staging with lock and activate semantics.
//!
//! Updates never touch the active version in place. Binaries stage into a
//! scratch directory, the scratch payload is verified, and only then is it
//! swapped in — mirroring the installer's tmp/previous/rename activation
//! pattern. A failed update leaves the previous binaries, manifest, and
//! dev data exactly as they were. One inter-run OS lock (`fs2`, released
//! automatically if the stager dies) serializes concurrent `dev` runs.

use std::{fs, io::Read, path::Path};

use fs2::FileExt;
use sha2::{Digest, Sha256};

use crate::{
    binaries::BinarySet,
    error::DevError,
    manifest::{verify_payload_dir, write_payload_manifest},
    paths::{DevPaths, exe_name},
};

/// Lowercase hex SHA-256 of one file.
///
/// # Errors
///
/// Returns [`DevError::Stage`] when the file cannot be read.
pub fn hash_file(path: &Path) -> Result<String, DevError> {
    let mut file = fs::File::open(path).map_err(|_| DevError::Stage {
        stage: "stage",
        reason: format!("cannot read {}", path.display()),
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|_| DevError::Stage {
            stage: "stage",
            reason: format!("cannot read {}", path.display()),
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex_encode(&hasher.finalize()))
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut text, byte| {
            use std::fmt::Write;
            let _ = write!(text, "{byte:02x}");
            text
        })
}

/// Writes one file atomically through a sibling temporary plus rename.
///
/// # Errors
///
/// Returns [`DevError::Stage`] when the write fails.
pub fn write_atomic(path: &Path, bytes: &[u8], stage: &'static str) -> Result<(), DevError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|_| DevError::Stage {
            stage,
            reason: format!("cannot create {}", parent.display()),
        })?;
    }
    let temporary = path.with_extension("tmp-dev-write");
    fs::write(&temporary, bytes).map_err(|_| DevError::Stage {
        stage,
        reason: format!("cannot write {}", path.display()),
    })?;
    fs::rename(&temporary, path).map_err(|_| DevError::Stage {
        stage,
        reason: format!("cannot activate {}", path.display()),
    })?;
    Ok(())
}

/// Inter-run staging lock, held for the whole mutation sequence.
///
/// The OS lock releases automatically when the holder dies, so a crashed
/// run never wedges later runs. The file itself is retained.
pub struct DevLock {
    _file: fs::File,
}

impl DevLock {
    /// Acquires the staging lock for one dev directory.
    ///
    /// # Errors
    ///
    /// Returns [`DevError::StagingLocked`] when another run holds it, or
    /// [`DevError::Stage`] when the lock file cannot be created.
    pub fn acquire(paths: &DevPaths) -> Result<Self, DevError> {
        if let Some(parent) = paths.lock_path().parent() {
            fs::create_dir_all(parent).map_err(|_| DevError::Stage {
                stage: "stage",
                reason: format!("cannot create {}", parent.display()),
            })?;
        }
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(paths.lock_path())
            .map_err(|_| DevError::Stage {
                stage: "stage",
                reason: format!("cannot open {}", paths.lock_path().display()),
            })?;
        match file.try_lock_exclusive() {
            Ok(()) => Ok(Self { _file: file }),
            Err(_) => Err(DevError::StagingLocked {
                lock: paths.lock_path(),
            }),
        }
    }
}

/// Copies one file, creating its parent directory.
///
/// # Errors
///
/// Returns [`DevError::Stage`] when the copy fails.
fn stage_copy(source: &Path, destination: &Path) -> Result<(), DevError> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|_| DevError::Stage {
            stage: "stage",
            reason: format!("cannot create {}", parent.display()),
        })?;
    }
    fs::copy(source, destination).map_err(|_| DevError::Stage {
        stage: "stage",
        reason: format!("cannot stage {}", destination.display()),
    })?;
    Ok(())
}

/// Outcome counts for one staging run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StageCounts {
    /// Binaries whose bytes changed and were staged.
    pub rewritten: usize,
    /// Binaries identical to the active version.
    pub reused: usize,
}

/// Stages, verifies, and activates the four binaries plus the permanent
/// `ae` launcher.
///
/// The active version is replaced only after the scratch payload verifies,
/// and the previous tree is retained until the swap completes. When every
/// binary is identical, activation is skipped entirely so a running dev
/// session is never disturbed. A failure removes the scratch tree, so the
/// active version and dev data are left untouched.
///
/// # Errors
///
/// Returns [`DevError::Stage`] or [`DevError::PayloadUnverified`] when any
/// step fails; the active version and dev data are left untouched.
pub fn stage_binaries(set: &BinarySet, paths: &DevPaths) -> Result<StageCounts, DevError> {
    stage_payload(set, None, paths)
}

/// Stages the binaries like [`stage_binaries`] plus, when given, the build
/// identity document at [`artisan_build_info::RESOURCE_PATH`], which the
/// payload manifest then covers like every binary.
///
/// # Errors
///
/// Returns [`DevError::Stage`] or [`DevError::PayloadUnverified`] when any
/// step fails; the active version and dev data are left untouched.
pub fn stage_payload(
    set: &BinarySet,
    identity: Option<&artisan_build_info::BuildInfo>,
    paths: &DevPaths,
) -> Result<StageCounts, DevError> {
    let result = stage_payload_inner(set, identity, paths);
    if result.is_err() {
        let _ = fs::remove_dir_all(paths.staging_root());
    }
    result
}

fn stage_payload_inner(
    set: &BinarySet,
    identity: Option<&artisan_build_info::BuildInfo>,
    paths: &DevPaths,
) -> Result<StageCounts, DevError> {
    // Capture the source digests first: activation later asserts the
    // staged bytes equal these, so a corrupt copy or a tampered scratch
    // tree can never be "verified" against its own bytes.
    let mut sources = Vec::with_capacity(4);
    for (relative, source) in set.entries() {
        let digest = hash_file(&source)?;
        sources.push((relative, source, digest));
    }
    let staging = paths.staging_root();
    if staging.exists() {
        fs::remove_dir_all(&staging).map_err(|_| DevError::Stage {
            stage: "stage",
            reason: format!("cannot clear {}", staging.display()),
        })?;
    }
    let mut rewritten = 0_usize;
    let mut reused = 0_usize;
    for (relative, source, expected) in &sources {
        let staged = staging.join(relative);
        let active = paths.version_root.join(relative);
        if active.is_file() && hash_file(&active).is_ok_and(|current| current == *expected) {
            reused += 1;
            stage_copy(&active, &staged)?;
        } else {
            stage_copy(source, &staged)?;
            rewritten += 1;
        }
        // Changed or not, the scratch tree must equal the captured source
        // bytes before anything verifies or activates it.
        if hash_file(&staged)? != *expected {
            return Err(DevError::Stage {
                stage: "stage",
                reason: format!("staged copy does not match source: {}", staged.display()),
            });
        }
    }
    if let Some(identity) = identity {
        let bytes = identity.to_json();
        let relative = artisan_build_info::RESOURCE_PATH;
        write_atomic(&staging.join(relative), &bytes, "stage")?;
        if fs::read(paths.version_root.join(relative)).is_ok_and(|current| current == bytes) {
            reused += 1;
        } else {
            rewritten += 1;
        }
    }
    write_payload_manifest(&staging)?;
    verify_payload_dir(&staging)?;
    permanent_launcher(set, paths)?;
    if rewritten == 0 && paths.version_root.is_dir() {
        verify_payload_dir(&paths.version_root)?;
        fs::remove_dir_all(&staging).map_err(|_| DevError::Stage {
            stage: "stage",
            reason: format!("cannot clear {}", staging.display()),
        })?;
        return Ok(StageCounts { rewritten, reused });
    }
    activate(paths)?;
    verify_payload_dir(&paths.version_root)?;
    Ok(StageCounts { rewritten, reused })
}

/// Stages the permanent `ae` launcher beside the home (outside versions).
///
/// Compared by hash like the versioned binaries; identical launchers are
/// left alone so a running session keeps its file. The staged copy is
/// asserted against the captured source digest before returning.
fn permanent_launcher(set: &BinarySet, paths: &DevPaths) -> Result<(), DevError> {
    let expected = hash_file(&set.ae)?;
    if paths.permanent_ae.is_file()
        && hash_file(&paths.permanent_ae).is_ok_and(|current| current == expected)
    {
        return Ok(());
    }
    stage_copy(&set.ae, &paths.permanent_ae)?;
    if hash_file(&paths.permanent_ae)? != expected {
        return Err(DevError::Stage {
            stage: "stage",
            reason: format!(
                "staged copy does not match source: {}",
                paths.permanent_ae.display()
            ),
        });
    }
    Ok(())
}

/// Swaps the verified scratch tree into the active version.
///
/// The previous tree is retained until the swap completes, then removed.
/// Follows the installer's previous/rename discipline: any failure before
/// the final rename leaves the active version untouched.
fn activate(paths: &DevPaths) -> Result<(), DevError> {
    let staging = paths.staging_root();
    let active = &paths.version_root;
    let previous = paths.previous_root();
    if previous.exists() {
        fs::remove_dir_all(&previous).map_err(|_| DevError::Stage {
            stage: "stage",
            reason: format!("cannot clear {}", previous.display()),
        })?;
    }
    if active.exists() {
        fs::rename(active, &previous).map_err(|_| DevError::Stage {
            stage: "stage",
            reason: format!("cannot retire {}", active.display()),
        })?;
    }
    if let Err(error) = fs::rename(&staging, active) {
        let _ = fs::rename(&previous, active);
        return Err(DevError::Stage {
            stage: "stage",
            reason: format!("cannot activate {}: {error}", active.display()),
        });
    }
    if previous.exists() {
        fs::remove_dir_all(&previous).map_err(|_| DevError::Stage {
            stage: "stage",
            reason: format!("cannot clear {}", previous.display()),
        })?;
    }
    Ok(())
}

/// Relative payload names staged into one version root, in order.
#[must_use]
pub fn staged_relative_names() -> [String; 4] {
    [
        format!("bin/{}", exe_name("ae")),
        format!("bin/{}", exe_name("installer")),
        format!("bin/{}", exe_name("editor")),
        format!("bin/{}", exe_name("forge")),
    ]
}
