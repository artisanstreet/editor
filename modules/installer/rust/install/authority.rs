//! Guarded filesystem authority for one installation root.
//!
//! Owns the exclusive installer lock, the pending-operation markers that fence
//! the root, identity-checked file and directory primitives, and the stage
//! lease used while a release is being unpacked. Every open refuses symbolic
//! links and reparse points and revalidates identities around each mutation so
//! the workflow and state children can act on paths without re-implementing
//! the safety checks.

use std::{
    cell::RefCell,
    ffi::OsString,
    fs::{File, Metadata, OpenOptions},
    io::Read,
    path::{Component, Path, PathBuf},
};

use fs2::FileExt;
use sha2::{Digest, Sha256};

use crate::error::{InstallerError, Result, io};

pub(crate) const INSTALLER_LOCK_NAME: &str = ".installer.lock";
const AE_REPLACEMENT_MARKER_SUFFIX: &str = ".artisan-installer-ae-replacement.pending";
const CLEANUP_MARKER_SUFFIX: &str = ".artisan-installer-cleanup.pending";
#[cfg(windows)]
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
#[cfg(windows)]
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RootMode {
    Create,
    Existing,
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) struct FileIdentity(u64, u64);

#[derive(Clone, Copy)]
pub(crate) enum EntryKind {
    File,
    Directory,
}

#[derive(Clone, Copy)]
pub(crate) enum PendingMarkerKind {
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
pub(crate) struct InstallerLock {
    file: File,
    pub(crate) root: PathBuf,
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
    pub(crate) fn acquire(root: &Path, mode: RootMode) -> Result<Self> {
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
    pub(crate) fn re_fence(&self) -> Result<()> {
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

    pub(crate) fn fence(&self) -> Result<()> {
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
pub(crate) struct PendingMarker {
    pub(crate) path: PathBuf,
    pub(crate) identity: FileIdentity,
}

impl std::fmt::Debug for PendingMarker {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("PendingMarker")
    }
}

impl PendingMarker {
    pub(crate) fn create(lock: &InstallerLock, kind: PendingMarkerKind) -> Result<Self> {
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
    pub(crate) fn clear_after_success(&self) -> Result<()> {
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

pub(crate) fn pending_marker_path(root: &Path, kind: PendingMarkerKind) -> Result<PathBuf> {
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

pub(crate) fn ordinary_metadata(metadata: &Metadata, kind: EntryKind) -> bool {
    if metadata_is_symlink_or_reparse(metadata) {
        return false;
    }
    match kind {
        EntryKind::File => metadata.is_file(),
        EntryKind::Directory => metadata.is_dir(),
    }
}

pub(crate) fn ordinary_path_identity(
    path: &Path,
    kind: EntryKind,
) -> std::result::Result<FileIdentity, ()> {
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

pub(crate) fn identity_from_file(
    file: &File,
    kind: EntryKind,
) -> std::result::Result<FileIdentity, ()> {
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

pub(crate) fn open_for_read(path: &Path, kind: EntryKind) -> std::io::Result<File> {
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

pub(crate) fn ensure_owned_directory(path: &Path) -> Result<()> {
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

pub(crate) fn owned_file_identity(path: &Path) -> Result<Option<FileIdentity>> {
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

pub(crate) fn ordinary_file_exists(path: &Path) -> Result<bool> {
    Ok(owned_file_identity(path)?.is_some())
}

pub(crate) fn ordinary_directory_exists(path: &Path) -> Result<bool> {
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

pub(crate) fn create_owned_file(path: &Path) -> Result<File> {
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

pub(crate) fn copy_owned_file(source: &Path, destination: &Path) -> Result<FileIdentity> {
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

pub(crate) fn require_identity(path: &Path, kind: EntryKind, expected: FileIdentity) -> Result<()> {
    let actual =
        ordinary_path_identity(path, kind).map_err(|()| InstallerError::UnsafeOwnedPath)?;
    if actual != expected {
        return Err(InstallerError::UnsafeOwnedPath);
    }
    Ok(())
}

pub(crate) fn sync_owned_file(path: &Path, file: &File) -> Result<FileIdentity> {
    let identity =
        identity_from_file(file, EntryKind::File).map_err(|()| InstallerError::UnsafeOwnedPath)?;
    require_identity(path, EntryKind::File, identity)?;
    file.sync_all().map_err(io(path))?;
    require_identity(path, EntryKind::File, identity)?;
    Ok(identity)
}

pub(crate) fn remove_owned_file(path: &Path) -> Result<()> {
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
pub(crate) struct StageLease {
    path: PathBuf,
    pub(crate) armed: bool,
}

impl StageLease {
    pub(crate) fn acquire(path: PathBuf, release_version: &str) -> Result<Self> {
        match std::fs::create_dir(&path) {
            Ok(()) => Ok(Self { path, armed: true }),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                Err(InstallerError::ExistingRelease(release_version.to_owned()))
            }
            Err(error) => Err(io(&path)(error)),
        }
    }

    pub(crate) fn transfer_to(&mut self, release: &Path) -> Result<()> {
        std::fs::rename(&self.path, release).map_err(io(release))?;
        self.armed = false;
        Ok(())
    }

    pub(crate) fn cleanup(&mut self) -> Result<()> {
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

    pub(crate) fn finish(&self) -> Result<()> {
        if self.armed {
            Err(InstallerError::StageCleanupIncomplete)
        } else {
            Ok(())
        }
    }
}

pub(crate) fn complete_install(stage: &mut StageLease, result: Result<()>) -> Result<()> {
    match result {
        Ok(()) => stage.finish(),
        Err(original) => match stage.cleanup() {
            Ok(()) => Err(original),
            Err(cleanup) => Err(cleanup),
        },
    }
}

pub(crate) fn complete_install_locked(
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
