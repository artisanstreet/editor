use std::{
    cell::RefCell,
    ffi::OsString,
    fs::{File, Metadata, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    process::Stdio,
};

use chrono::Utc;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

use crate::{
    archive,
    background_process::{background_command, detached_background_command},
    error::{InstallerError, Result, io},
    integrations::{
        OwnedIntegration, apply_protocol, prepare_protocol, remove_protocol, verify_protocol,
    },
    manifest::{Artifact, TrustKey, fetch},
    platform::Platform,
    processes::{Retirement, RetirementPolicy, retire_superseded},
    shortcuts,
};

const ABSOLUTE_ARTIFACT_LIMIT: u64 = 2 * 1024 * 1024 * 1024;
const NATIVE_PAYLOAD_LABEL: &str = "native payload";
const INSTALLER_LOCK_NAME: &str = ".installer.lock";
const AE_REPLACEMENT_MARKER_SUFFIX: &str = ".artisan-installer-ae-replacement.pending";
const CLEANUP_MARKER_SUFFIX: &str = ".artisan-installer-cleanup.pending";
#[cfg(windows)]
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
#[cfg(windows)]
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
/// First install configures and verifies Forge, but deliberately leaves launch
/// to the editor's background handoff. That gives the window exact ownership
/// of the process it caused and lets normal window close stop that Forge. An
/// explicit autostart task remains independently owned by `ae setup --autostart`.
const FIRST_RUN_CONFIGURATION_COMMANDS: [&[&str]; 3] = [&["setup"], &["doctor"], &["status"]];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RootMode {
    Create,
    Existing,
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct FileIdentity(u64, u64);

#[derive(Clone, Copy)]
enum EntryKind {
    File,
    Directory,
}

#[derive(Clone, Copy)]
enum PendingMarkerKind {
    AeReplacement,
    Cleanup,
}

impl PendingMarkerKind {
    const fn suffix(self) -> &'static str {
        match self {
            Self::AeReplacement => AE_REPLACEMENT_MARKER_SUFFIX,
            Self::Cleanup => CLEANUP_MARKER_SUFFIX,
        }
    }
}

/// A private, per-install-root OS lock. The file is deliberately retained for
/// the whole lifecycle operation and is never removed or truncated.
struct InstallerLock {
    file: File,
    root: PathBuf,
    root_identity: FileIdentity,
    lock_identity: FileIdentity,
    owned_marker: RefCell<Option<MarkerFence>>,
}

impl std::fmt::Debug for InstallerLock {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("InstallerLock")
    }
}

impl InstallerLock {
    fn acquire(root: &Path, mode: RootMode) -> Result<Self> {
        let root_identity = ensure_root(root, mode)?;
        ensure_no_pending_markers(root)?;
        let lock_path = root.join(INSTALLER_LOCK_NAME);
        let (file, lock_identity) = open_lock_file(&lock_path)?;
        let lock = Self {
            file,
            root: root.to_path_buf(),
            root_identity,
            lock_identity,
            owned_marker: RefCell::new(None),
        };
        lock.re_fence()?;
        lock.fence()?;
        Ok(lock)
    }

    /// Revalidate both the root and the path identity of the already-opened
    /// sentinel without ever replacing the retained handle.
    fn re_fence(&self) -> Result<()> {
        let current_root = ensure_root(&self.root, RootMode::Existing)?;
        if current_root != self.root_identity {
            return Err(InstallerError::InstallationRootChanged);
        }

        let lock_path = self.root.join(INSTALLER_LOCK_NAME);
        let current_lock = ordinary_path_identity(&lock_path, EntryKind::File)
            .map_err(|()| InstallerError::InvalidInstallerLock)?;
        if current_lock != self.lock_identity {
            return Err(InstallerError::InstallationRootChanged);
        }
        let handle_identity = identity_from_file(&self.file, EntryKind::File)
            .map_err(|()| InstallerError::InvalidInstallerLock)?;
        if handle_identity != self.lock_identity {
            return Err(InstallerError::InstallationRootChanged);
        }
        Ok(())
    }

    fn fence(&self) -> Result<()> {
        self.re_fence()?;
        let marker = self
            .owned_marker
            .borrow()
            .as_ref()
            .map(|marker| MarkerFence {
                path: marker.path.clone(),
                identity: marker.identity,
            });
        if let Some(marker) = marker.as_ref() {
            if ensure_owned_marker(self, marker)? {
                Ok(())
            } else {
                self.owned_marker.replace(None);
                ensure_no_pending_markers(&self.root)
            }
        } else {
            ensure_no_pending_markers(&self.root)
        }
    }
}

#[derive(Clone)]
struct MarkerFence {
    path: PathBuf,
    identity: FileIdentity,
}

/// Marker ownership is intentionally explicit. In particular, this type does
/// not implement `Drop`: a failed spawn must leave the marker behind.
struct PendingMarker {
    path: PathBuf,
    identity: FileIdentity,
}

impl std::fmt::Debug for PendingMarker {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("PendingMarker")
    }
}

impl PendingMarker {
    fn create(lock: &InstallerLock, kind: PendingMarkerKind) -> Result<Self> {
        lock.re_fence()?;
        ensure_no_pending_markers(&lock.root)?;
        let path = pending_marker_path(&lock.root, kind)?;
        match std::fs::symlink_metadata(&path) {
            Ok(_) => return Err(InstallerError::InstallationRootPending),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(InstallerError::InvalidInstallerMarker),
        }
        match std::fs::create_dir(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err(InstallerError::InstallationRootPending);
            }
            Err(_) => return Err(InstallerError::InvalidInstallerMarker),
        }
        let metadata =
            std::fs::symlink_metadata(&path).map_err(|_| InstallerError::InvalidInstallerMarker)?;
        if !ordinary_metadata(&metadata, EntryKind::Directory) {
            return Err(InstallerError::InvalidInstallerMarker);
        }
        let identity = ordinary_path_identity(&path, EntryKind::Directory)
            .map_err(|()| InstallerError::InvalidInstallerMarker)?;
        ensure_no_pending_markers_except(&lock.root, &path)?;
        lock.owned_marker.replace(Some(MarkerFence {
            path: path.clone(),
            identity,
        }));
        Ok(Self { path, identity })
    }

    #[cfg_attr(not(test), allow(dead_code))]
    fn clear_after_success(&self) -> Result<()> {
        let metadata = std::fs::symlink_metadata(&self.path)
            .map_err(|_| InstallerError::InvalidInstallerMarker)?;
        if !ordinary_metadata(&metadata, EntryKind::Directory) {
            return Err(InstallerError::InvalidInstallerMarker);
        }
        let identity = ordinary_path_identity(&self.path, EntryKind::Directory)
            .map_err(|()| InstallerError::InvalidInstallerMarker)?;
        if identity != self.identity {
            return Err(InstallerError::InstallationRootChanged);
        }
        std::fs::remove_dir(&self.path).map_err(|_| InstallerError::InvalidInstallerMarker)
    }
}

fn ensure_owned_marker(lock: &InstallerLock, marker: &MarkerFence) -> Result<bool> {
    let metadata = match std::fs::symlink_metadata(&marker.path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(_) => return Err(InstallerError::InvalidInstallerMarker),
    };
    if !ordinary_metadata(&metadata, EntryKind::Directory) {
        return Err(InstallerError::InvalidInstallerMarker);
    }
    let identity = ordinary_path_identity(&marker.path, EntryKind::Directory)
        .map_err(|()| InstallerError::InvalidInstallerMarker)?;
    if identity != marker.identity {
        return Err(InstallerError::InstallationRootChanged);
    }
    ensure_no_pending_markers_except(&lock.root, &marker.path)?;
    Ok(true)
}

fn ensure_root(root: &Path, mode: RootMode) -> Result<FileIdentity> {
    if !root.is_absolute()
        || root.file_name().is_none()
        || root
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(InstallerError::UnsafeInstallationRoot);
    }

    let mut ancestors: Vec<PathBuf> = root
        .ancestors()
        .filter(|path| !path.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .collect();
    ancestors.reverse();

    for ancestor in ancestors {
        match std::fs::symlink_metadata(&ancestor) {
            Ok(metadata) => {
                if !ordinary_metadata(&metadata, EntryKind::Directory) {
                    return Err(InstallerError::UnsafeInstallationRoot);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if mode == RootMode::Existing {
                    return Err(InstallerError::UnsafeInstallationRoot);
                }
                match std::fs::create_dir(&ancestor) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(_) => return Err(InstallerError::UnsafeInstallationRoot),
                }
                let metadata = std::fs::symlink_metadata(&ancestor)
                    .map_err(|_| InstallerError::UnsafeInstallationRoot)?;
                if !ordinary_metadata(&metadata, EntryKind::Directory) {
                    return Err(InstallerError::UnsafeInstallationRoot);
                }
            }
            Err(_) => return Err(InstallerError::UnsafeInstallationRoot),
        }
    }

    ordinary_path_identity(root, EntryKind::Directory)
        .map_err(|()| InstallerError::UnsafeInstallationRoot)
}

fn open_lock_file(path: &Path) -> Result<(File, FileIdentity)> {
    let preexisting_identity = match std::fs::symlink_metadata(path) {
        Ok(metadata) if ordinary_metadata(&metadata, EntryKind::File) => Some(
            ordinary_path_identity(path, EntryKind::File)
                .map_err(|()| InstallerError::InvalidInstallerLock)?,
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Ok(_) | Err(_) => return Err(InstallerError::InvalidInstallerLock),
    };

    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    configure_ordinary_open(&mut options, EntryKind::File);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.mode(0o600);
    }
    let file = match preexisting_identity {
        Some(_) => {
            let mut existing = OpenOptions::new();
            existing.read(true).write(true);
            configure_ordinary_open(&mut existing, EntryKind::File);
            existing
                .open(path)
                .map_err(|_| InstallerError::InvalidInstallerLock)?
        }
        None => match options.open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                match std::fs::symlink_metadata(path) {
                    Ok(metadata) if ordinary_metadata(&metadata, EntryKind::File) => {}
                    Ok(_) | Err(_) => return Err(InstallerError::InvalidInstallerLock),
                }
                let mut existing = OpenOptions::new();
                existing.read(true).write(true);
                configure_ordinary_open(&mut existing, EntryKind::File);
                existing
                    .open(path)
                    .map_err(|_| InstallerError::InvalidInstallerLock)?
            }
            Err(_) => return Err(InstallerError::InvalidInstallerLock),
        },
    };

    let identity = identity_from_file(&file, EntryKind::File)
        .map_err(|()| InstallerError::InvalidInstallerLock)?;
    if preexisting_identity.is_some_and(|expected| expected != identity) {
        return Err(InstallerError::InstallationRootChanged);
    }
    file.try_lock_exclusive().map_err(|error| {
        let is_contended = error.kind() == std::io::ErrorKind::WouldBlock
            || error.raw_os_error().is_some_and(|raw_os_error| {
                Some(raw_os_error) == fs2::lock_contended_error().raw_os_error()
            });
        if is_contended {
            InstallerError::InstallationRootBusy
        } else {
            InstallerError::InvalidInstallerLock
        }
    })?;
    let path_identity = ordinary_path_identity(path, EntryKind::File)
        .map_err(|()| InstallerError::InvalidInstallerLock)?;
    if path_identity != identity {
        return Err(InstallerError::InstallationRootChanged);
    }
    Ok((file, identity))
}

fn ensure_no_pending_markers(root: &Path) -> Result<()> {
    let paths = pending_marker_paths(root)?;
    for path in paths {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if ordinary_metadata(&metadata, EntryKind::Directory) => {
                return Err(InstallerError::InstallationRootPending);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) | Err(_) => return Err(InstallerError::InvalidInstallerMarker),
        }
    }
    Ok(())
}

fn ensure_no_pending_markers_except(root: &Path, own_marker: &Path) -> Result<()> {
    for path in pending_marker_paths(root)? {
        if path == own_marker {
            continue;
        }
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if ordinary_metadata(&metadata, EntryKind::Directory) => {
                return Err(InstallerError::InstallationRootPending);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) | Err(_) => return Err(InstallerError::InvalidInstallerMarker),
        }
    }
    Ok(())
}

fn pending_marker_paths(root: &Path) -> Result<[PathBuf; 2]> {
    let parent = root
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or(InstallerError::UnsafeInstallationRoot)?;
    let name = root
        .file_name()
        .ok_or(InstallerError::UnsafeInstallationRoot)?;
    Ok([
        pending_marker_path_from(parent, name, AE_REPLACEMENT_MARKER_SUFFIX),
        pending_marker_path_from(parent, name, CLEANUP_MARKER_SUFFIX),
    ])
}

fn pending_marker_path(root: &Path, kind: PendingMarkerKind) -> Result<PathBuf> {
    let parent = root
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or(InstallerError::UnsafeInstallationRoot)?;
    let name = root
        .file_name()
        .ok_or(InstallerError::UnsafeInstallationRoot)?;
    Ok(pending_marker_path_from(parent, name, kind.suffix()))
}

fn pending_marker_path_from(parent: &Path, name: &std::ffi::OsStr, suffix: &str) -> PathBuf {
    let mut marker_name = OsString::from(".");
    marker_name.push(name);
    marker_name.push(suffix);
    parent.join(marker_name)
}

fn ordinary_metadata(metadata: &Metadata, kind: EntryKind) -> bool {
    if metadata_is_symlink_or_reparse(metadata) {
        return false;
    }
    match kind {
        EntryKind::File => metadata.is_file(),
        EntryKind::Directory => metadata.is_dir(),
    }
}

fn ordinary_path_identity(path: &Path, kind: EntryKind) -> std::result::Result<FileIdentity, ()> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| ())?;
    if !ordinary_metadata(&metadata, kind) {
        return Err(());
    }
    let identity = opened_path_identity(path, kind)?;
    #[cfg(unix)]
    if identity != identity_from_metadata(&metadata) {
        return Err(());
    }
    let second_identity = opened_path_identity(path, kind)?;
    if identity != second_identity {
        return Err(());
    }
    let metadata = std::fs::symlink_metadata(path).map_err(|_| ())?;
    if !ordinary_metadata(&metadata, kind) {
        return Err(());
    }
    #[cfg(unix)]
    if identity != identity_from_metadata(&metadata) {
        return Err(());
    }
    Ok(identity)
}

fn identity_from_file(file: &File, kind: EntryKind) -> std::result::Result<FileIdentity, ()> {
    let metadata = file.metadata().map_err(|_| ())?;
    if !ordinary_metadata(&metadata, kind) {
        return Err(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        Ok(FileIdentity(metadata.dev(), metadata.ino()))
    }
    #[cfg(windows)]
    {
        let information = winapi_util::file::information(file).map_err(|_| ())?;
        let file_type = winapi_util::file::typ(file).map_err(|_| ())?;
        if !file_type.is_disk()
            || information.file_attributes() & 0x400 != 0
            || !ordinary_metadata(&metadata, kind)
        {
            return Err(());
        }
        Ok(FileIdentity(
            information.volume_serial_number(),
            information.file_index(),
        ))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = metadata;
        Err(())
    }
}

#[cfg(unix)]
fn identity_from_metadata(metadata: &Metadata) -> FileIdentity {
    use std::os::unix::fs::MetadataExt;

    FileIdentity(metadata.dev(), metadata.ino())
}

fn opened_path_identity(path: &Path, kind: EntryKind) -> std::result::Result<FileIdentity, ()> {
    let file = open_for_read(path, kind).map_err(|_| ())?;
    identity_from_file(&file, kind)
}

fn open_for_read(path: &Path, kind: EntryKind) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    configure_ordinary_open(&mut options, kind);
    options.open(path)
}

fn open_for_update(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    configure_ordinary_open(&mut options, EntryKind::File);
    options.open(path)
}

#[cfg(windows)]
fn configure_ordinary_open(options: &mut OpenOptions, kind: EntryKind) {
    use std::os::windows::fs::OpenOptionsExt;

    let mut flags = FILE_FLAG_OPEN_REPARSE_POINT;
    if matches!(kind, EntryKind::Directory) {
        flags |= FILE_FLAG_BACKUP_SEMANTICS;
    }
    options.custom_flags(flags);
}

#[cfg(not(windows))]
fn configure_ordinary_open(_: &mut OpenOptions, _: EntryKind) {}

fn ensure_owned_directory(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if ordinary_metadata(&metadata, EntryKind::Directory) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(path).map_err(|_| InstallerError::UnsafeOwnedPath)?;
        }
        Ok(_) | Err(_) => return Err(InstallerError::UnsafeOwnedPath),
    }
    ordinary_path_identity(path, EntryKind::Directory)
        .map_err(|()| InstallerError::UnsafeOwnedPath)?;
    Ok(())
}

fn owned_file_identity(path: &Path) -> Result<Option<FileIdentity>> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if ordinary_metadata(&metadata, EntryKind::File) => {
            ordinary_path_identity(path, EntryKind::File)
                .map(Some)
                .map_err(|()| InstallerError::UnsafeOwnedPath)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Ok(_) | Err(_) => Err(InstallerError::UnsafeOwnedPath),
    }
}

fn ordinary_file_exists(path: &Path) -> Result<bool> {
    Ok(owned_file_identity(path)?.is_some())
}

fn ordinary_directory_exists(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if ordinary_metadata(&metadata, EntryKind::Directory) => {
            ordinary_path_identity(path, EntryKind::Directory)
                .map(|_| true)
                .map_err(|()| InstallerError::UnsafeOwnedPath)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Ok(_) | Err(_) => Err(InstallerError::UnsafeOwnedPath),
    }
}

fn create_owned_file(path: &Path) -> Result<File> {
    let expected = owned_file_identity(path)?;
    let file = if expected.is_some() {
        open_for_update(path).map_err(|_| InstallerError::UnsafeOwnedPath)?
    } else {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        configure_ordinary_open(&mut options, EntryKind::File);
        options
            .open(path)
            .map_err(|_| InstallerError::UnsafeOwnedPath)?
    };
    let identity =
        identity_from_file(&file, EntryKind::File).map_err(|()| InstallerError::UnsafeOwnedPath)?;
    if expected.is_some_and(|expected| expected != identity)
        || ordinary_path_identity(path, EntryKind::File)
            .map_err(|()| InstallerError::UnsafeOwnedPath)?
            != identity
    {
        return Err(InstallerError::UnsafeOwnedPath);
    }
    file.set_len(0)
        .map_err(|_| InstallerError::UnsafeOwnedPath)?;
    Ok(file)
}

fn copy_owned_file(source: &Path, destination: &Path) -> Result<FileIdentity> {
    let source_identity = ordinary_path_identity(source, EntryKind::File)
        .map_err(|()| InstallerError::UnsafeOwnedPath)?;
    let mut source_file = open_for_read(source, EntryKind::File).map_err(io(source))?;
    if identity_from_file(&source_file, EntryKind::File)
        .map_err(|()| InstallerError::UnsafeOwnedPath)?
        != source_identity
    {
        return Err(InstallerError::UnsafeOwnedPath);
    }
    let permissions = source_file.metadata().map_err(io(source))?.permissions();
    let mut destination_file = create_owned_file(destination)?;
    std::io::copy(&mut source_file, &mut destination_file).map_err(io(destination))?;
    destination_file
        .set_permissions(permissions)
        .map_err(io(destination))?;
    destination_file.sync_all().map_err(io(destination))?;
    identity_from_file(&destination_file, EntryKind::File)
        .map_err(|()| InstallerError::UnsafeOwnedPath)
}

fn require_identity(path: &Path, kind: EntryKind, expected: FileIdentity) -> Result<()> {
    let actual =
        ordinary_path_identity(path, kind).map_err(|()| InstallerError::UnsafeOwnedPath)?;
    if actual != expected {
        return Err(InstallerError::UnsafeOwnedPath);
    }
    Ok(())
}

fn sync_owned_file(path: &Path, file: &File) -> Result<FileIdentity> {
    let identity =
        identity_from_file(file, EntryKind::File).map_err(|()| InstallerError::UnsafeOwnedPath)?;
    require_identity(path, EntryKind::File, identity)?;
    file.sync_all().map_err(io(path))?;
    require_identity(path, EntryKind::File, identity)?;
    Ok(identity)
}

fn remove_owned_file(path: &Path) -> Result<()> {
    let Some(expected) = owned_file_identity(path)? else {
        return Ok(());
    };
    if ordinary_path_identity(path, EntryKind::File)
        .map_err(|()| InstallerError::UnsafeOwnedPath)?
        != expected
    {
        return Err(InstallerError::UnsafeOwnedPath);
    }
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(InstallerError::UnsafeOwnedPath),
    }
}

/// Owns exactly one stage directory after its atomic creation succeeds.
/// Cleanup is explicit because its failure must be reported to the caller.
#[derive(Debug)]
struct StageLease {
    path: PathBuf,
    armed: bool,
}

impl StageLease {
    fn acquire(path: PathBuf, release_version: &str) -> Result<Self> {
        match std::fs::create_dir(&path) {
            Ok(()) => Ok(Self { path, armed: true }),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                Err(InstallerError::ExistingRelease(release_version.to_owned()))
            }
            Err(error) => Err(io(&path)(error)),
        }
    }

    fn transfer_to(&mut self, release: &Path) -> Result<()> {
        std::fs::rename(&self.path, release).map_err(io(release))?;
        self.armed = false;
        Ok(())
    }

    fn cleanup(&mut self) -> Result<()> {
        if !self.armed {
            return Ok(());
        }
        match remove_owned_stage(&self.path) {
            Ok(()) => {
                self.armed = false;
                Ok(())
            }
            Err(_) => Err(InstallerError::StageCleanupIncomplete),
        }
    }

    fn finish(&self) -> Result<()> {
        if self.armed {
            Err(InstallerError::StageCleanupIncomplete)
        } else {
            Ok(())
        }
    }
}

fn complete_install(stage: &mut StageLease, result: Result<()>) -> Result<()> {
    match result {
        Ok(()) => stage.finish(),
        Err(original) => match stage.cleanup() {
            Ok(()) => Err(original),
            Err(cleanup) => Err(cleanup),
        },
    }
}

fn complete_install_locked(
    lock: &InstallerLock,
    stage: &mut StageLease,
    result: Result<()>,
) -> Result<()> {
    if result.is_err() {
        lock.fence()?;
    }
    complete_install(stage, result)
}

fn remove_owned_stage(path: &Path) -> std::io::Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if metadata_is_symlink_or_reparse(&metadata) || !metadata.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "owned stage is not an ordinary directory",
        ));
    }
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn metadata_is_symlink_or_reparse(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}

pub struct InstallIntegrationOptions {
    /// Whether this install may own the `artisan://` handler. A secondary
    /// install beside an existing installation must leave the handler with
    /// its current owner rather than fail on finding it taken.
    pub register_protocol: bool,
    /// Whether this install owns the desktop and Start Menu launchers.
    pub register_shortcuts: bool,
}

pub struct InstallOptions {
    pub manifest_url: Url,
    pub signature_url: Url,
    pub platform: Platform,
    pub install_root: PathBuf,
    pub trust: TrustKey,
    pub run_setup: bool,
    pub integrations: InstallIntegrationOptions,
    /// `None` leaves superseded editor and Forge processes running. A policy
    /// retires them and controls whether a stuck Forge may be ended.
    pub retirement: Option<RetirementPolicy>,
}

#[allow(clippy::too_many_lines)]
pub async fn install(options: InstallOptions) -> Result<()> {
    let root_lock = InstallerLock::acquire(&options.install_root, RootMode::Create)?;
    root_lock.fence()?;
    recover_activation_pointer_swap(&root_lock)?;
    // Plain HTTP is permitted only from this machine's own loopback, which
    // cannot be intercepted off-host. A locally built, locally signed release
    // is installed by serving its output directory on 127.0.0.1; every remote
    // manifest still requires TLS, and the signature check applies to both.
    let loopback_manifest = options.manifest_url.host().is_some_and(|host| match host {
        url::Host::Ipv4(address) => address.is_loopback(),
        url::Host::Ipv6(address) => address.is_loopback(),
        url::Host::Domain(domain) => domain == "localhost",
    });
    let client = reqwest::Client::builder()
        .https_only(!loopback_manifest)
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(InstallerError::ManifestRequest)?;
    let artifact_base_url = options
        .manifest_url
        .join("./")
        .map_err(|error| InstallerError::InvalidTrustKey(error.to_string()))?;
    let manifest = fetch(
        &client,
        options.manifest_url.clone(),
        options.signature_url.clone(),
        &options.trust,
    )
    .await?;
    let current_version = semver::Version::parse(env!("CARGO_PKG_VERSION"))
        .map_err(|error| InstallerError::InvalidTrustKey(error.to_string()))?;
    let minimum_version = semver::Version::parse(&manifest.minimum_installer_version)
        .map_err(|error| InstallerError::InvalidTrustKey(error.to_string()))?;
    if current_version < minimum_version {
        return Err(InstallerError::InstallerTooOld {
            current: current_version.to_string(),
            minimum: minimum_version.to_string(),
        });
    }
    let product_version = semver::Version::parse(&manifest.product_version)
        .map_err(|error| InstallerError::InvalidRelease(error.to_string()))?;
    let compatibility_version =
        semver::Version::parse(&manifest.editor_forge_compatibility_version)
            .map_err(|error| InstallerError::InvalidRelease(error.to_string()))?;
    let minimum_cli_version = semver::Version::parse(&manifest.minimum_cli_version)
        .map_err(|error| InstallerError::InvalidRelease(error.to_string()))?;
    if product_version != compatibility_version || product_version < minimum_cli_version {
        return Err(InstallerError::InvalidRelease(
            "product, Editor/Forge compatibility, and minimum CLI versions disagree".to_owned(),
        ));
    }
    let versions = options.install_root.join("versions");
    ensure_owned_directory(&versions)?;
    let existing_release = versions.join(&manifest.product_version);
    let existing_release = match std::fs::symlink_metadata(&existing_release) {
        Ok(metadata) if ordinary_metadata(&metadata, EntryKind::Directory) => {
            ordinary_path_identity(&existing_release, EntryKind::Directory)
                .map_err(|()| InstallerError::UnsafeOwnedPath)?;
            Some(existing_release)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Ok(_) | Err(_) => return Err(InstallerError::UnsafeOwnedPath),
    };
    if let Some(existing_release) = existing_release {
        let lifecycle_ae = release_cli(&existing_release)?;
        let existing_protocol = read_existing_protocol(&options.install_root)?;
        let bootstrap = versioned_installer_path(&existing_release);
        if !ordinary_file_exists(&bootstrap)? {
            return Err(InstallerError::MissingInstaller(bootstrap));
        }
        root_lock.fence()?;
        let retirement = retire_for(&options, &existing_release, &lifecycle_ae)?;
        let stable_ae = install_stable_cli(&root_lock, &options.install_root, &existing_release)?;
        let protocol = if options.integrations.register_protocol {
            prepare_protocol(&options.platform, &stable_ae, existing_protocol.as_ref())?
        } else {
            None
        };
        let launchers = planned_shortcuts(&options, &stable_ae, &existing_release);
        root_lock.fence()?;
        let activation_launchers = shortcut_records(&launchers)?;
        activate(
            &root_lock,
            &options.install_root,
            &existing_release,
            &manifest,
            &options,
            &ActivationIntegrations {
                stable_ae: &stable_ae,
                protocol: protocol.as_ref(),
                launchers: &activation_launchers,
            },
        )?;
        root_lock.fence()?;
        if options.integrations.register_protocol {
            apply_protocol(&options.platform, &stable_ae, existing_protocol.as_ref())?;
        }
        root_lock.fence()?;
        shortcuts::apply(&launchers)?;
        root_lock.fence()?;
        restore_retired_forge(&options, &existing_release, retirement)?;
        root_lock.fence()?;
        invoke_ae(&existing_release, &["--version"])?;
        return Ok(());
    }
    let stage = options.install_root.join(format!(
        ".stage-{}-{}",
        manifest.product_version,
        std::process::id()
    ));
    root_lock.fence()?;
    let mut stage_lease = StageLease::acquire(stage.clone(), &manifest.product_version)?;

    let result = async {
        let artifact = manifest
            .artifacts
            .iter()
            .find(|artifact| {
                artifact.platform == options.platform.os
                    && artifact.architecture == options.platform.arch
                    && (options.platform.os != "linux"
                        || artifact.libc.as_deref() == Some(platform_libc()))
            })
            .ok_or_else(|| InstallerError::MissingArtifact {
                component: NATIVE_PAYLOAD_LABEL.to_owned(),
                target: options.platform.target(),
            })?;
        let artifact_url = artifact_base_url
            .join(&artifact.file_name)
            .map_err(|error| InstallerError::InvalidTrustKey(error.to_string()))?;
        install_artifact(&client, artifact, artifact_url, &stage).await?;
        // The tree is final: record per-file digests so `ae doctor` can
        // detect payload drift after activation.
        crate::payload::write_manifest(&stage)?;

        root_lock.fence()?;
        let release = versions.join(&manifest.product_version);
        match std::fs::symlink_metadata(&release) {
            Ok(_) => {
                return Err(InstallerError::ExistingRelease(
                    manifest.product_version.clone(),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(InstallerError::UnsafeOwnedPath),
        }
        stage_lease.transfer_to(&release)?;
        let lifecycle_ae = release_cli(&release)?;
        let existing_protocol = read_existing_protocol(&options.install_root)?;
        let bootstrap = versioned_installer_path(&release);
        if !ordinary_file_exists(&bootstrap)? {
            return Err(InstallerError::MissingInstaller(bootstrap));
        }
        root_lock.fence()?;
        let retirement = retire_for(&options, &release, &lifecycle_ae)?;
        let stable_ae = install_stable_cli(&root_lock, &options.install_root, &release)?;
        let protocol = if options.integrations.register_protocol {
            prepare_protocol(&options.platform, &stable_ae, existing_protocol.as_ref())?
        } else {
            None
        };
        let launchers = planned_shortcuts(&options, &stable_ae, &release);
        root_lock.fence()?;
        let activation_launchers = shortcut_records(&launchers)?;
        activate(
            &root_lock,
            &options.install_root,
            &release,
            &manifest,
            &options,
            &ActivationIntegrations {
                stable_ae: &stable_ae,
                protocol: protocol.as_ref(),
                launchers: &activation_launchers,
            },
        )?;
        root_lock.fence()?;
        if options.integrations.register_protocol {
            apply_protocol(&options.platform, &stable_ae, existing_protocol.as_ref())?;
        }
        root_lock.fence()?;
        shortcuts::apply(&launchers)?;
        if options.run_setup {
            root_lock.fence()?;
            run_setup_sequence(&release)?;
        } else {
            root_lock.fence()?;
            restore_retired_forge(&options, &release, retirement)?;
        }
        Ok(())
    }
    .await;
    complete_install_locked(&root_lock, &mut stage_lease, result)
}

async fn install_artifact(
    client: &reqwest::Client,
    artifact: &Artifact,
    artifact_url: Url,
    stage: &Path,
) -> Result<()> {
    if artifact.size == 0 || artifact.size > ABSOLUTE_ARTIFACT_LIMIT {
        return Err(InstallerError::ArtifactTooLarge {
            url: artifact_url.clone(),
        });
    }
    let response = client
        .get(artifact_url.clone())
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|source| InstallerError::ArtifactRequest {
            url: artifact_url.clone(),
            source,
        })?;
    if response
        .content_length()
        .is_some_and(|size| size > artifact.size)
    {
        return Err(InstallerError::ArtifactTooLarge {
            url: artifact_url.clone(),
        });
    }
    let download = stage.join(format!(".{}.download", artifact.id));
    let mut file = create_owned_file(&download)?;
    let mut response = response;
    let mut downloaded = 0_u64;
    let mut hasher = Sha256::new();
    while let Some(chunk) =
        response
            .chunk()
            .await
            .map_err(|source| InstallerError::ArtifactRequest {
                url: artifact_url.clone(),
                source,
            })?
    {
        downloaded = downloaded.saturating_add(chunk.len() as u64);
        if downloaded > artifact.size {
            return Err(InstallerError::ArtifactTooLarge {
                url: artifact_url.clone(),
            });
        }
        hasher.update(&chunk);
        file.write_all(&chunk).map_err(io(&download))?;
    }
    if downloaded != artifact.size {
        return Err(InstallerError::ArtifactSizeMismatch {
            expected: artifact.size,
            actual: downloaded,
        });
    }
    file.sync_all().map_err(io(&download))?;
    let digest = hex::encode(hasher.finalize());
    if !digest.eq_ignore_ascii_case(&artifact.sha256) {
        return Err(InstallerError::ChecksumMismatch(artifact_url));
    }
    archive::extract(&download, artifact.format, stage, &artifact.archive_entries)?;
    remove_owned_file(&download)?;
    Ok(())
}

pub(crate) fn platform_libc() -> &'static str {
    if cfg!(target_env = "musl") {
        "musl"
    } else {
        "glibc"
    }
}

#[derive(Debug, Serialize)]
struct Components {
    editor: bool,
    forge: bool,
}

fn installed_components() -> Components {
    Components {
        editor: true,
        forge: true,
    }
}

/// The launchers this run owns, or none when the caller opted out.
fn planned_shortcuts(
    options: &InstallOptions,
    stable_ae: &Path,
    release: &Path,
) -> Vec<shortcuts::ShortcutTarget> {
    if options.integrations.register_shortcuts {
        shortcuts::targets(&options.platform, stable_ae, release)
    } else {
        Vec::new()
    }
}

fn shortcut_records(targets: &[shortcuts::ShortcutTarget]) -> Result<Vec<OwnedIntegration>> {
    targets
        .iter()
        .map(shortcuts::ShortcutTarget::owned)
        .collect()
}

/// Closes the old editor and proves Forge can stop before activation changes
/// any durable pointer or integration. A busy Forge therefore cancels the
/// update while the prior installation remains authoritative.
fn retire_for(options: &InstallOptions, release: &Path, stable_ae: &Path) -> Result<Retirement> {
    let Some(policy) = options.retirement else {
        return Ok(Retirement::default());
    };

    let retirement = retire_superseded(&options.install_root, release, stable_ae, policy)?;
    if !retirement.is_empty() {
        println!(
            "retired superseded instances: {} editor, {} forge",
            retirement.editors_closed, retirement.forges_stopped
        );
    }
    Ok(retirement)
}

/// A maintenance update has no editor launch after the installer returns, so
/// preserve a Forge that was running before the update by starting the newly
/// activated version. Setup-driven installs deliberately skip this: their
/// caller opens the editor, whose background handoff must own the Forge it
/// starts so window close can stop that exact process.
fn restore_retired_forge(
    options: &InstallOptions,
    release: &Path,
    retirement: Retirement,
) -> Result<()> {
    if should_restore_retired_forge(options.run_setup, retirement) {
        invoke_ae(release, &["start"])?;
    }
    Ok(())
}

fn run_setup_sequence(release: &Path) -> Result<()> {
    for arguments in FIRST_RUN_CONFIGURATION_COMMANDS {
        invoke_ae(release, arguments)?;
    }
    Ok(())
}

fn should_restore_retired_forge(run_setup: bool, retirement: Retirement) -> bool {
    !run_setup && retirement.forges_stopped > 0
}

struct ActivationIntegrations<'a> {
    stable_ae: &'a Path,
    protocol: Option<&'a OwnedIntegration>,
    launchers: &'a [OwnedIntegration],
}

fn activate(
    lock: &InstallerLock,
    root: &Path,
    release: &Path,
    manifest: &crate::manifest::ReleaseManifest,
    options: &InstallOptions,
    integrations: &ActivationIntegrations<'_>,
) -> Result<()> {
    lock.fence()?;
    let next = root.join(".installation.json.tmp");
    let current = root.join("installation.json");
    let now = Utc::now().to_rfc3339();
    let mut integration_records = serde_json::Map::from_iter([(
        "ae_path".to_owned(),
        serde_json::to_value(OwnedIntegration {
            path: integrations.stable_ae.display().to_string(),
            fingerprint: hash_file(&release.join("bin").join(if cfg!(windows) {
                "ae.exe"
            } else {
                "ae"
            }))?,
        })
        .map_err(InstallerError::InvalidPayload)?,
    )]);
    if let Some(protocol) = integrations.protocol {
        integration_records.insert(
            "protocol".to_owned(),
            serde_json::to_value(protocol).map_err(InstallerError::InvalidPayload)?,
        );
    }
    if !integrations.launchers.is_empty() {
        integration_records.insert(
            "shortcuts".to_owned(),
            serde_json::to_value(integrations.launchers).map_err(InstallerError::InvalidPayload)?,
        );
    }
    let contents = serde_json::json!({
        "format_version": 1,
        "install_root": root,
        "platform": options.platform.os,
        "architecture": options.platform.arch,
        "channel": manifest.channel.as_str(),
        "components": installed_components(),
        "integrations": integration_records,
        "installed_at": now,
        "updated_at": now,
        "activation_state": "active",
        "finalization_state": "complete",
        "active_version": manifest.product_version.as_str(),
        "permanent_ae_path": integrations.stable_ae,
        "artifact": {
            "artifact_id": manifest.artifacts.iter()
                .find(|artifact| artifact.platform == options.platform.os
                    && artifact.architecture == options.platform.arch)
                .map_or("unknown", |artifact| artifact.id.as_str()),
            "sha256": manifest.artifacts.iter()
                .find(|artifact| artifact.platform == options.platform.os
                    && artifact.architecture == options.platform.arch)
                .map_or("", |artifact| artifact.sha256.as_str()),
            "signing_key_id": manifest.signing_identity.key_id.as_str(),
        },
        "transaction": { "state": "idle" }
    });
    let mut file = create_owned_file(&next)?;
    serde_json::to_writer(&mut file, &contents)
        .map_err(|error| InstallerError::Archive(error.to_string()))?;
    let next_identity = sync_owned_file(&next, &file)?;
    drop(file);
    require_identity(&next, EntryKind::File, next_identity)?;
    let previous = root.join(".installation.json.previous");
    lock.fence()?;
    remove_owned_file(&previous)?;
    if let Some(current_identity) = owned_file_identity(&current)? {
        require_identity(&current, EntryKind::File, current_identity)?;
        std::fs::rename(&current, &previous).map_err(io(&current))?;
    }
    match std::fs::symlink_metadata(&current) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) | Err(_) => return Err(InstallerError::UnsafeOwnedPath),
    }
    if let Err(source) = std::fs::rename(&next, &current) {
        if let Some(previous_identity) = owned_file_identity(&previous)? {
            require_identity(&previous, EntryKind::File, previous_identity)?;
            let _ = std::fs::rename(&previous, &current);
        }
        return Err(io(&current)(source));
    }
    remove_owned_file(&previous)?;
    Ok(())
}

fn install_stable_cli(lock: &InstallerLock, root: &Path, release: &Path) -> Result<PathBuf> {
    lock.fence()?;
    let source = release_cli(release)?;
    let bin = root.join("bin");
    ensure_owned_directory(&bin)?;
    let stable = bin.join(if cfg!(windows) { "ae.exe" } else { "ae" });
    let temporary = bin.join(".ae.next");
    let temporary_identity = copy_owned_file(&source, &temporary)?;
    match std::fs::symlink_metadata(&stable) {
        Ok(metadata) => {
            if !ordinary_metadata(&metadata, EntryKind::File) {
                return Err(InstallerError::UnsafeOwnedPath);
            }
            let stable_identity = ordinary_path_identity(&stable, EntryKind::File)
                .map_err(|()| InstallerError::UnsafeOwnedPath)?;
            lock.fence()?;
            require_identity(&stable, EntryKind::File, stable_identity)?;
            if hash_file(&stable)? == hash_file(&source)? {
                require_identity(&stable, EntryKind::File, stable_identity)?;
                require_identity(&temporary, EntryKind::File, temporary_identity)?;
                remove_owned_file(&temporary)?;
                lock.fence()?;
                integrate_path(&bin)?;
                return Ok(stable);
            }
            require_identity(&stable, EntryKind::File, stable_identity)?;
            if let Err(remove_error) = std::fs::remove_file(&stable) {
                #[cfg(windows)]
                {
                    let _ = remove_error;
                    schedule_stable_cli_replacement(
                        lock,
                        &temporary,
                        &stable,
                        temporary_identity,
                        stable_identity,
                    )?;
                    lock.fence()?;
                    integrate_path(&bin)?;
                    return Ok(stable);
                }
                #[cfg(not(windows))]
                return Err(io(&stable)(remove_error));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(InstallerError::UnsafeOwnedPath),
    }
    lock.fence()?;
    require_identity(&temporary, EntryKind::File, temporary_identity)?;
    match std::fs::symlink_metadata(&stable) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) | Err(_) => return Err(InstallerError::UnsafeOwnedPath),
    }
    std::fs::rename(&temporary, &stable).map_err(io(&stable))?;
    lock.fence()?;
    integrate_path(&bin)?;
    Ok(stable)
}

fn release_cli(release: &Path) -> Result<PathBuf> {
    let bin = release.join("bin");
    let executable = release
        .join("bin")
        .join(if cfg!(windows) { "ae.exe" } else { "ae" });
    if !ordinary_directory_exists(&bin)? || !ordinary_file_exists(&executable)? {
        return Err(InstallerError::MissingCli(executable));
    }
    Ok(executable)
}

fn versioned_installer_path(release: &Path) -> PathBuf {
    release.join("bin").join(if cfg!(windows) {
        "installer.exe"
    } else {
        "installer"
    })
}

#[cfg(windows)]
fn schedule_stable_cli_replacement(
    lock: &InstallerLock,
    source: &Path,
    destination: &Path,
    source_identity: FileIdentity,
    destination_identity: FileIdentity,
) -> Result<()> {
    lock.fence()?;
    let marker = PendingMarker::create(lock, PendingMarkerKind::AeReplacement)?;
    lock.fence()?;
    require_identity(source, EntryKind::File, source_identity)?;
    match std::fs::symlink_metadata(destination) {
        Ok(metadata) if ordinary_metadata(&metadata, EntryKind::File) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) | Err(_) => return Err(InstallerError::UnsafeOwnedPath),
    }
    require_identity(destination, EntryKind::File, destination_identity)?;
    lock.fence()?;
    require_identity(source, EntryKind::File, source_identity)?;
    require_identity(destination, EntryKind::File, destination_identity)?;
    detached_background_command("cmd.exe")
        .args([
            "/d",
            "/s",
            "/c",
            "ping 127.0.0.1 -n 3 > nul & move /y \"%ARTISAN_AE_SOURCE%\" \"%ARTISAN_AE_DESTINATION%\" > nul && rmdir \"%ARTISAN_AE_MARKER%\" > nul",
        ])
        .env("ARTISAN_AE_SOURCE", source)
        .env("ARTISAN_AE_DESTINATION", destination)
        .env("ARTISAN_AE_MARKER", &marker.path)
        .spawn()
        .map_err(|_| InstallerError::LifecycleHelper)?;
    Ok(())
}

#[cfg(windows)]
fn integrate_path(bin: &Path) -> Result<()> {
    use winreg::{RegKey, enums::HKEY_CURRENT_USER};
    if cfg!(debug_assertions) {
        eprintln!(
            "development build guard: leaving the user PATH untouched instead of registering {}",
            bin.display()
        );
        return Ok(());
    }
    let environment = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(
            "Environment",
            winreg::enums::KEY_READ | winreg::enums::KEY_WRITE,
        )
        .map_err(io("HKCU\\Environment"))?;
    let current: String = environment.get_value("Path").unwrap_or_default();
    let candidate = bin.display().to_string();
    let next = prepend_windows_path_entry(&current, &candidate);
    if next != current {
        environment
            .set_value("Path", &next)
            .map_err(io("HKCU\\Environment\\Path"))?;
    }
    Ok(())
}

#[cfg(windows)]
fn prepend_windows_path_entry(current: &str, candidate: &str) -> String {
    std::iter::once(candidate)
        .chain(
            current
                .split(';')
                .filter(|entry| !entry.is_empty() && !entry.eq_ignore_ascii_case(candidate)),
        )
        .collect::<Vec<_>>()
        .join(";")
}

#[cfg(unix)]
fn integrate_path(bin: &Path) -> Result<()> {
    use std::os::unix::fs::symlink;
    if cfg!(debug_assertions) {
        eprintln!(
            "development build guard: leaving ~/.local/bin untouched instead of linking {}",
            bin.display()
        );
        return Ok(());
    }
    let home = std::env::var_os("HOME").ok_or(InstallerError::MissingHome)?;
    let command_bin = PathBuf::from(home).join(".local").join("bin");
    std::fs::create_dir_all(&command_bin).map_err(io(&command_bin))?;
    let link = command_bin.join("ae");
    let target = bin.join("ae");
    if link.symlink_metadata().is_ok() {
        if std::fs::read_link(&link).ok().as_deref() == Some(target.as_path()) {
            return Ok(());
        }
        return Err(InstallerError::InvalidInstallation(format!(
            "refusing to replace existing command at {}",
            link.display()
        )));
    }
    symlink(target, &link).map_err(io(&link))
}

pub(crate) fn hash_file(path: &Path) -> Result<String> {
    let mut file = open_for_read(path, EntryKind::File).map_err(io(path))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(io(path))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

#[derive(Debug, Deserialize)]
struct InstalledState {
    activation_state: String,
    active_version: String,
    install_root: PathBuf,
    permanent_ae_path: PathBuf,
    #[serde(default)]
    integrations: InstalledIntegrations,
}

#[derive(Debug, Default, Deserialize)]
struct InstalledIntegrations {
    ae_path: Option<OwnedIntegration>,
    protocol: Option<OwnedIntegration>,
    /// Absent in installations predating installer-owned launchers, which is
    /// why removal and repair both treat an empty record as "not ours".
    #[serde(default)]
    shortcuts: Vec<OwnedIntegration>,
}

pub fn repair(root: &Path) -> Result<()> {
    let root_lock = InstallerLock::acquire(root, RootMode::Existing)?;
    root_lock.fence()?;
    recover_activation_pointer_swap(&root_lock)?;
    let state = read_installed_state(root)?;
    validate_state_root(root, &state)?;
    let release = root.join("versions").join(&state.active_version);
    if !ordinary_directory_exists(&root.join("versions"))? {
        return Err(InstallerError::InvalidInstallation(
            "the installation versions directory is missing or unsafe".to_owned(),
        ));
    }
    let bootstrap = versioned_installer_path(&release);
    if !ordinary_directory_exists(&release)? {
        return Err(InstallerError::MissingInstaller(bootstrap));
    }
    if !ordinary_file_exists(&bootstrap)? {
        return Err(InstallerError::MissingInstaller(bootstrap));
    }
    let stable = install_stable_cli(&root_lock, root, &release)?;
    if stable != state.permanent_ae_path {
        return Err(InstallerError::InvalidInstallation(
            "permanent ae path is outside the bootstrap-owned layout".to_owned(),
        ));
    }
    let platform = Platform::detect()?;
    let existing_protocol = state.integrations.protocol.as_ref();
    // An installation with no recorded protocol ownership was installed with
    // `--skip-protocol` beside a primary installation. Repairing it must not
    // adopt the handler the primary owns — finding it registered elsewhere is
    // this installation's healthy state, not damage to fix.
    if existing_protocol.is_some() {
        root_lock.fence()?;
        let protocol = prepare_protocol(&platform, &stable, existing_protocol)?;
        if protocol.as_ref() != state.integrations.protocol.as_ref()
            && let Some(protocol) = protocol.as_ref()
        {
            root_lock.fence()?;
            persist_protocol_record(root, protocol)?;
        }
        root_lock.fence()?;
        apply_protocol(&platform, &stable, existing_protocol)?;
    }
    // Launchers are rewritten rather than merely checked: their icon is taken
    // from the versioned editor executable, so every update leaves the
    // previous release's path behind in an otherwise healthy shortcut.
    let launchers = shortcuts::targets(&platform, &stable, &release);
    if !state.integrations.shortcuts.is_empty() || !launchers.is_empty() {
        root_lock.fence()?;
        let records = shortcut_records(&launchers)?;
        root_lock.fence()?;
        shortcuts::apply(&launchers)?;
        root_lock.fence()?;
        persist_shortcut_records(root, &records)?;
    }
    root_lock.fence()?;
    invoke_ae_diagnostic(&release, &["doctor"])
}

pub fn diagnose(root: &Path) -> Result<()> {
    let root_lock = InstallerLock::acquire(root, RootMode::Existing)?;
    root_lock.fence()?;
    recover_activation_pointer_swap(&root_lock)?;
    let state = read_installed_state(root)?;
    validate_state_root(root, &state)?;
    let stable = root
        .join("bin")
        .join(if cfg!(windows) { "ae.exe" } else { "ae" });
    if !ordinary_directory_exists(&root.join("bin"))?
        || stable != state.permanent_ae_path
        || !ordinary_file_exists(&stable)?
    {
        return Err(InstallerError::InvalidInstallation(
            "permanent ae path is missing or outside the bootstrap-owned layout".to_owned(),
        ));
    }
    root_lock.fence()?;
    verify_protocol(
        &Platform::detect()?,
        &stable,
        state.integrations.protocol.as_ref(),
    )
}

fn invoke_ae_diagnostic(release: &Path, arguments: &[&str]) -> Result<()> {
    let executable = release
        .join("bin")
        .join(if cfg!(windows) { "ae.exe" } else { "ae" });
    if !ordinary_file_exists(&executable)? {
        return Err(InstallerError::MissingCli(executable));
    }
    // Doctor reports Forge-instance problems independently. Repair owns the
    // installation invariants above and must not recurse through `--fix`.
    let _status = background_command(&executable)
        .args(arguments)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(io(&executable))?;
    Ok(())
}

pub fn uninstall(root: &Path, remove_data: bool) -> Result<()> {
    let root_lock = InstallerLock::acquire(root, RootMode::Existing)?;
    root_lock.fence()?;
    recover_activation_pointer_swap(&root_lock)?;
    let state = read_installed_state(root)?;
    validate_state_root(root, &state)?;
    let stable = root
        .join("bin")
        .join(if cfg!(windows) { "ae.exe" } else { "ae" });
    root_lock.fence()?;
    remove_protocol(
        &Platform::detect()?,
        &state.permanent_ae_path,
        state.integrations.protocol.as_ref(),
    )?;
    root_lock.fence()?;
    shortcuts::remove(
        &shortcuts::targets(
            &Platform::detect()?,
            &state.permanent_ae_path,
            &root.join("versions").join(&state.active_version),
        ),
        &state.integrations.shortcuts,
    )?;
    if let Some(integration) = state.integrations.ae_path {
        let path = Path::new(&integration.path);
        if path == stable
            && let Ok(metadata) = std::fs::symlink_metadata(path)
            && ordinary_metadata(&metadata, EntryKind::File)
            && hash_file(path)? == integration.fingerprint
        {
            root_lock.fence()?;
            std::fs::remove_file(path).map_err(io(path))?;
        }
    }
    root_lock.fence()?;
    remove_path_integration(&root.join("bin"))?;
    root_lock.fence()?;
    remove_path_in_root(root, &root.join("bin"))?;
    root_lock.fence()?;
    remove_path_in_root(root, &root.join("installation.json"))?;
    if remove_data {
        // The home hosts one Forge instance at its root; legacy `profiles/`
        // trees predate the single-instance layout and are removed alongside.
        for name in [
            "config.json",
            "secrets.json",
            "state.json",
            "forge.log",
            "data",
            "profiles",
        ] {
            root_lock.fence()?;
            remove_path_in_root(root, &root.join(name))?;
        }
    }
    root_lock.fence()?;
    schedule_installation_cleanup(&root_lock, root)
}

pub fn prepare_update(root: &Path, retirement: Option<RetirementPolicy>) -> Result<()> {
    let root_lock = InstallerLock::acquire(root, RootMode::Existing)?;
    root_lock.fence()?;
    recover_activation_pointer_swap(&root_lock)?;
    let Some(retirement_policy) = retirement else {
        return Ok(());
    };
    let lifecycle_ae = root
        .join("bin")
        .join(if cfg!(windows) { "ae.exe" } else { "ae" });
    if !ordinary_directory_exists(&root.join("bin"))? {
        return Err(InstallerError::MissingCli(lifecycle_ae));
    }
    if !ordinary_file_exists(&lifecycle_ae)? {
        return Err(InstallerError::MissingCli(lifecycle_ae));
    }
    let incoming_release = root.join(".incoming-release");
    if !ordinary_directory_exists(&root.join("versions"))? {
        return Err(InstallerError::InvalidInstallation(
            "the installation versions directory is missing or unsafe".to_owned(),
        ));
    }
    root_lock.fence()?;
    let retirement = retire_superseded(root, &incoming_release, &lifecycle_ae, retirement_policy)?;
    if !retirement.is_empty() {
        println!(
            "prepared update: closed {} editor, stopped {} forge",
            retirement.editors_closed, retirement.forges_stopped
        );
    }
    Ok(())
}

#[cfg(windows)]
fn remove_path_integration(bin: &Path) -> Result<()> {
    use winreg::{RegKey, enums::HKEY_CURRENT_USER};
    if cfg!(debug_assertions) {
        eprintln!(
            "development build guard: leaving the user PATH untouched instead of removing {}",
            bin.display()
        );
        return Ok(());
    }
    let environment = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(
            "Environment",
            winreg::enums::KEY_READ | winreg::enums::KEY_WRITE,
        )
        .map_err(io("HKCU\\Environment"))?;
    let current: String = environment.get_value("Path").unwrap_or_default();
    let candidate = bin.display().to_string();
    let next = current
        .split(';')
        .filter(|entry| !entry.eq_ignore_ascii_case(&candidate))
        .collect::<Vec<_>>()
        .join(";");
    if next != current {
        environment
            .set_value("Path", &next)
            .map_err(io("HKCU\\Environment\\Path"))?;
    }
    Ok(())
}

#[cfg(unix)]
fn remove_path_integration(bin: &Path) -> Result<()> {
    if cfg!(debug_assertions) {
        eprintln!(
            "development build guard: leaving ~/.local/bin untouched instead of unlinking {}",
            bin.display()
        );
        return Ok(());
    }
    let home = std::env::var_os("HOME").ok_or(InstallerError::MissingHome)?;
    let link = PathBuf::from(home).join(".local").join("bin").join("ae");
    if link.symlink_metadata().is_ok()
        && std::fs::read_link(&link).ok().as_deref() == Some(bin.join("ae").as_path())
    {
        std::fs::remove_file(&link).map_err(io(&link))?;
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct ActivationPointerState {
    activation_state: String,
    finalization_state: String,
    active_version: String,
    install_root: PathBuf,
    permanent_ae_path: PathBuf,
}

struct ValidatedActivationPointer {
    path: PathBuf,
    identity: FileIdentity,
}

fn activation_pointer_paths(root: &Path) -> [PathBuf; 3] {
    [
        root.join("installation.json"),
        root.join(".installation.json.tmp"),
        root.join(".installation.json.previous"),
    ]
}

fn ambiguous_activation_error() -> InstallerError {
    InstallerError::InstallationActivationTransactionAmbiguous
}

/// Recover only the three files that participate in the activation pointer
/// swap. Every file is inspected before any residue is removed or restored so
/// an unsafe transaction remains available for a later, informed retry.
fn recover_activation_pointer_swap(lock: &InstallerLock) -> Result<()> {
    lock.fence().map_err(|_| ambiguous_activation_error())?;
    let [current_path, temporary_path, previous_path] = activation_pointer_paths(&lock.root);
    let current = inspect_activation_pointer(&lock.root, &current_path)?;
    let temporary = inspect_activation_pointer(&lock.root, &temporary_path)?;
    let previous = inspect_activation_pointer(&lock.root, &previous_path)?;

    lock.fence().map_err(|_| ambiguous_activation_error())?;
    if let Some(current) = current.as_ref() {
        if let Some(temporary) = temporary.as_ref() {
            revalidate_activation_pointer(Some(current), &current_path)?;
            revalidate_activation_pointer(Some(temporary), &temporary_path)?;
            revalidate_activation_pointer(previous.as_ref(), &previous_path)?;
            remove_validated_activation_pointer(temporary)?;
        }
        if let Some(previous) = previous.as_ref() {
            revalidate_activation_pointer(Some(current), &current_path)?;
            revalidate_activation_pointer(Some(previous), &previous_path)?;
            remove_validated_activation_pointer(previous)?;
        }
        return Ok(());
    }

    if let Some(previous) = previous.as_ref() {
        revalidate_activation_pointer(None, &current_path)?;
        revalidate_activation_pointer(Some(previous), &previous_path)?;
        revalidate_activation_pointer(temporary.as_ref(), &temporary_path)?;
        std::fs::rename(&previous.path, &current_path).map_err(|_| ambiguous_activation_error())?;
        require_identity(&current_path, EntryKind::File, previous.identity)
            .map_err(|_| ambiguous_activation_error())?;
        if let Some(temporary) = temporary.as_ref() {
            revalidate_activation_pointer(Some(temporary), &temporary_path)?;
            require_identity(&current_path, EntryKind::File, previous.identity)
                .map_err(|_| ambiguous_activation_error())?;
            remove_validated_activation_pointer(temporary)?;
        }
        return Ok(());
    }

    if let Some(temporary) = temporary.as_ref() {
        revalidate_activation_pointer(None, &current_path)?;
        revalidate_activation_pointer(Some(temporary), &temporary_path)?;
        revalidate_activation_pointer(None, &previous_path)?;
        remove_validated_activation_pointer(temporary)?;
    }
    Ok(())
}

fn inspect_activation_pointer(
    root: &Path,
    path: &Path,
) -> Result<Option<ValidatedActivationPointer>> {
    let Some(identity) = owned_file_identity(path).map_err(|_| ambiguous_activation_error())?
    else {
        return Ok(None);
    };
    let mut file =
        open_for_read(path, EntryKind::File).map_err(|_| ambiguous_activation_error())?;
    if identity_from_file(&file, EntryKind::File).map_err(|()| ambiguous_activation_error())?
        != identity
    {
        return Err(ambiguous_activation_error());
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|_| ambiguous_activation_error())?;
    if identity_from_file(&file, EntryKind::File).map_err(|()| ambiguous_activation_error())?
        != identity
    {
        return Err(ambiguous_activation_error());
    }
    require_identity(path, EntryKind::File, identity).map_err(|_| ambiguous_activation_error())?;
    let state: ActivationPointerState =
        serde_json::from_slice(&bytes).map_err(|_| ambiguous_activation_error())?;
    validate_activation_pointer_state(root, &state)?;
    require_identity(path, EntryKind::File, identity).map_err(|_| ambiguous_activation_error())?;
    Ok(Some(ValidatedActivationPointer {
        path: path.to_path_buf(),
        identity,
    }))
}

fn validate_activation_pointer_state(root: &Path, state: &ActivationPointerState) -> Result<()> {
    let stable = root
        .join("bin")
        .join(if cfg!(windows) { "ae.exe" } else { "ae" });
    if state.activation_state != "active"
        || state.finalization_state != "complete"
        || state.install_root.as_path() != root
        || state.permanent_ae_path.as_path() != stable.as_path()
        || !is_safe_release_component(&state.active_version)
    {
        return Err(ambiguous_activation_error());
    }
    Ok(())
}

fn revalidate_activation_pointer(
    pointer: Option<&ValidatedActivationPointer>,
    path: &Path,
) -> Result<()> {
    match pointer {
        Some(pointer) => {
            let identity = owned_file_identity(path).map_err(|_| ambiguous_activation_error())?;
            if identity != Some(pointer.identity) {
                return Err(ambiguous_activation_error());
            }
        }
        None => match std::fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) | Err(_) => return Err(ambiguous_activation_error()),
        },
    }
    Ok(())
}

fn remove_validated_activation_pointer(pointer: &ValidatedActivationPointer) -> Result<()> {
    let identity = owned_file_identity(&pointer.path).map_err(|_| ambiguous_activation_error())?;
    if identity != Some(pointer.identity) {
        return Err(ambiguous_activation_error());
    }
    remove_owned_file(&pointer.path).map_err(|_| ambiguous_activation_error())
}

fn read_installed_state(root: &Path) -> Result<InstalledState> {
    let path = root.join("installation.json");
    ordinary_file_exists(&path)?;
    let bytes = std::fs::read(&path).map_err(io(&path))?;
    serde_json::from_slice(&bytes).map_err(InstallerError::InvalidPayload)
}

fn read_existing_protocol(root: &Path) -> Result<Option<OwnedIntegration>> {
    let path = root.join("installation.json");
    if !ordinary_file_exists(&path)? {
        return Ok(None);
    }
    read_installed_state(root).map(|state| state.integrations.protocol)
}

fn persist_protocol_record(root: &Path, protocol: &OwnedIntegration) -> Result<()> {
    let current = root.join("installation.json");
    let next = root.join(".installation.json.protocol");
    let bytes = std::fs::read(&current).map_err(io(&current))?;
    let mut document: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(InstallerError::InvalidPayload)?;
    let integrations = document
        .get_mut("integrations")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| {
            InstallerError::InvalidInstallation(
                "installation manifest integrations are missing".to_owned(),
            )
        })?;
    integrations.insert(
        "protocol".to_owned(),
        serde_json::to_value(protocol).map_err(InstallerError::InvalidPayload)?,
    );
    let mut file = create_owned_file(&next)?;
    serde_json::to_writer(&mut file, &document).map_err(InstallerError::InvalidPayload)?;
    let next_identity = sync_owned_file(&next, &file)?;
    drop(file);
    require_identity(&next, EntryKind::File, next_identity)?;

    let previous = root.join(".installation.json.protocol.previous");
    remove_owned_file(&previous)?;
    let current_identity = owned_file_identity(&current)?.ok_or(InstallerError::UnsafeOwnedPath)?;
    require_identity(&current, EntryKind::File, current_identity)?;
    std::fs::rename(&current, &previous).map_err(io(&current))?;
    if let Err(source) = std::fs::rename(&next, &current) {
        if let Some(previous_identity) = owned_file_identity(&previous)? {
            require_identity(&previous, EntryKind::File, previous_identity)?;
            let _ = std::fs::rename(&previous, &current);
        }
        return Err(io(&current)(source));
    }
    remove_owned_file(&previous)
}

/// Records the launchers a repair just rewrote, through the same
/// write-and-swap the protocol record uses so an interrupted repair never
/// leaves the manifest half-written.
fn persist_shortcut_records(root: &Path, launchers: &[OwnedIntegration]) -> Result<()> {
    let current = root.join("installation.json");
    let next = root.join(".installation.json.shortcuts");
    let bytes = std::fs::read(&current).map_err(io(&current))?;
    let mut document: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(InstallerError::InvalidPayload)?;
    let integrations = document
        .get_mut("integrations")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| {
            InstallerError::InvalidInstallation(
                "installation manifest integrations are missing".to_owned(),
            )
        })?;
    integrations.insert(
        "shortcuts".to_owned(),
        serde_json::to_value(launchers).map_err(InstallerError::InvalidPayload)?,
    );
    let mut file = create_owned_file(&next)?;
    serde_json::to_writer(&mut file, &document).map_err(InstallerError::InvalidPayload)?;
    let next_identity = sync_owned_file(&next, &file)?;
    drop(file);
    require_identity(&next, EntryKind::File, next_identity)?;

    let previous = root.join(".installation.json.shortcuts.previous");
    remove_owned_file(&previous)?;
    let current_identity = owned_file_identity(&current)?.ok_or(InstallerError::UnsafeOwnedPath)?;
    require_identity(&current, EntryKind::File, current_identity)?;
    std::fs::rename(&current, &previous).map_err(io(&current))?;
    if let Err(source) = std::fs::rename(&next, &current) {
        if let Some(previous_identity) = owned_file_identity(&previous)? {
            require_identity(&previous, EntryKind::File, previous_identity)?;
            let _ = std::fs::rename(&previous, &current);
        }
        return Err(io(&current)(source));
    }
    remove_owned_file(&previous)
}

fn validate_state_root(root: &Path, state: &InstalledState) -> Result<()> {
    let stable = root
        .join("bin")
        .join(if cfg!(windows) { "ae.exe" } else { "ae" });
    if state.activation_state != "active"
        || state.install_root != root
        || state.permanent_ae_path != stable
        || !is_safe_release_component(&state.active_version)
    {
        return Err(InstallerError::InvalidInstallation(
            "installation manifest does not own the requested root".to_owned(),
        ));
    }
    Ok(())
}

fn is_safe_release_component(value: &str) -> bool {
    if value.is_empty() || value.contains('\0') || value.contains(['/', '\\']) {
        return false;
    }
    let mut components = Path::new(value).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

fn remove_path_in_root(root: &Path, path: &Path) -> Result<()> {
    if !path.starts_with(root)
        || path == root
        || path.parent() != Some(root)
        || path.file_name() == Some(std::ffi::OsStr::new(INSTALLER_LOCK_NAME))
    {
        return Err(InstallerError::InvalidInstallation(
            "refusing removal outside the installation root".to_owned(),
        ));
    }
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(InstallerError::UnsafeOwnedPath),
    };
    if !ordinary_metadata(
        &metadata,
        if metadata.is_dir() {
            EntryKind::Directory
        } else {
            EntryKind::File
        },
    ) {
        return Err(InstallerError::UnsafeOwnedPath);
    }
    if metadata.is_dir() {
        let expected = ordinary_path_identity(path, EntryKind::Directory)
            .map_err(|()| InstallerError::UnsafeOwnedPath)?;
        validate_owned_tree(path)?;
        require_identity(path, EntryKind::Directory, expected)?;
        match std::fs::remove_dir_all(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(InstallerError::UnsafeOwnedPath),
        }
    } else {
        let expected = ordinary_path_identity(path, EntryKind::File)
            .map_err(|()| InstallerError::UnsafeOwnedPath)?;
        require_identity(path, EntryKind::File, expected)?;
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(InstallerError::UnsafeOwnedPath),
        }
    }
    Ok(())
}

fn validate_owned_tree(path: &Path) -> Result<()> {
    let metadata = std::fs::symlink_metadata(path).map_err(|_| InstallerError::UnsafeOwnedPath)?;
    if !ordinary_metadata(&metadata, EntryKind::Directory) {
        return Err(InstallerError::UnsafeOwnedPath);
    }
    let identity = ordinary_path_identity(path, EntryKind::Directory)
        .map_err(|()| InstallerError::UnsafeOwnedPath)?;
    for entry in std::fs::read_dir(path).map_err(|_| InstallerError::UnsafeOwnedPath)? {
        let entry = entry.map_err(|_| InstallerError::UnsafeOwnedPath)?;
        let child = entry.path();
        let metadata =
            std::fs::symlink_metadata(&child).map_err(|_| InstallerError::UnsafeOwnedPath)?;
        if ordinary_metadata(&metadata, EntryKind::Directory) {
            validate_owned_tree(&child)?;
        } else if !ordinary_metadata(&metadata, EntryKind::File) {
            return Err(InstallerError::UnsafeOwnedPath);
        } else {
            ordinary_path_identity(&child, EntryKind::File)
                .map_err(|()| InstallerError::UnsafeOwnedPath)?;
        }
    }
    require_identity(path, EntryKind::Directory, identity)?;
    Ok(())
}

fn schedule_installation_cleanup(lock: &InstallerLock, root: &Path) -> Result<()> {
    let versions = root.join("versions");
    match std::fs::symlink_metadata(&versions) {
        Ok(_) => validate_owned_tree(&versions)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(InstallerError::UnsafeOwnedPath),
    }
    lock.fence()?;
    let marker = PendingMarker::create(lock, PendingMarkerKind::Cleanup)?;
    match std::fs::symlink_metadata(&versions) {
        Ok(_) => validate_owned_tree(&versions)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(InstallerError::UnsafeOwnedPath),
    }
    lock.fence()?;
    #[cfg(windows)]
    {
        let mut command = detached_background_command("cmd.exe");
        let script = "ping 127.0.0.1 -n 3 > nul & if exist \"%ARTISAN_VERSIONS%\" (rmdir /s /q \"%ARTISAN_VERSIONS%\" > nul) & if not exist \"%ARTISAN_VERSIONS%\" (rmdir \"%ARTISAN_CLEANUP_MARKER%\" > nul)";
        command.args(["/d", "/s", "/c", script]);
        command.env("ARTISAN_VERSIONS", versions);
        command.env("ARTISAN_CLEANUP_MARKER", &marker.path);
        command
            .spawn()
            .map_err(|_| InstallerError::LifecycleHelper)?;
    }
    #[cfg(unix)]
    {
        std::process::Command::new("sh")
            .args([
                "-c",
                "sleep 1; if [ -e \"$1\" ] || [ -L \"$1\" ]; then [ -d \"$1\" ] && [ ! -L \"$1\" ] || exit 1; rm -rf -- \"$1\" || exit 1; fi; rmdir -- \"$2\"",
                "artisan-uninstall",
            ])
            .arg(&versions)
            .arg(&marker.path)
            .spawn()
            .map_err(|_| InstallerError::LifecycleHelper)?;
    }
    Ok(())
}

fn invoke_ae(release: &Path, arguments: &[&str]) -> Result<()> {
    let executable = release
        .join("bin")
        .join(if cfg!(windows) { "ae.exe" } else { "ae" });
    if !ordinary_file_exists(&executable)? {
        return Err(InstallerError::MissingCli(executable));
    }
    let status = background_command(&executable)
        .args(arguments)
        .status()
        .map_err(io(&executable))?;
    if !status.success() {
        return Err(InstallerError::CliFailed {
            command: arguments.join(" "),
            status: status.to_string(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use sha2::{Digest, Sha256};
    use tempfile::tempdir;

    use super::{
        InstallerError, InstallerLock, PendingMarker, PendingMarkerKind, RootMode, StageLease,
        complete_install, inspect_activation_pointer, pending_marker_path,
        recover_activation_pointer_swap, remove_path_in_root, remove_validated_activation_pointer,
    };

    #[cfg(windows)]
    use super::prepend_windows_path_entry;

    #[cfg(unix)]
    fn create_directory_link(target: &std::path::Path, link: &std::path::Path) -> bool {
        std::os::unix::fs::symlink(target, link).is_ok()
    }

    #[cfg(windows)]
    fn create_directory_link(target: &std::path::Path, link: &std::path::Path) -> bool {
        std::os::windows::fs::symlink_dir(target, link).is_ok()
    }

    #[cfg(unix)]
    fn remove_directory_link(link: &std::path::Path) {
        fs::remove_file(link).expect("directory link");
    }

    #[cfg(windows)]
    fn remove_directory_link(link: &std::path::Path) {
        fs::remove_dir(link).expect("directory link");
    }

    #[cfg(unix)]
    fn create_file_link(target: &std::path::Path, link: &std::path::Path) -> bool {
        std::os::unix::fs::symlink(target, link).is_ok()
    }

    #[cfg(windows)]
    fn create_file_link(target: &std::path::Path, link: &std::path::Path) -> bool {
        std::os::windows::fs::symlink_file(target, link).is_ok()
    }

    fn activation_document(root: &Path, active_version: &str) -> serde_json::Value {
        serde_json::json!({
            "format_version": 1,
            "install_root": root,
            "activation_state": "active",
            "finalization_state": "complete",
            "active_version": active_version,
            "permanent_ae_path": root.join("bin").join(if cfg!(windows) { "ae.exe" } else { "ae" }),
        })
    }

    fn activation_bytes(root: &Path, active_version: &str) -> Vec<u8> {
        serde_json::to_vec(&activation_document(root, active_version)).expect("activation JSON")
    }

    fn assert_ambiguous(error: &InstallerError) {
        assert!(matches!(
            error,
            InstallerError::InstallationActivationTransactionAmbiguous
        ));
        assert_eq!(
            error.to_string(),
            "installation activation transaction is ambiguous; no files were changed"
        );
    }

    #[test]
    fn activation_recovery_without_residue_is_a_no_op() {
        let directory = tempdir().expect("temp");
        let root = directory.path().join("Artisan");
        let lock = InstallerLock::acquire(&root, RootMode::Create).expect("root lock");

        recover_activation_pointer_swap(&lock).expect("no-residue recovery");

        for path in super::activation_pointer_paths(&root) {
            assert!(path.symlink_metadata().is_err());
        }
    }

    #[test]
    fn activation_recovery_after_crash_before_pointer_swap_preserves_current() {
        let directory = tempdir().expect("temp");
        let root = directory.path().join("Artisan");
        let lock = InstallerLock::acquire(&root, RootMode::Create).expect("root lock");
        let [current, temporary, previous] = super::activation_pointer_paths(&root);
        let current_bytes = activation_bytes(&root, "1.2.3");
        fs::write(&current, &current_bytes).expect("current pointer");
        fs::write(&temporary, activation_bytes(&root, "2.0.0")).expect("temporary pointer");

        recover_activation_pointer_swap(&lock).expect("pre-swap recovery");

        assert_eq!(fs::read(&current).expect("current bytes"), current_bytes);
        assert!(temporary.symlink_metadata().is_err());
        assert!(previous.symlink_metadata().is_err());
    }

    #[test]
    fn activation_recovery_after_current_to_previous_restores_exact_bytes() {
        let directory = tempdir().expect("temp");
        let root = directory.path().join("Artisan");
        let lock = InstallerLock::acquire(&root, RootMode::Create).expect("root lock");
        let [current, temporary, previous] = super::activation_pointer_paths(&root);
        let previous_bytes = activation_bytes(&root, "1.2.3");
        fs::write(&previous, &previous_bytes).expect("previous pointer");
        fs::write(&temporary, activation_bytes(&root, "2.0.0")).expect("temporary pointer");

        recover_activation_pointer_swap(&lock).expect("previous recovery");

        assert_eq!(fs::read(&current).expect("restored bytes"), previous_bytes);
        assert!(temporary.symlink_metadata().is_err());
        assert!(previous.symlink_metadata().is_err());
    }

    #[test]
    fn activation_recovery_removes_uncommitted_temporary_without_pointer() {
        let directory = tempdir().expect("temp");
        let root = directory.path().join("Artisan");
        let lock = InstallerLock::acquire(&root, RootMode::Create).expect("root lock");
        let [current, temporary, previous] = super::activation_pointer_paths(&root);
        fs::write(&temporary, activation_bytes(&root, "2.0.0")).expect("temporary pointer");

        recover_activation_pointer_swap(&lock).expect("temporary-only recovery");

        assert!(current.symlink_metadata().is_err());
        assert!(temporary.symlink_metadata().is_err());
        assert!(previous.symlink_metadata().is_err());
    }

    #[test]
    fn activation_recovery_after_new_current_installation_keeps_new_bytes() {
        let directory = tempdir().expect("temp");
        let root = directory.path().join("Artisan");
        let lock = InstallerLock::acquire(&root, RootMode::Create).expect("root lock");
        let [current, temporary, previous] = super::activation_pointer_paths(&root);
        let current_bytes = activation_bytes(&root, "2.0.0");
        fs::write(&current, &current_bytes).expect("new current pointer");
        fs::write(&previous, activation_bytes(&root, "1.2.3")).expect("previous pointer");

        recover_activation_pointer_swap(&lock).expect("post-swap recovery");

        assert_eq!(fs::read(&current).expect("current bytes"), current_bytes);
        assert!(temporary.symlink_metadata().is_err());
        assert!(previous.symlink_metadata().is_err());
    }

    #[test]
    fn activation_recovery_is_idempotent_after_first_recovery() {
        let directory = tempdir().expect("temp");
        let root = directory.path().join("Artisan");
        let lock = InstallerLock::acquire(&root, RootMode::Create).expect("root lock");
        let [current, temporary, previous] = super::activation_pointer_paths(&root);
        let current_bytes = activation_bytes(&root, "1.2.3");
        fs::write(&current, &current_bytes).expect("current pointer");
        fs::write(&temporary, activation_bytes(&root, "2.0.0")).expect("temporary pointer");
        fs::write(&previous, activation_bytes(&root, "0.9.0")).expect("previous pointer");

        recover_activation_pointer_swap(&lock).expect("first recovery");
        recover_activation_pointer_swap(&lock).expect("second recovery");

        assert_eq!(fs::read(&current).expect("current bytes"), current_bytes);
        assert!(temporary.symlink_metadata().is_err());
        assert!(previous.symlink_metadata().is_err());
    }

    #[test]
    fn activation_recovery_rejects_invalid_documents_without_mutation() {
        let directory = tempdir().expect("temp");
        let invalid_root = directory.path().join("different-root");
        for name in [
            "malformed",
            "root-mismatch",
            "pending",
            "non-complete",
            "unsafe-version",
            "unsafe-path",
        ] {
            let root = directory.path().join(format!("case-{name}"));
            let lock = InstallerLock::acquire(&root, RootMode::Create).expect("root lock");
            let current = root.join("installation.json");
            let bytes = if name == "malformed" {
                b"{".to_vec()
            } else {
                let mut document = activation_document(&root, "1.2.3");
                match name {
                    "root-mismatch" => document["install_root"] = serde_json::json!(invalid_root),
                    "pending" => document["activation_state"] = serde_json::json!("pending"),
                    "non-complete" => document["finalization_state"] = serde_json::json!("pending"),
                    "unsafe-version" => document["active_version"] = serde_json::json!("../escape"),
                    "unsafe-path" => {
                        document["permanent_ae_path"] = serde_json::json!(invalid_root.join("ae"));
                    }
                    _ => unreachable!(),
                }
                serde_json::to_vec(&document).expect("invalid state document")
            };
            fs::write(&current, &bytes).expect("invalid current pointer");

            let error = recover_activation_pointer_swap(&lock).expect_err("invalid pointer");
            assert_ambiguous(&error);
            assert_eq!(fs::read(&current).expect("current remains"), bytes);
            assert!(
                root.join(".installation.json.tmp")
                    .symlink_metadata()
                    .is_err()
            );
            assert!(
                root.join(".installation.json.previous")
                    .symlink_metadata()
                    .is_err()
            );
        }
    }

    #[test]
    fn activation_recovery_validates_all_residue_before_mutating() {
        let directory = tempdir().expect("temp");
        let root = directory.path().join("Artisan");
        let lock = InstallerLock::acquire(&root, RootMode::Create).expect("root lock");
        let [current, temporary, previous] = super::activation_pointer_paths(&root);
        let current_bytes = activation_bytes(&root, "1.2.3");
        let previous_bytes = activation_bytes(&root, "0.9.0");
        fs::write(&current, &current_bytes).expect("current pointer");
        fs::write(&temporary, b"malformed").expect("malformed temporary pointer");
        fs::write(&previous, &previous_bytes).expect("previous pointer");

        let error = recover_activation_pointer_swap(&lock).expect_err("ambiguous residue");
        assert_ambiguous(&error);
        assert_eq!(fs::read(&current).expect("current remains"), current_bytes);
        assert_eq!(
            fs::read(&temporary).expect("temporary remains"),
            b"malformed"
        );
        assert_eq!(
            fs::read(&previous).expect("previous remains"),
            previous_bytes
        );
    }

    #[test]
    fn activation_recovery_rejects_links_and_directories_without_mutation() {
        let directory = tempdir().expect("temp");
        let root = directory.path().join("Artisan");
        let lock = InstallerLock::acquire(&root, RootMode::Create).expect("root lock");
        let target = root.join("foreign-pointer");
        let temporary = root.join(".installation.json.tmp");
        fs::write(&target, activation_bytes(&root, "1.2.3")).expect("foreign pointer");
        if !create_file_link(&target, &temporary) {
            eprintln!("SKIP: file links are not supported on this host");
            return;
        }

        let error = recover_activation_pointer_swap(&lock).expect_err("symlink residue");
        assert_ambiguous(&error);
        assert!(temporary.symlink_metadata().is_ok());
        assert_eq!(
            fs::read(&target).expect("foreign pointer remains"),
            activation_bytes(&root, "1.2.3")
        );

        fs::remove_file(&temporary).expect("remove test link");
        let previous = root.join(".installation.json.previous");
        fs::create_dir(&previous).expect("directory residue");
        let error = recover_activation_pointer_swap(&lock).expect_err("directory residue");
        assert_ambiguous(&error);
        assert!(previous.is_dir());
    }

    #[cfg(windows)]
    #[test]
    fn activation_recovery_rejects_windows_reparse_residue_without_mutation() {
        let directory = tempdir().expect("temp");
        let root = directory.path().join("Artisan");
        let lock = InstallerLock::acquire(&root, RootMode::Create).expect("root lock");
        let target = root.join("foreign-directory");
        let temporary = root.join(".installation.json.tmp");
        fs::create_dir(&target).expect("foreign directory");
        if !create_directory_link(&target, &temporary) {
            eprintln!("SKIP: directory links are not supported on this host");
            return;
        }

        let error = recover_activation_pointer_swap(&lock).expect_err("reparse residue");
        assert_ambiguous(&error);
        assert!(temporary.symlink_metadata().is_ok());
        assert!(target.is_dir());
    }

    #[test]
    fn activation_recovery_rejects_identity_substituted_residue() {
        let directory = tempdir().expect("temp");
        let root = directory.path().join("Artisan");
        let _lock = InstallerLock::acquire(&root, RootMode::Create).expect("root lock");
        let temporary = root.join(".installation.json.tmp");
        let replacement = root.join("replacement");
        fs::write(&temporary, activation_bytes(&root, "1.2.3")).expect("temporary pointer");
        let validated = inspect_activation_pointer(&root, &temporary)
            .expect("inspect temporary pointer")
            .expect("temporary pointer");
        fs::write(&replacement, activation_bytes(&root, "2.0.0")).expect("replacement");
        fs::remove_file(&temporary).expect("remove original pointer");
        fs::rename(&replacement, &temporary).expect("substitute pointer");

        let error =
            remove_validated_activation_pointer(&validated).expect_err("identity substitution");
        assert_ambiguous(&error);
        assert!(temporary.is_file());
    }

    #[test]
    fn activation_recovery_preserves_versions_credentials_data_and_spaces() {
        let directory = tempdir().expect("temp");
        let root = directory.path().join("Artisan install root with spaces");
        let lock = InstallerLock::acquire(&root, RootMode::Create).expect("root lock");
        let versions = root.join("versions").join("1.2.3").join("payload");
        let credentials = root.join("credentials").join("credentials.json");
        let data = root.join("data").join("state.db");
        for path in [&versions, &credentials, &data] {
            fs::create_dir_all(path.parent().expect("parent")).expect("preserved directory");
            fs::write(path, path.to_string_lossy().as_bytes()).expect("preserved file");
        }
        let [current, temporary, previous] = super::activation_pointer_paths(&root);
        let current_bytes = activation_bytes(&root, "1.2.3");
        fs::write(&current, &current_bytes).expect("current pointer");
        fs::write(&temporary, activation_bytes(&root, "2.0.0")).expect("temporary pointer");

        recover_activation_pointer_swap(&lock).expect("space-path recovery");

        assert_eq!(fs::read(&current).expect("current bytes"), current_bytes);
        for path in [&versions, &credentials, &data] {
            assert_eq!(
                fs::read(path).expect("preserved file bytes"),
                path.to_string_lossy().as_bytes()
            );
        }
        assert!(temporary.symlink_metadata().is_err());
        assert!(previous.symlink_metadata().is_err());
    }

    #[test]
    fn installer_lock_serializes_releases_and_preserves_its_sentinel() {
        let directory = tempdir().expect("temp");
        let root = directory.path().join("nested").join("Artisan");
        let sentinel = root.join(super::INSTALLER_LOCK_NAME);
        fs::create_dir_all(&root).expect("root");
        fs::write(&sentinel, b"sentinel").expect("sentinel contents");
        let first = InstallerLock::acquire(&root, RootMode::Existing).expect("first lock");

        let contention = InstallerLock::acquire(&root, RootMode::Existing)
            .expect_err("the root lock must be exclusive");
        assert!(matches!(contention, InstallerError::InstallationRootBusy));
        assert_eq!(contention.to_string(), "installation root is busy");
        assert_eq!(format!("{contention:?}"), "InstallationRootBusy");
        assert_eq!(format!("{first:?}"), "InstallerLock");

        drop(first);
        let second = InstallerLock::acquire(&root, RootMode::Existing).expect("released lock");
        drop(second);
        assert_eq!(fs::read(sentinel).expect("sentinel remains"), b"sentinel");
    }

    #[test]
    fn root_creation_rejects_unsafe_shapes_without_following_them() {
        let directory = tempdir().expect("temp");
        let missing_root = directory.path().join("missing-root");
        assert!(matches!(
            InstallerLock::acquire(&missing_root, RootMode::Existing),
            Err(InstallerError::UnsafeInstallationRoot)
        ));
        assert!(!missing_root.exists());

        let file_root = directory.path().join("file-root");
        fs::write(&file_root, b"foreign").expect("file root");
        assert!(matches!(
            InstallerLock::acquire(&file_root, RootMode::Create),
            Err(InstallerError::UnsafeInstallationRoot)
        ));

        let real_parent = directory.path().join("real-parent");
        fs::create_dir(&real_parent).expect("real parent");
        let linked_root = directory.path().join("linked-root");
        if create_directory_link(&real_parent, &linked_root) {
            assert!(matches!(
                InstallerLock::acquire(&linked_root, RootMode::Existing),
                Err(InstallerError::UnsafeInstallationRoot)
            ));
            remove_directory_link(&linked_root);
        }
        let linked_parent = directory.path().join("linked-parent");
        if !create_directory_link(&real_parent, &linked_parent) {
            eprintln!("SKIP: directory links are not supported on this host");
            return;
        }
        let unsafe_root = linked_parent.join("root");
        assert!(matches!(
            InstallerLock::acquire(&unsafe_root, RootMode::Create),
            Err(InstallerError::UnsafeInstallationRoot)
        ));
        assert!(!unsafe_root.exists());
    }

    #[test]
    fn lock_and_pending_marker_shapes_fail_closed_and_stay_untouched() {
        let directory = tempdir().expect("temp");
        let root = directory.path().join("Artisan");
        fs::create_dir(&root).expect("root");
        let lock_path = root.join(super::INSTALLER_LOCK_NAME);
        fs::create_dir(&lock_path).expect("lock directory");
        assert!(matches!(
            InstallerLock::acquire(&root, RootMode::Existing),
            Err(InstallerError::InvalidInstallerLock)
        ));
        fs::remove_dir(&lock_path).expect("lock directory");
        let lock_target = directory.path().join("foreign-lock");
        fs::write(&lock_target, b"foreign").expect("foreign lock target");
        if create_file_link(&lock_target, &lock_path) {
            assert!(matches!(
                InstallerLock::acquire(&root, RootMode::Existing),
                Err(InstallerError::InvalidInstallerLock)
            ));
            fs::remove_file(&lock_path).expect("lock link");
        }

        let marker = pending_marker_path(&root, PendingMarkerKind::Cleanup).expect("marker path");
        fs::write(&marker, b"foreign marker").expect("foreign marker file");
        assert!(matches!(
            InstallerLock::acquire(&root, RootMode::Existing),
            Err(InstallerError::InvalidInstallerMarker)
        ));
        fs::remove_file(&marker).expect("foreign marker file");
        fs::create_dir(&marker).expect("foreign marker");
        let error = InstallerLock::acquire(&root, RootMode::Existing)
            .expect_err("pending marker must fence the root");
        assert!(matches!(error, InstallerError::InstallationRootPending));
        assert!(marker.is_dir());
        assert_eq!(
            error.to_string(),
            "installation root has a pending operation"
        );
    }

    #[test]
    fn pending_markers_collide_atomically_and_clear_only_explicitly() {
        let directory = tempdir().expect("temp");
        let root = directory.path().join("Artisan");
        let lock = InstallerLock::acquire(&root, RootMode::Create).expect("root lock");
        let marker =
            PendingMarker::create(&lock, PendingMarkerKind::AeReplacement).expect("marker");
        assert!(marker.path.is_dir());
        assert_eq!(format!("{marker:?}"), "PendingMarker");
        assert!(matches!(
            PendingMarker::create(&lock, PendingMarkerKind::Cleanup),
            Err(InstallerError::InstallationRootPending)
        ));
        let marker_path = marker.path.clone();
        drop(marker);
        assert!(marker_path.is_dir());
        let collision = PendingMarker::create(&lock, PendingMarkerKind::AeReplacement)
            .expect_err("the existing marker remains a collision");
        assert!(matches!(collision, InstallerError::InstallationRootPending));
        let marker = PendingMarker {
            identity: super::ordinary_path_identity(&marker_path, super::EntryKind::Directory)
                .expect("marker identity"),
            path: marker_path,
        };
        marker
            .clear_after_success()
            .expect("helper success cleanup");
        assert!(
            !pending_marker_path(&root, PendingMarkerKind::AeReplacement)
                .expect("marker path")
                .exists()
        );
    }

    #[cfg(unix)]
    #[test]
    fn lock_rejects_path_identity_substitution() {
        let directory = tempdir().expect("temp");
        let root = directory.path().join("Artisan");
        let lock = InstallerLock::acquire(&root, RootMode::Create).expect("root lock");
        let sentinel = root.join(super::INSTALLER_LOCK_NAME);
        let moved = root.join("moved-lock");
        fs::rename(&sentinel, &moved).expect("move sentinel");
        fs::write(&sentinel, b"replacement").expect("replacement sentinel");

        assert!(matches!(
            lock.re_fence(),
            Err(InstallerError::InstallationRootChanged)
        ));
        assert_eq!(
            fs::read(&sentinel).expect("replacement remains"),
            b"replacement"
        );
        assert_eq!(fs::read(moved).expect("original remains open"), b"sentinel");
    }

    #[test]
    fn recursive_owned_removal_preserves_a_foreign_link_and_target() {
        let directory = tempdir().expect("temp");
        let root = directory.path().join("Artisan");
        let foreign = directory.path().join("foreign");
        fs::create_dir(&root).expect("root");
        fs::create_dir(&foreign).expect("foreign");
        fs::write(foreign.join("keep"), b"keep").expect("foreign contents");
        let link = root.join("data");
        if !create_directory_link(&foreign, &link) {
            eprintln!("SKIP: directory links are not supported on this host");
            return;
        }

        let error = remove_path_in_root(&root, &link).expect_err("foreign link is unsafe");
        assert!(matches!(error, InstallerError::UnsafeOwnedPath));
        assert!(link.symlink_metadata().is_ok());
        assert_eq!(
            fs::read(foreign.join("keep")).expect("foreign target"),
            b"keep"
        );
    }

    #[test]
    fn sha256_representation_matches_release_contract() {
        let mut hasher = Sha256::new();
        hasher.update(b"artisan");
        assert_eq!(
            hex::encode(hasher.finalize()),
            "0b74ed7ff22b86fd0838fd29a78940a8d54377951e968867948a57b3e53646fc"
        );
    }

    #[test]
    fn permanent_lifecycle_binary_has_a_stable_archive_location() {
        let root = tempdir().expect("temp");
        let release = root.path().join("versions").join("1.2.3");
        let expected =
            root.path()
                .join("versions")
                .join("1.2.3")
                .join("bin")
                .join(if cfg!(windows) {
                    "installer.exe"
                } else {
                    "installer"
                });
        assert_eq!(super::versioned_installer_path(&release), expected);
        assert!(!expected.ends_with(if cfg!(windows) {
            "ae-installer.exe"
        } else {
            "ae-installer"
        }));
    }

    #[test]
    fn first_install_leaves_forge_launch_to_the_editor_handoff() {
        assert_eq!(super::FIRST_RUN_CONFIGURATION_COMMANDS[0], ["setup"]);
        assert!(
            super::FIRST_RUN_CONFIGURATION_COMMANDS
                .iter()
                .flat_map(|arguments| arguments.iter())
                .all(|argument| *argument != "start")
        );
        assert!(!super::should_restore_retired_forge(
            true,
            super::Retirement {
                editors_closed: 1,
                forges_stopped: 1,
            },
        ));
    }

    #[test]
    fn maintenance_update_restores_a_previously_running_forge() {
        assert!(super::should_restore_retired_forge(
            false,
            super::Retirement {
                editors_closed: 0,
                forges_stopped: 1,
            },
        ));
        assert!(!super::should_restore_retired_forge(
            false,
            super::Retirement::default(),
        ));
    }

    #[test]
    fn installation_manifest_components_are_always_enabled() {
        assert_eq!(
            serde_json::to_value(super::installed_components()).expect("component projection"),
            serde_json::json!({"editor": true, "forge": true})
        );
    }

    #[test]
    fn failed_install_removes_only_its_owned_stage() {
        let root = tempdir().expect("temp");
        let stage = root.path().join(".stage-1.2.3-owned");
        let sibling = root.path().join(".stage-1.2.3-sibling");
        let mut lease = StageLease::acquire(stage.clone(), "1.2.3").expect("stage lease");
        fs::create_dir(&sibling).expect("sibling stage");
        fs::write(stage.join("partial"), b"partial payload").expect("partial payload");

        let result = complete_install(
            &mut lease,
            Err(InstallerError::Archive(
                "post-acquisition failure".to_owned(),
            )),
        );

        assert!(matches!(
            result,
            Err(InstallerError::Archive(message)) if message == "post-acquisition failure"
        ));
        assert!(!stage.exists());
        assert!(sibling.is_dir());
    }

    #[test]
    fn pre_existing_stage_collision_is_rejected_and_untouched() {
        let root = tempdir().expect("temp");
        let stage = root.path().join(".stage-1.2.3-owned");
        fs::create_dir(&stage).expect("pre-existing stage");
        let marker = stage.join("marker");
        fs::write(&marker, b"keep").expect("collision marker");

        let result = StageLease::acquire(stage.clone(), "1.2.3");

        assert!(matches!(
            result,
            Err(InstallerError::ExistingRelease(version)) if version == "1.2.3"
        ));
        assert!(stage.is_dir());
        assert_eq!(fs::read(marker).expect("collision marker"), b"keep");
    }

    #[test]
    fn missing_owned_stage_cleanup_is_idempotent() {
        let root = tempdir().expect("temp");
        let stage = root.path().join(".stage-1.2.3-owned");
        let mut lease = StageLease::acquire(stage.clone(), "1.2.3").expect("stage lease");
        fs::remove_dir(&stage).expect("remove stage before cleanup");

        assert!(lease.cleanup().is_ok());
        assert!(!lease.armed);
        assert!(lease.cleanup().is_ok());
    }

    #[test]
    fn cleanup_refuses_a_regular_file_target() {
        let root = tempdir().expect("temp");
        let stage = root.path().join(".stage-1.2.3-owned");
        let mut lease = StageLease::acquire(stage.clone(), "1.2.3").expect("stage lease");
        fs::remove_dir(&stage).expect("remove stage before replacement");
        fs::write(&stage, b"do not remove").expect("file replacement");

        let result = lease.cleanup();

        assert!(matches!(
            result,
            Err(InstallerError::StageCleanupIncomplete)
        ));
        assert_eq!(fs::read(stage).expect("file target"), b"do not remove");
    }

    #[test]
    fn cleanup_refuses_a_link_or_reparse_target() {
        let root = tempdir().expect("temp");
        let target = root.path().join("target");
        fs::create_dir(&target).expect("link target");
        let stage = root.path().join(".stage-1.2.3-owned");
        let mut lease = StageLease::acquire(stage.clone(), "1.2.3").expect("stage lease");
        fs::remove_dir(&stage).expect("remove stage before replacement");
        if !create_directory_link(&target, &stage) {
            eprintln!("SKIP: directory links are not supported on this host");
            return;
        }

        let result = lease.cleanup();

        assert!(matches!(
            result,
            Err(InstallerError::StageCleanupIncomplete)
        ));
        assert!(stage.symlink_metadata().is_ok());
        assert!(target.is_dir());
    }

    #[test]
    fn cleanup_failure_takes_precedence_and_is_path_free() {
        let root = tempdir().expect("temp");
        let stage = root.path().join(".stage-1.2.3-owned");
        let mut lease = StageLease::acquire(stage.clone(), "1.2.3").expect("stage lease");
        fs::remove_dir(&stage).expect("remove stage before replacement");
        fs::write(&stage, b"preserve").expect("file replacement");
        let original = format!("original failure at {}", stage.display());

        let error = complete_install(&mut lease, Err(InstallerError::Archive(original)))
            .expect_err("cleanup failure");

        assert!(matches!(error, InstallerError::StageCleanupIncomplete));
        assert_eq!(error.to_string(), "staging cleanup could not be completed");
        assert!(!error.to_string().contains(&stage.display().to_string()));
        assert_eq!(fs::read(stage).expect("file target"), b"preserve");
    }

    #[test]
    fn successful_transfer_disarms_lease_before_later_failure() {
        let root = tempdir().expect("temp");
        let stage = root.path().join(".stage-1.2.3-owned");
        let release_parent = root.path().join("versions");
        let release = release_parent.join("1.2.3");
        fs::create_dir(&release_parent).expect("release parent");
        let mut lease = StageLease::acquire(stage.clone(), "1.2.3").expect("stage lease");
        fs::write(stage.join("payload"), b"release payload").expect("payload");

        lease.transfer_to(&release).expect("stage transfer");
        let result = complete_install(
            &mut lease,
            Err(InstallerError::Archive("activation failure".to_owned())),
        );

        assert!(matches!(
            result,
            Err(InstallerError::Archive(message)) if message == "activation failure"
        ));
        assert!(!lease.armed);
        assert!(!stage.exists());
        assert_eq!(
            fs::read(release.join("payload")).expect("release payload"),
            b"release payload"
        );
    }

    #[cfg(windows)]
    #[test]
    fn stable_cli_precedes_stale_path_entries() {
        let stable = r"C:\Users\test\AppData\Local\Artisan\bin";
        let legacy = r"C:\Users\test\AppData\Local\Programs\artisan-editor\resources\artisan-forge";

        assert_eq!(
            prepend_windows_path_entry(&format!("{legacy};{stable};C:\\Windows"), stable),
            format!("{stable};{legacy};C:\\Windows")
        );
    }
}
