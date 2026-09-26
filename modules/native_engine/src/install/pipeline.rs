//! Installs one verified release artifact as a new, unactivated generation.
//!
//! The caller holds the exclusive install lock. The artifact is streamed
//! through the vendor digest check before anything is extracted, extraction
//! happens in a private `staging-<hex>` directory, the executable is measured
//! and verified, and only then is the staging directory renamed to
//! `generation-<hex>`. Interrupted installs leave only staging directories,
//! which the next install removes.

use std::{
    fs, io,
    io::{BufReader, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256, Sha512};
use tempfile::Builder;

use crate::io as native_files;

use super::{
    archive::{ArchiveError, Extraction, extract_gzip_tar, measure},
    authority::ManagedEngineAuthority,
    catalog::{ArtifactPlan, HostPlatform, Layout, ManagedEngine},
    feed::{ArtifactDigest, ReleaseArtifact},
    operations::{InstallError, InstallProgress},
    spec::{ManagedInstallLock, ManagedInstallPaths},
    state::ManagedGeneration,
    transport::{ReleaseTransport, TransportError},
    trust::{TrustError, check_or_record},
};

const STAGING_PREFIX: &str = "staging-";
pub(crate) const GENERATION_PREFIX: &str = "generation-";

/// Downloads, verifies, extracts, measures, and publishes `artifact` as a new
/// generation directory. The generation is not activated.
pub(crate) fn install_artifact(
    authority: ManagedEngineAuthority,
    paths: &ManagedInstallPaths,
    lock: &ManagedInstallLock,
    transport: &dyn ReleaseTransport,
    artifact: &ReleaseArtifact,
    progress: &dyn Fn(InstallProgress),
) -> Result<ManagedGeneration, InstallError> {
    let plan = authority
        .plan()
        .map_err(|_| InstallError::UnsupportedPlatform)?;
    if !authority.engine().meets_floor(&artifact.version) {
        return Err(InstallError::BelowFloor);
    }
    cleanup_staging(paths, lock)?;
    let staging = StagingDirectory::create(paths, lock)?;
    let trust = TrustScope {
        engine_root: paths.engine_root(),
        engine: authority.engine(),
        platform: authority.platform(),
    };
    let entry = staging.path.join(plan.layout.entry());
    match plan.layout {
        Layout::SingleBinary { .. } => {
            let mut file = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&entry)
                .map_err(|_| InstallError::GenerationUnavailable)?;
            download_verified(transport, artifact, &plan, &trust, &mut file, progress)?;
            file.sync_all()
                .map_err(|_| InstallError::GenerationUnavailable)?;
            make_executable(&entry)?;
        }
        Layout::TarMember {
            member,
            archive: policy,
            ..
        } => {
            let mut archive =
                download_archive(paths, transport, artifact, &plan, &trust, progress)?;
            progress(InstallProgress::Extracting);
            extract_gzip_tar(
                BufReader::new(archive.as_file_mut()),
                Extraction::Member {
                    member,
                    target: &entry,
                },
                &policy,
            )
            .map_err(InstallError::Archive)?;
            make_executable(&entry)?;
        }
        Layout::TarTree {
            strip,
            archive: policy,
            ..
        } => {
            let mut archive =
                download_archive(paths, transport, artifact, &plan, &trust, progress)?;
            progress(InstallProgress::Extracting);
            extract_gzip_tar(
                BufReader::new(archive.as_file_mut()),
                Extraction::Tree {
                    strip,
                    destination: &staging.path,
                },
                &policy,
            )
            .map_err(InstallError::Archive)?;
        }
    }
    progress(InstallProgress::Verifying);
    lock.fence(paths).map_err(InstallError::Lock)?;
    let (sha256, size) = measure(&entry).map_err(|error| match error {
        ArchiveError::TargetMissing => InstallError::Archive(ArchiveError::TargetMissing),
        _ => InstallError::ExecutableInvalid,
    })?;
    let staged_id = native_files::verify_file(&entry, size, &sha256)
        .map_err(|_| InstallError::ExecutableInvalid)?;
    let generation_id = random_name(GENERATION_PREFIX)?;
    let published = staging.publish(paths, lock, &generation_id)?;
    let published_id =
        native_files::verify_file(&published.join(plan.layout.entry()), size, &sha256)
            .map_err(|_| InstallError::ExecutableInvalid)?;
    if published_id != staged_id {
        return Err(InstallError::ExecutableInvalid);
    }
    Ok(ManagedGeneration {
        binary: plan.layout.entry().to_owned(),
        directory: generation_id,
        sha256: hex_lower(&sha256),
        size: Some(size),
        version: artifact.version.to_string(),
    })
}

fn download_archive(
    paths: &ManagedInstallPaths,
    transport: &dyn ReleaseTransport,
    artifact: &ReleaseArtifact,
    plan: &ArtifactPlan,
    trust: &TrustScope<'_>,
    progress: &dyn Fn(InstallProgress),
) -> Result<tempfile::NamedTempFile, InstallError> {
    let mut archive = Builder::new()
        .prefix(".download-")
        .suffix(".tgz")
        .tempfile_in(paths.versions_root())
        .map_err(|_| InstallError::GenerationUnavailable)?;
    if archive.path().parent() != Some(paths.versions_root()) {
        return Err(InstallError::GenerationUnavailable);
    }
    download_verified(
        transport,
        artifact,
        plan,
        trust,
        archive.as_file_mut(),
        progress,
    )?;
    archive
        .as_file()
        .sync_all()
        .map_err(|_| InstallError::GenerationUnavailable)?;
    archive
        .as_file_mut()
        .seek(SeekFrom::Start(0))
        .map_err(|_| InstallError::GenerationUnavailable)?;
    Ok(archive)
}

/// Streams the artifact into `sink` while hashing it, then requires the
/// vendor digest and, when published, the exact size.
fn download_verified(
    transport: &dyn ReleaseTransport,
    artifact: &ReleaseArtifact,
    plan: &ArtifactPlan,
    trust: &TrustScope<'_>,
    sink: &mut dyn Write,
    progress: &dyn Fn(InstallProgress),
) -> Result<(), InstallError> {
    let bound = artifact
        .size_bytes
        .map_or(plan.download_bound_bytes, |size| {
            size.min(plan.download_bound_bytes)
        });
    if artifact
        .size_bytes
        .is_some_and(|size| size > plan.download_bound_bytes)
    {
        return Err(InstallError::Transport(TransportError::TooLarge));
    }
    let mut writer = HashingWriter {
        sink,
        sha256: Sha256::new(),
        sha512: Sha512::new(),
        received: 0,
        total: artifact.size_bytes,
        progress,
    };
    progress(InstallProgress::Downloading {
        received_bytes: 0,
        total_bytes: artifact.size_bytes,
    });
    let total = transport
        .download(&artifact.url, bound, &mut writer)
        .map_err(InstallError::Transport)?;
    writer
        .flush()
        .map_err(|_| InstallError::GenerationUnavailable)?;
    if artifact.size_bytes.is_some_and(|size| size != total) {
        return Err(InstallError::IntegrityMismatch);
    }
    let matches = match artifact.digest {
        ArtifactDigest::Sha256(expected) => writer.sha256.finalize()[..] == expected[..],
        ArtifactDigest::Sha512(expected) => writer.sha512.finalize()[..] == expected[..],
        ArtifactDigest::TrustOnFirstDownload => {
            let sha256 = hex_lower(&writer.sha256.finalize());
            return trust.check(artifact, &sha256, total);
        }
    };
    if matches {
        Ok(())
    } else {
        Err(InstallError::IntegrityMismatch)
    }
}

struct HashingWriter<'a> {
    sink: &'a mut dyn Write,
    sha256: Sha256,
    sha512: Sha512,
    received: u64,
    total: Option<u64>,
    progress: &'a dyn Fn(InstallProgress),
}

impl Write for HashingWriter<'_> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let written = self.sink.write(buffer)?;
        self.sha256.update(&buffer[..written]);
        self.sha512.update(&buffer[..written]);
        let before = self.received / PROGRESS_STEP;
        self.received += written as u64;
        if self.received / PROGRESS_STEP != before {
            (self.progress)(InstallProgress::Downloading {
                received_bytes: self.received,
                total_bytes: self.total,
            });
        }
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.sink.flush()
    }
}

const PROGRESS_STEP: u64 = 4 * 1024 * 1024;

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), InstallError> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .map_err(|_| InstallError::ExecutableInvalid)
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<(), InstallError> {
    Ok(())
}

/// Removes leftover staging directories from interrupted installs.
pub(crate) fn cleanup_staging(
    paths: &ManagedInstallPaths,
    lock: &ManagedInstallLock,
) -> Result<(), InstallError> {
    lock.fence(paths).map_err(InstallError::Lock)?;
    for candidate in owned_directories(paths, is_staging_name)? {
        native_files::verify_directory(&candidate).map_err(|_| InstallError::CleanupFailed)?;
        fs::remove_dir_all(&candidate).map_err(|_| InstallError::CleanupFailed)?;
    }
    lock.fence(paths).map_err(InstallError::Lock)
}

/// Removes generation directories the state no longer references. Removal
/// is best effort: a generation still executing on Windows stays until the
/// next prune.
pub(crate) fn prune_generations<'a>(
    paths: &ManagedInstallPaths,
    lock: &ManagedInstallLock,
    referenced: impl Iterator<Item = &'a str>,
) -> Result<(), InstallError> {
    let referenced = referenced.collect::<Vec<_>>();
    lock.fence(paths).map_err(InstallError::Lock)?;
    for candidate in owned_directories(paths, is_generation_name)? {
        let keep = candidate
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| referenced.contains(&name));
        if !keep && native_files::verify_directory(&candidate).is_ok() {
            let _ = fs::remove_dir_all(&candidate);
        }
    }
    Ok(())
}

fn owned_directories(
    paths: &ManagedInstallPaths,
    owned: fn(&str) -> bool,
) -> Result<Vec<PathBuf>, InstallError> {
    let entries = fs::read_dir(paths.versions_root()).map_err(|_| InstallError::CleanupFailed)?;
    let mut directories = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|_| InstallError::CleanupFailed)?;
        if entry.file_name().to_str().is_some_and(owned) {
            directories.push(entry.path());
        }
    }
    Ok(directories)
}

pub(crate) fn is_staging_name(name: &str) -> bool {
    is_hex_name(name, STAGING_PREFIX)
}

pub(crate) fn is_generation_name(name: &str) -> bool {
    is_hex_name(name, GENERATION_PREFIX)
}

fn is_hex_name(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix).is_some_and(|hex| {
        hex.len() == 32
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

struct StagingDirectory {
    path: PathBuf,
    versions_root: PathBuf,
    published: bool,
}

impl StagingDirectory {
    fn create(
        paths: &ManagedInstallPaths,
        lock: &ManagedInstallLock,
    ) -> Result<Self, InstallError> {
        for _ in 0..8 {
            lock.fence(paths).map_err(InstallError::Lock)?;
            let path = paths.versions_root().join(random_name(STAGING_PREFIX)?);
            match fs::create_dir(&path) {
                Ok(()) => {
                    native_files::verify_directory(&path)
                        .map_err(|_| InstallError::GenerationUnavailable)?;
                    return Ok(Self {
                        path,
                        versions_root: paths.versions_root().to_path_buf(),
                        published: false,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(_) => return Err(InstallError::GenerationUnavailable),
            }
        }
        Err(InstallError::GenerationCollision)
    }

    fn publish(
        mut self,
        paths: &ManagedInstallPaths,
        lock: &ManagedInstallLock,
        generation_id: &str,
    ) -> Result<PathBuf, InstallError> {
        if !is_generation_name(generation_id) {
            return Err(InstallError::GenerationUnavailable);
        }
        native_files::verify_directory(&self.path)
            .map_err(|_| InstallError::GenerationUnavailable)?;
        lock.fence(paths).map_err(InstallError::Lock)?;
        let destination = paths.versions_root().join(generation_id);
        if fs::symlink_metadata(&destination).is_ok() {
            return Err(InstallError::GenerationCollision);
        }
        fs::rename(&self.path, &destination).map_err(|error| {
            if error.kind() == io::ErrorKind::AlreadyExists {
                InstallError::GenerationCollision
            } else {
                InstallError::GenerationUnavailable
            }
        })?;
        self.published = true;
        native_files::verify_directory(&destination)
            .map_err(|_| InstallError::GenerationUnavailable)?;
        lock.fence(paths).map_err(InstallError::Lock)?;
        Ok(destination)
    }
}

impl Drop for StagingDirectory {
    fn drop(&mut self) {
        if !self.published
            && self.path.parent() == Some(self.versions_root.as_path())
            && self
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(is_staging_name)
            && native_files::verify_directory(&self.path).is_ok()
        {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

fn random_name(prefix: &str) -> Result<String, InstallError> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| InstallError::RandomUnavailable)?;
    Ok(format!("{prefix}{}", hex_lower(&bytes)))
}

pub(crate) fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push(HEX[usize::from(byte >> 4)] as char);
        result.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    result
}

/// Where a trust-on-first-download record of this install lives.
struct TrustScope<'a> {
    engine_root: &'a Path,
    engine: ManagedEngine,
    platform: HostPlatform,
}

impl TrustScope<'_> {
    /// Checks a vendor-digest-free download against its first-download
    /// record, recording it the first time. A mismatch keeps the record.
    fn check(
        &self,
        artifact: &ReleaseArtifact,
        sha256: &str,
        size: u64,
    ) -> Result<(), InstallError> {
        check_or_record(
            self.engine_root,
            self.engine,
            self.platform,
            &artifact.version,
            &artifact.url,
            sha256,
            size,
        )
        .map(|_| ())
        .map_err(|error| match error {
            TrustError::Mismatch => InstallError::TrustMismatch,
            TrustError::Store(_) => InstallError::StateInvalid,
        })
    }
}
