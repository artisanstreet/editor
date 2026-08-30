use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use base64::{Engine, engine::general_purpose::STANDARD};
use flate2::bufread::GzDecoder;
use fs2::FileExt;
use reqwest::blocking::Client;
use sha2::{Digest, Sha512};
use tempfile::{Builder, NamedTempFile};

use crate::{
    engine_catalog::{NativeOpenCode2Authority, NativeOpenCode2InstallSpec},
    instance::{self, NativeFileId, NativeInstanceConfig, NativeInstanceError},
};

const LOCK_TIMEOUT: Duration = Duration::from_secs(30);
const LOCK_POLL: Duration = Duration::from_millis(50);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);
const EXPANDED_ARCHIVE_BOUND: u64 = 512 * 1024 * 1024;
const MAX_ARCHIVE_ENTRIES: usize = 16_384;
const MAX_MEMBER_NAME_BYTES: usize = 255;
const STAGING_PREFIX: &str = "staging-";
const GENERATION_PREFIX: &str = "generation-";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InstallOutcome {
    Installed,
    AlreadyInstalled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub(crate) enum NativeOpenCode2InstallError {
    #[error("OpenCode2 is unsupported on this platform")]
    UnsupportedPlatform,
    #[error("OpenCode2 installation root is invalid")]
    InvalidRoot,
    #[error("OpenCode2 installation root is unavailable")]
    RootUnavailable,
    #[error("OpenCode2 installation lock timed out")]
    LockTimeout,
    #[error("OpenCode2 installation lock is unavailable")]
    LockUnavailable,
    #[error("OpenCode2 installation lock identity changed")]
    LockIdentityChanged,
    #[error("OpenCode2 installation state is invalid")]
    StateInvalid,
    #[error("OpenCode2 download failed")]
    DownloadFailed,
    #[error("OpenCode2 download was rejected")]
    DownloadRejected,
    #[error("OpenCode2 download exceeds its bound")]
    DownloadTooLarge,
    #[error("OpenCode2 download integrity is invalid")]
    IntegrityInvalid,
    #[error("OpenCode2 download integrity does not match")]
    IntegrityMismatch,
    #[error("OpenCode2 download custody changed")]
    ArchiveIdentityChanged,
    #[error("OpenCode2 archive is invalid")]
    ArchiveInvalid,
    #[error("OpenCode2 archive target is missing")]
    ArchiveTargetMissing,
    #[error("OpenCode2 archive target is invalid")]
    ArchiveTargetInvalid,
    #[error("OpenCode2 executable verification failed")]
    ExecutableInvalid,
    #[error("OpenCode2 generation could not be created")]
    GenerationUnavailable,
    #[error("OpenCode2 generation name collided")]
    GenerationCollision,
    #[error("OpenCode2 state could not be constructed")]
    StateConstructionFailed,
    #[error("OpenCode2 state could not be published")]
    StatePublicationFailed,
    #[error("OpenCode2 activation could not be verified")]
    ActivationUnverified,
    #[error("OpenCode2 activation is uncertain after publication")]
    PostCommitUncertain,
    #[error("OpenCode2 secure random source failed")]
    RandomUnavailable,
    #[error("OpenCode2 staging cleanup failed")]
    CleanupFailed,
}

impl NativeOpenCode2InstallError {
    pub(crate) const fn cli_reason(self) -> &'static str {
        match self {
            Self::UnsupportedPlatform => "unsupported_platform",
            Self::InvalidRoot => "invalid_root",
            Self::RootUnavailable => "root_unavailable",
            Self::LockTimeout => "lock_timeout",
            Self::LockUnavailable => "lock_unavailable",
            Self::LockIdentityChanged => "lock_identity_changed",
            Self::StateInvalid => "state_invalid",
            Self::DownloadFailed => "download_failed",
            Self::DownloadRejected => "download_rejected",
            Self::DownloadTooLarge => "download_too_large",
            Self::IntegrityInvalid => "integrity_invalid",
            Self::IntegrityMismatch => "integrity_mismatch",
            Self::ArchiveIdentityChanged => "archive_identity_changed",
            Self::ArchiveInvalid => "archive_invalid",
            Self::ArchiveTargetMissing => "archive_target_missing",
            Self::ArchiveTargetInvalid => "archive_target_invalid",
            Self::ExecutableInvalid => "executable_invalid",
            Self::GenerationUnavailable => "generation_unavailable",
            Self::GenerationCollision => "generation_collision",
            Self::StateConstructionFailed => "state_construction_failed",
            Self::StatePublicationFailed => "state_publication_failed",
            Self::ActivationUnverified => "activation_unverified",
            Self::PostCommitUncertain => "post_commit_uncertain",
            Self::RandomUnavailable => "random_unavailable",
            Self::CleanupFailed => "cleanup_failed",
        }
    }
}

#[derive(Debug)]
struct InstallPaths {
    database_parent: PathBuf,
    toolchain_root: PathBuf,
    engine_root: PathBuf,
    versions_root: PathBuf,
    lock_path: PathBuf,
}

impl InstallPaths {
    fn derive(
        instance: &NativeInstanceConfig,
        spec: &NativeOpenCode2InstallSpec,
    ) -> Result<Self, NativeOpenCode2InstallError> {
        let database = instance.database_path();
        if !database.is_absolute()
            || database
                .components()
                .any(|component| matches!(component, Component::ParentDir))
        {
            return Err(NativeOpenCode2InstallError::InvalidRoot);
        }
        let database_parent = database
            .parent()
            .filter(|parent| parent.is_absolute() && !parent.as_os_str().is_empty())
            .ok_or(NativeOpenCode2InstallError::InvalidRoot)?;
        verify_directory(database_parent)?;

        let toolchain_root = database_parent.join("toolchain");
        let engine_root = toolchain_root.join(spec.engine_id());
        let versions_root = engine_root.join("versions");
        let lock_path = engine_root.join("install.lock");
        if !engine_root.is_absolute()
            || engine_root
                .components()
                .any(|component| matches!(component, Component::ParentDir))
        {
            return Err(NativeOpenCode2InstallError::InvalidRoot);
        }
        Ok(Self {
            database_parent: database_parent.to_path_buf(),
            toolchain_root,
            engine_root,
            versions_root,
            lock_path,
        })
    }

    fn prepare(&self) -> Result<(), NativeOpenCode2InstallError> {
        ensure_directory(&self.toolchain_root)?;
        ensure_directory(&self.engine_root)?;
        ensure_directory(&self.versions_root)?;
        self.verify()
    }

    fn verify(&self) -> Result<(), NativeOpenCode2InstallError> {
        verify_directory(&self.database_parent)?;
        verify_directory(&self.toolchain_root)?;
        verify_directory(&self.engine_root)?;
        verify_directory(&self.versions_root)
    }
}

pub(crate) fn install(
    instance: &NativeInstanceConfig,
) -> Result<InstallOutcome, NativeOpenCode2InstallError> {
    if !is_supported_platform() {
        return Err(NativeOpenCode2InstallError::UnsupportedPlatform);
    }

    let authority = NativeOpenCode2Authority::new();
    let spec = NativeOpenCode2Authority::certified_install_spec();
    let paths = InstallPaths::derive(instance, &spec)?;
    paths.prepare()?;
    let lock = InstallLock::acquire(&paths)?;
    lock.fence(&paths)?;
    let state = authority
        .read_install_state(&paths.engine_root)
        .map_err(|_| NativeOpenCode2InstallError::StateInvalid)?;
    let already_installed = if state.is_some() {
        authority
            .resolve_active(instance)
            .map_err(|_| NativeOpenCode2InstallError::StateInvalid)?;
        true
    } else {
        false
    };
    cleanup_staging(&paths, &lock)?;
    if already_installed {
        return Ok(InstallOutcome::AlreadyInstalled);
    }

    let staging = StagingDirectory::create(&paths, &lock)?;
    let mut archive = download_archive(&paths, &lock, &spec)?;
    extract_archive(&mut archive, &staging.path, &paths, &lock, &spec)?;
    lock.fence(&paths)?;
    let staged_executable = staging.path.join(spec.binary());
    let staged_id = verify_executable(&staged_executable, &spec)?;

    verify_archive_custody(&archive)?;
    let archive_path = archive.path().to_path_buf();
    drop(archive);
    if fs::symlink_metadata(&archive_path).is_ok() {
        return Err(NativeOpenCode2InstallError::CleanupFailed);
    }

    lock.fence(&paths)?;
    verify_directory(&staging.path)?;
    let prepublish_id = verify_executable(&staged_executable, &spec)?;
    if prepublish_id != staged_id {
        return Err(NativeOpenCode2InstallError::ExecutableInvalid);
    }
    let generation_id = new_generation_id()?;
    let published = staging.publish(&paths, &lock, &generation_id)?;
    let generation_executable = published.path.join(spec.binary());
    lock.fence(&paths)?;
    let published_id = verify_executable(&generation_executable, &spec)?;
    if published_id != staged_id {
        return Err(NativeOpenCode2InstallError::ExecutableInvalid);
    }

    let state = authority
        .new_install_state(&generation_id, None)
        .map_err(|_| NativeOpenCode2InstallError::StateConstructionFailed)?;
    lock.fence(&paths)?;
    let publication = authority
        .write_install_state(&paths.engine_root, &state)
        .map_err(|_| NativeOpenCode2InstallError::StatePublicationFailed)?;
    match publication {
        instance::NativeAtomicReplaceOutcome::Committed => {
            verify_activation(
                &authority,
                instance,
                &paths,
                &generation_id,
                staged_id,
                false,
            )?;
        }
        instance::NativeAtomicReplaceOutcome::CommittedButUnverified => {
            verify_activation(
                &authority,
                instance,
                &paths,
                &generation_id,
                staged_id,
                true,
            )?;
        }
    }
    lock.fence(&paths)?;
    Ok(InstallOutcome::Installed)
}

fn is_supported_platform() -> bool {
    cfg!(all(target_os = "windows", target_arch = "x86_64"))
}

fn verify_directory(path: &Path) -> Result<(), NativeOpenCode2InstallError> {
    for ancestor in path.ancestors() {
        let metadata = fs::symlink_metadata(ancestor)
            .map_err(|_| NativeOpenCode2InstallError::RootUnavailable)?;
        if is_reparse_or_symlink(&metadata) || !metadata.is_dir() {
            return Err(NativeOpenCode2InstallError::InvalidRoot);
        }
    }
    Ok(())
}

fn ensure_directory(path: &Path) -> Result<(), NativeOpenCode2InstallError> {
    let parent = path
        .parent()
        .ok_or(NativeOpenCode2InstallError::InvalidRoot)?;
    verify_directory(parent)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if is_reparse_or_symlink(&metadata) || !metadata.is_dir() {
                return Err(NativeOpenCode2InstallError::InvalidRoot);
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => match fs::create_dir(path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err(NativeOpenCode2InstallError::RootUnavailable),
        },
        Err(_) => return Err(NativeOpenCode2InstallError::RootUnavailable),
    }
    verify_directory(path)
}

fn is_reparse_or_symlink(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

struct InstallLock {
    path: PathBuf,
    file: File,
}

impl InstallLock {
    fn acquire(paths: &InstallPaths) -> Result<Self, NativeOpenCode2InstallError> {
        paths.verify()?;
        let file = open_lock(&paths.lock_path)?;
        let lock = Self {
            path: paths.lock_path.clone(),
            file,
        };
        let deadline = Instant::now() + LOCK_TIMEOUT;
        loop {
            match lock.file.try_lock_exclusive() {
                Ok(()) => break,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        return Err(NativeOpenCode2InstallError::LockTimeout);
                    }
                    thread::sleep(LOCK_POLL);
                }
                Err(_) => return Err(NativeOpenCode2InstallError::LockUnavailable),
            }
        }
        lock.fence(paths)?;
        Ok(lock)
    }

    fn fence(&self, paths: &InstallPaths) -> Result<(), NativeOpenCode2InstallError> {
        paths.verify()?;
        verify_regular_file(&self.path)?;
        if file_identity(&self.file)? != path_identity(&self.path)? {
            return Err(NativeOpenCode2InstallError::LockIdentityChanged);
        }
        Ok(())
    }
}

fn open_lock(path: &Path) -> Result<File, NativeOpenCode2InstallError> {
    if let Ok(metadata) = fs::symlink_metadata(path)
        && (is_reparse_or_symlink(&metadata) || !metadata.is_file())
    {
        return Err(NativeOpenCode2InstallError::LockUnavailable);
    }
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)
        .map_err(|_| NativeOpenCode2InstallError::LockUnavailable)?;
    let metadata = file
        .metadata()
        .map_err(|_| NativeOpenCode2InstallError::LockUnavailable)?;
    if is_reparse_or_symlink(&metadata) || !metadata.is_file() {
        return Err(NativeOpenCode2InstallError::LockUnavailable);
    }
    verify_regular_file(path)?;
    if file_identity(&file)? != path_identity(path)? {
        return Err(NativeOpenCode2InstallError::LockIdentityChanged);
    }
    Ok(file)
}

fn verify_regular_file(path: &Path) -> Result<(), NativeOpenCode2InstallError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| NativeOpenCode2InstallError::LockUnavailable)?;
    if is_reparse_or_symlink(&metadata) || !metadata.is_file() {
        return Err(NativeOpenCode2InstallError::LockIdentityChanged);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(windows)]
    volume: u64,
    #[cfg(windows)]
    index: u64,
}

fn file_identity(file: &File) -> Result<FileIdentity, NativeOpenCode2InstallError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        let metadata = file
            .metadata()
            .map_err(|_| NativeOpenCode2InstallError::LockUnavailable)?;
        return Ok(FileIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        });
    }
    #[cfg(windows)]
    {
        let info = winapi_util::file::information(winapi_util::HandleRef::from_file(file))
            .map_err(|_| NativeOpenCode2InstallError::LockUnavailable)?;
        let volume = info.volume_serial_number();
        let index = info.file_index();
        if volume == 0 && index == 0 {
            return Err(NativeOpenCode2InstallError::LockUnavailable);
        }
        Ok(FileIdentity { volume, index })
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = file;
        Err(NativeOpenCode2InstallError::LockUnavailable)
    }
}

fn path_identity(path: &Path) -> Result<FileIdentity, NativeOpenCode2InstallError> {
    let file = File::open(path).map_err(|_| NativeOpenCode2InstallError::LockUnavailable)?;
    file_identity(&file)
}

fn cleanup_staging(
    paths: &InstallPaths,
    lock: &InstallLock,
) -> Result<(), NativeOpenCode2InstallError> {
    lock.fence(paths)?;
    let entries = fs::read_dir(&paths.versions_root)
        .map_err(|_| NativeOpenCode2InstallError::CleanupFailed)?;
    for entry in entries {
        let entry = entry.map_err(|_| NativeOpenCode2InstallError::CleanupFailed)?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !is_staging_name(name) {
            continue;
        }
        let candidate = entry.path();
        verify_directory(&candidate)?;
        lock.fence(paths)?;
        verify_directory(&candidate)?;
        fs::remove_dir_all(&candidate)
            .map_err(|_| NativeOpenCode2InstallError::CleanupFailed)?;
        if fs::symlink_metadata(&candidate).is_ok() {
            return Err(NativeOpenCode2InstallError::CleanupFailed);
        }
    }
    lock.fence(paths)
}

fn is_staging_name(name: &str) -> bool {
    is_hex_name(name, STAGING_PREFIX)
}

fn is_generation_name(name: &str) -> bool {
    is_hex_name(name, GENERATION_PREFIX)
}

fn is_hex_name(name: &str, prefix: &str) -> bool {
    let Some(hex) = name.strip_prefix(prefix) else {
        return false;
    };
    hex.len() == 32
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

struct StagingDirectory {
    path: PathBuf,
    versions_root: PathBuf,
    published: bool,
}

impl StagingDirectory {
    fn create(
        paths: &InstallPaths,
        lock: &InstallLock,
    ) -> Result<Self, NativeOpenCode2InstallError> {
        for _ in 0..8 {
            lock.fence(paths)?;
            let path = paths.versions_root.join(random_name(STAGING_PREFIX)?);
            match fs::create_dir(&path) {
                Ok(()) => {
                    verify_directory(&path)?;
                    lock.fence(paths)?;
                    verify_directory(&path)?;
                    return Ok(Self {
                        path,
                        versions_root: paths.versions_root.clone(),
                        published: false,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(_) => return Err(NativeOpenCode2InstallError::GenerationUnavailable),
            }
        }
        Err(NativeOpenCode2InstallError::GenerationCollision)
    }

    fn publish(
        self,
        paths: &InstallPaths,
        lock: &InstallLock,
        generation_id: &str,
    ) -> Result<PublishedGeneration, NativeOpenCode2InstallError> {
        if !is_generation_name(generation_id) {
            return Err(NativeOpenCode2InstallError::GenerationUnavailable);
        }
        verify_directory(&self.path)?;
        lock.fence(paths)?;
        verify_directory(&self.path)?;
        let destination = paths.versions_root.join(generation_id);
        if fs::symlink_metadata(&destination).is_ok() {
            return Err(NativeOpenCode2InstallError::GenerationCollision);
        }
        let mut staging = self;
        fs::rename(&staging.path, &destination).map_err(|error| {
            if error.kind() == io::ErrorKind::AlreadyExists {
                NativeOpenCode2InstallError::GenerationCollision
            } else {
                NativeOpenCode2InstallError::GenerationUnavailable
            }
        })?;
        staging.published = true;
        drop(staging);
        verify_directory(&destination)?;
        lock.fence(paths)?;
        Ok(PublishedGeneration { path: destination })
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
            && verify_directory(&self.versions_root).is_ok()
        {
            let Ok(metadata) = fs::symlink_metadata(&self.path) else {
                return;
            };
            if is_reparse_or_symlink(&metadata) || !metadata.is_dir() {
                return;
            }
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

struct PublishedGeneration {
    path: PathBuf,
}

fn random_name(prefix: &str) -> Result<String, NativeOpenCode2InstallError> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|_| NativeOpenCode2InstallError::RandomUnavailable)?;
    Ok(format!("{prefix}{}", hex_lower(&bytes)))
}

fn new_generation_id() -> Result<String, NativeOpenCode2InstallError> {
    let name = random_name(GENERATION_PREFIX)?;
    if is_generation_name(&name) {
        Ok(name)
    } else {
        Err(NativeOpenCode2InstallError::GenerationUnavailable)
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push(HEX[(byte >> 4) as usize] as char);
        result.push(HEX[(byte & 0x0f) as usize] as char);
    }
    result
}

fn download_archive(
    paths: &InstallPaths,
    lock: &InstallLock,
    spec: &NativeOpenCode2InstallSpec,
) -> Result<NamedTempFile, NativeOpenCode2InstallError> {
    lock.fence(paths)?;
    let mut archive = Builder::new()
        .prefix(".opencode2-download-")
        .suffix(".tgz")
        .tempfile_in(&paths.versions_root)
        .map_err(|_| NativeOpenCode2InstallError::DownloadFailed)?;
    if archive.path().parent() != Some(paths.versions_root.as_path()) {
        return Err(NativeOpenCode2InstallError::DownloadFailed);
    }
    verify_archive_custody(&archive)?;

    let client = Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|_| NativeOpenCode2InstallError::DownloadFailed)?;
    let mut response = client
        .get(spec.npm_url())
        .send()
        .map_err(|_| NativeOpenCode2InstallError::DownloadFailed)?;
    if !response.status().is_success() {
        return Err(NativeOpenCode2InstallError::DownloadRejected);
    }
    let expected = certified_integrity(spec)?;
    let declared_length = response.content_length();
    let digest = copy_bounded(
        &mut response,
        archive.as_file_mut(),
        declared_length,
        spec.download_bound_bytes(),
    )?;
    verify_compressed_integrity(&digest, &expected)?;
    verify_archive_custody(&archive)?;
    lock.fence(paths)?;
    archive
        .as_file_mut()
        .flush()
        .map_err(|_| NativeOpenCode2InstallError::DownloadFailed)?;
    archive
        .as_file()
        .sync_all()
        .map_err(|_| NativeOpenCode2InstallError::DownloadFailed)?;
    verify_archive_custody(&archive)?;
    lock.fence(paths)?;
    Ok(archive)
}

fn verify_archive_custody(archive: &NamedTempFile) -> Result<(), NativeOpenCode2InstallError> {
    verify_regular_file(archive.path())
        .map_err(|_| NativeOpenCode2InstallError::ArchiveIdentityChanged)?;
    let handle_id = file_identity(archive.as_file())
        .map_err(|_| NativeOpenCode2InstallError::ArchiveIdentityChanged)?;
    let path_id = path_identity(archive.path())
        .map_err(|_| NativeOpenCode2InstallError::ArchiveIdentityChanged)?;
    if handle_id != path_id {
        return Err(NativeOpenCode2InstallError::ArchiveIdentityChanged);
    }
    Ok(())
}

fn certified_integrity(
    spec: &NativeOpenCode2InstallSpec,
) -> Result<[u8; 64], NativeOpenCode2InstallError> {
    let decoded = STANDARD
        .decode(spec.npm_integrity_sha512())
        .map_err(|_| NativeOpenCode2InstallError::IntegrityInvalid)?;
    decoded
        .try_into()
        .map_err(|_| NativeOpenCode2InstallError::IntegrityInvalid)
}

fn verify_compressed_integrity(
    actual: &[u8; 64],
    expected: &[u8; 64],
) -> Result<(), NativeOpenCode2InstallError> {
    if actual == expected {
        Ok(())
    } else {
        Err(NativeOpenCode2InstallError::IntegrityMismatch)
    }
}

fn copy_bounded<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    declared_length: Option<u64>,
    bound: u64,
) -> Result<[u8; 64], NativeOpenCode2InstallError> {
    if declared_length.is_some_and(|length| length > bound) {
        return Err(NativeOpenCode2InstallError::DownloadTooLarge);
    }
    let mut hasher = Sha512::new();
    let mut total = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|_| NativeOpenCode2InstallError::DownloadFailed)?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or(NativeOpenCode2InstallError::DownloadTooLarge)?;
        if total > bound {
            return Err(NativeOpenCode2InstallError::DownloadTooLarge);
        }
        writer
            .write_all(&buffer[..read])
            .map_err(|_| NativeOpenCode2InstallError::DownloadFailed)?;
        hasher.update(&buffer[..read]);
    }
    if declared_length.is_some_and(|length| length != total) {
        return Err(NativeOpenCode2InstallError::DownloadFailed);
    }
    let digest = hasher.finalize();
    let mut result = [0_u8; 64];
    result.copy_from_slice(&digest);
    Ok(result)
}

fn extract_archive(
    archive: &mut NamedTempFile,
    staging_path: &Path,
    paths: &InstallPaths,
    lock: &InstallLock,
    spec: &NativeOpenCode2InstallSpec,
) -> Result<(), NativeOpenCode2InstallError> {
    verify_directory(staging_path)?;
    verify_archive_custody(archive)?;
    lock.fence(paths)?;
    verify_directory(staging_path)?;
    verify_archive_custody(archive)?;
    archive
        .as_file_mut()
        .seek(SeekFrom::Start(0))
        .map_err(|_| NativeOpenCode2InstallError::ArchiveInvalid)?;
    let buffered = BufReader::new(archive.as_file_mut());
    let mut gzip = GzDecoder::new(buffered);
    let target_path = staging_path.join(spec.binary());
    parse_tar(
        &mut gzip,
        spec.archive_member(),
        &target_path,
        spec.executable_size_bytes(),
    )?;
    ensure_gzip_end(gzip)?;
    verify_archive_custody(archive)?;
    lock.fence(paths)?;
    Ok(())
}

fn ensure_gzip_end<R: BufRead>(mut gzip: GzDecoder<R>) -> Result<(), NativeOpenCode2InstallError> {
    let mut byte = [0_u8; 1];
    match gzip.read(&mut byte) {
        Ok(0) => {}
        Ok(_) | Err(_) => return Err(NativeOpenCode2InstallError::ArchiveInvalid),
    }
    let mut buffered = gzip.into_inner();
    if !buffered
        .fill_buf()
        .map_err(|_| NativeOpenCode2InstallError::ArchiveInvalid)?
        .is_empty()
    {
        return Err(NativeOpenCode2InstallError::ArchiveInvalid);
    }
    Ok(())
}

fn parse_tar<R: Read>(
    reader: &mut R,
    target_name: &str,
    target_path: &Path,
    target_size: u64,
) -> Result<(), NativeOpenCode2InstallError> {
    let mut names = ArchiveNames::default();
    let mut expanded = 0_u64;
    let mut entries = 0_usize;
    let mut target_found = false;
    let mut header = [0_u8; 512];
    loop {
        reader
            .read_exact(&mut header)
            .map_err(|_| NativeOpenCode2InstallError::ArchiveInvalid)?;
        if header.iter().all(|byte| *byte == 0) {
            let mut terminal = [0_u8; 512];
            reader
                .read_exact(&mut terminal)
                .map_err(|_| NativeOpenCode2InstallError::ArchiveInvalid)?;
            if terminal.iter().any(|byte| *byte != 0) {
                return Err(NativeOpenCode2InstallError::ArchiveInvalid);
            }
            break;
        }
        entries = entries
            .checked_add(1)
            .ok_or(NativeOpenCode2InstallError::ArchiveInvalid)?;
        if entries > MAX_ARCHIVE_ENTRIES {
            return Err(NativeOpenCode2InstallError::ArchiveInvalid);
        }
        validate_header(&header)?;
        let kind = match header[156] {
            0 | b'0' => EntryKind::Regular,
            b'5' => EntryKind::Directory,
            _ => return Err(NativeOpenCode2InstallError::ArchiveInvalid),
        };
        let size = parse_octal(&header[124..136])?;
        let name = member_name(&header)?;
        names.register(&name, kind == EntryKind::Regular)?;
        expanded = expanded
            .checked_add(size)
            .filter(|total| *total <= EXPANDED_ARCHIVE_BOUND)
            .ok_or(NativeOpenCode2InstallError::ArchiveInvalid)?;
        if name == target_name {
            if kind != EntryKind::Regular || target_found || size != target_size {
                return Err(NativeOpenCode2InstallError::ArchiveTargetInvalid);
            }
            let mut output = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(target_path)
                .map_err(|_| NativeOpenCode2InstallError::ArchiveTargetInvalid)?;
            copy_exact(reader, &mut output, size)?;
            output
                .flush()
                .and_then(|()| output.sync_all())
                .map_err(|_| NativeOpenCode2InstallError::ArchiveTargetInvalid)?;
            target_found = true;
        } else {
            discard_exact(reader, size)?;
        }
        discard_padding(reader, size)?;
    }
    if target_found {
        Ok(())
    } else {
        Err(NativeOpenCode2InstallError::ArchiveTargetMissing)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EntryKind {
    Regular,
    Directory,
}

fn validate_header(header: &[u8; 512]) -> Result<(), NativeOpenCode2InstallError> {
    let stored = parse_octal(&header[148..156])?;
    let calculated = header
        .iter()
        .enumerate()
        .map(|(index, byte)| {
            if (148..156).contains(&index) {
                u64::from(b' ')
            } else {
                u64::from(*byte)
            }
        })
        .sum::<u64>();
    if stored != calculated {
        return Err(NativeOpenCode2InstallError::ArchiveInvalid);
    }
    for field in [
        &header[100..108],
        &header[108..116],
        &header[116..124],
        &header[124..136],
        &header[136..148],
    ] {
        parse_octal(field)?;
    }
    Ok(())
}

fn parse_octal(field: &[u8]) -> Result<u64, NativeOpenCode2InstallError> {
    let mut value = 0_u64;
    let mut saw_digit = false;
    let mut ended = false;
    for byte in field {
        match *byte {
            b'0'..=b'7' if !ended => {
                saw_digit = true;
                value = value
                    .checked_mul(8)
                    .and_then(|value| value.checked_add(u64::from(*byte - b'0')))
                    .ok_or(NativeOpenCode2InstallError::ArchiveInvalid)?;
            }
            b' ' | 0 => {
                if saw_digit {
                    ended = true;
                }
            }
            _ => return Err(NativeOpenCode2InstallError::ArchiveInvalid),
        }
    }
    if saw_digit {
        Ok(value)
    } else {
        Err(NativeOpenCode2InstallError::ArchiveInvalid)
    }
}

fn member_name(header: &[u8; 512]) -> Result<String, NativeOpenCode2InstallError> {
    let prefix = text_field(&header[345..500])?;
    let name = text_field(&header[0..100])?;
    let combined = if prefix.is_empty() {
        name
    } else {
        format!("{prefix}/{name}")
    };
    if !is_safe_member_name(&combined) {
        return Err(NativeOpenCode2InstallError::ArchiveInvalid);
    }
    Ok(combined)
}

fn text_field(field: &[u8]) -> Result<String, NativeOpenCode2InstallError> {
    let end = field.iter().position(|byte| *byte == 0).unwrap_or(field.len());
    if field[end..]
        .iter()
        .any(|byte| *byte != 0)
    {
        return Err(NativeOpenCode2InstallError::ArchiveInvalid);
    }
    String::from_utf8(field[..end].to_vec())
        .map_err(|_| NativeOpenCode2InstallError::ArchiveInvalid)
}

fn is_safe_member_name(name: &str) -> bool {
    if name.is_empty()
        || name.len() > MAX_MEMBER_NAME_BYTES
        || name.starts_with('/')
        || name.contains('\\')
        || name.chars().any(char::is_control)
    {
        return false;
    }
    for component in name.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            return false;
        }
        if component.contains(':') {
            return false;
        }
    }
    true
}

#[derive(Default)]
struct ArchiveNames {
    entries: HashSet<String>,
    files: HashSet<String>,
}

impl ArchiveNames {
    fn register(&mut self, name: &str, is_file: bool) -> Result<(), NativeOpenCode2InstallError> {
        let folded = fold_name(name);
        if !self.entries.insert(folded.clone()) {
            return Err(NativeOpenCode2InstallError::ArchiveInvalid);
        }
        let mut prefix = String::new();
        let mut components = name.split('/').peekable();
        while let Some(component) = components.next() {
            if components.peek().is_some() {
                if !prefix.is_empty() {
                    prefix.push('/');
                }
                prefix.push_str(component);
                if self.files.contains(&fold_name(&prefix)) {
                    return Err(NativeOpenCode2InstallError::ArchiveInvalid);
                }
            }
        }
        if is_file
            && self.entries.iter().any(|entry| {
                entry
                    .strip_prefix(folded.as_str())
                    .is_some_and(|suffix| suffix.starts_with('/'))
            })
        {
            return Err(NativeOpenCode2InstallError::ArchiveInvalid);
        }
        if is_file {
            self.files.insert(folded);
        }
        Ok(())
    }
}

fn fold_name(name: &str) -> String {
    name.chars().flat_map(char::to_lowercase).collect()
}

fn copy_exact<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    mut remaining: u64,
) -> Result<(), NativeOpenCode2InstallError> {
    let mut buffer = vec![0_u8; 64 * 1024];
    while remaining > 0 {
        let length = usize::try_from(remaining.min(buffer.len() as u64))
            .map_err(|_| NativeOpenCode2InstallError::ArchiveInvalid)?;
        let read = reader
            .read(&mut buffer[..length])
            .map_err(|_| NativeOpenCode2InstallError::ArchiveInvalid)?;
        if read == 0 {
            return Err(NativeOpenCode2InstallError::ArchiveInvalid);
        }
        writer
            .write_all(&buffer[..read])
            .map_err(|_| NativeOpenCode2InstallError::ArchiveTargetInvalid)?;
        remaining -= read as u64;
    }
    Ok(())
}

fn discard_exact<R: Read>(
    reader: &mut R,
    mut remaining: u64,
) -> Result<(), NativeOpenCode2InstallError> {
    let mut buffer = vec![0_u8; 64 * 1024];
    while remaining > 0 {
        let length = usize::try_from(remaining.min(buffer.len() as u64))
            .map_err(|_| NativeOpenCode2InstallError::ArchiveInvalid)?;
        let read = reader
            .read(&mut buffer[..length])
            .map_err(|_| NativeOpenCode2InstallError::ArchiveInvalid)?;
        if read == 0 {
            return Err(NativeOpenCode2InstallError::ArchiveInvalid);
        }
        remaining -= read as u64;
    }
    Ok(())
}

fn discard_padding<R: Read>(reader: &mut R, size: u64) -> Result<(), NativeOpenCode2InstallError> {
    let padding = (512 - (size % 512)) % 512;
    let mut buffer = [0_u8; 512];
    let mut remaining = padding;
    while remaining > 0 {
        let length =
            usize::try_from(remaining).map_err(|_| NativeOpenCode2InstallError::ArchiveInvalid)?;
        reader
            .read_exact(&mut buffer[..length])
            .map_err(|_| NativeOpenCode2InstallError::ArchiveInvalid)?;
        if buffer[..length].iter().any(|byte| *byte != 0) {
            return Err(NativeOpenCode2InstallError::ArchiveInvalid);
        }
        remaining -= length as u64;
    }
    Ok(())
}

fn verify_executable(
    path: &Path,
    spec: &NativeOpenCode2InstallSpec,
) -> Result<NativeFileId, NativeOpenCode2InstallError> {
    instance::verify_native_file(path, spec.executable_size_bytes(), spec.executable_sha256())
        .map_err(|error| map_executable_error(&error))
}

fn map_executable_error(error: &NativeInstanceError) -> NativeOpenCode2InstallError {
    match error {
        NativeInstanceError::NotFound
        | NativeInstanceError::TooLarge
        | NativeInstanceError::FileSizeMismatch
        | NativeInstanceError::FileHashMismatch
        | NativeInstanceError::FileChanged
        | NativeInstanceError::InvalidPath(_)
        | NativeInstanceError::Io { .. }
        | NativeInstanceError::InvalidManifest
        | NativeInstanceError::UnsafePath(_) => NativeOpenCode2InstallError::ExecutableInvalid,
    }
}

fn verify_activation(
    authority: &NativeOpenCode2Authority,
    instance: &NativeInstanceConfig,
    paths: &InstallPaths,
    generation_id: &str,
    staged_id: NativeFileId,
    uncertain: bool,
) -> Result<(), NativeOpenCode2InstallError> {
    let failure = if uncertain {
        NativeOpenCode2InstallError::PostCommitUncertain
    } else {
        NativeOpenCode2InstallError::ActivationUnverified
    };
    if authority
        .read_install_state(&paths.engine_root)
        .map_err(|_| failure)?
        .is_none()
    {
        return Err(failure);
    }
    let resolved = authority.resolve_active(instance).map_err(|_| failure)?;
    if resolved.generation_id() != generation_id {
        return Err(failure);
    }
    let spec = NativeOpenCode2Authority::certified_install_spec();
    let executable = paths
        .versions_root
        .join(generation_id)
        .join(spec.binary());
    let published_id = verify_executable(&executable, &spec).map_err(|_| failure)?;
    if published_id != staged_id {
        return Err(failure);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, write::GzEncoder};
    use std::io::Cursor;

    #[test]
    fn names_are_exact_lowercase_hex_and_member_paths_are_never_joined() {
        assert!(is_staging_name("staging-0123456789abcdef0123456789abcdef"));
        assert!(is_generation_name(
            "generation-0123456789abcdef0123456789abcdef"
        ));
        assert!(!is_staging_name(
            "staging-0123456789ABCDEF0123456789abcdef"
        ));
        assert!(!is_generation_name("generation-0123456789abcdef"));
        for unsafe_name in [
            "",
            "/package/bin/opencode2.exe",
            "C:/package/bin/opencode2.exe",
            "package\\bin\\opencode2.exe",
            "package/../opencode2.exe",
            "package//opencode2.exe",
            "package/bin/./opencode2.exe",
        ] {
            assert!(!is_safe_member_name(unsafe_name), "{unsafe_name}");
        }
        assert!(is_safe_member_name("package/bin/opencode2.exe"));
    }

    #[test]
    fn bounded_copy_enforces_declared_and_streamed_bounds() {
        let mut output = Vec::new();
        let mut reader = Cursor::new(b"12345".to_vec());
        assert_eq!(
            copy_bounded(&mut reader, &mut output, Some(6), 5),
            Err(NativeOpenCode2InstallError::DownloadTooLarge)
        );
        let mut output = Vec::new();
        let mut reader = Cursor::new(b"12345".to_vec());
        assert_eq!(
            copy_bounded(&mut reader, &mut output, Some(4), 5),
            Err(NativeOpenCode2InstallError::DownloadFailed)
        );
        let mut output = Vec::new();
        let mut reader = Cursor::new(b"123456".to_vec());
        assert_eq!(
            copy_bounded(&mut reader, &mut output, None, 5),
            Err(NativeOpenCode2InstallError::DownloadTooLarge)
        );
        let mut output = Vec::new();
        let mut reader = Cursor::new(b"12345".to_vec());
        let digest = copy_bounded(&mut reader, &mut output, Some(5), 5).unwrap();
        let expected = Sha512::digest(b"12345");
        let mut expected_digest = [0_u8; 64];
        expected_digest.copy_from_slice(&expected);
        assert_eq!(digest, expected_digest);
        assert_eq!(output, b"12345");
    }

    #[test]
    fn integrity_decoding_is_the_certified_sha512_length() {
        let spec = NativeOpenCode2Authority::certified_install_spec();
        assert_eq!(certified_integrity(&spec).unwrap().len(), 64);
    }

    #[test]
    fn compressed_integrity_requires_exact_certified_digest() {
        let spec = NativeOpenCode2Authority::certified_install_spec();
        let expected = certified_integrity(&spec).unwrap();
        assert_eq!(verify_compressed_integrity(&expected, &expected), Ok(()));
        let mut actual = expected;
        actual[0] ^= 1;
        assert_eq!(
            verify_compressed_integrity(&actual, &expected),
            Err(NativeOpenCode2InstallError::IntegrityMismatch)
        );
    }

    #[test]
    fn archive_reader_accepts_only_the_exact_target_and_rejects_hostile_entries() {
        let target = b"target executable";
        let target_directory = tempfile::tempdir().unwrap();
        let target_path = target_directory.path().join("opencode2.exe");
        for name in [
            "package/bin/../opencode2.exe",
            "package\\bin\\opencode2.exe",
            "C:/package/bin/opencode2.exe",
        ] {
            let archive = tar_gzip(&[(name, b"bad".as_slice(), b'0')]);
            let mut decoder = GzDecoder::new(Cursor::new(archive));
            assert_eq!(
                parse_tar(&mut decoder, "package/bin/opencode2.exe", &target_path, 3),
                Err(NativeOpenCode2InstallError::ArchiveInvalid)
            );
        }
        let archive = tar_gzip(&[("package/bin/opencode2.exe", target, b'0')]);
        let mut decoder = GzDecoder::new(Cursor::new(archive));
        assert!(parse_tar(
            &mut decoder,
            "package/bin/opencode2.exe",
            &target_path,
            target.len() as u64
        )
        .is_ok());

        let archive = tar_gzip(&[(
            "package/bin/opencode2.exe",
            b"".as_slice(),
            b'5',
        )]);
        let mut decoder = GzDecoder::new(Cursor::new(archive));
        assert_eq!(
            parse_tar(
                &mut decoder,
                "package/bin/opencode2.exe",
                &target_path,
                0,
            ),
            Err(NativeOpenCode2InstallError::ArchiveTargetInvalid)
        );
    }

    #[test]
    fn archive_reader_rejects_unsupported_tar_entry_kinds() {
        let target_directory = tempfile::tempdir().unwrap();
        let target_path = target_directory.path().join("opencode2.exe");
        for kind in [b'1', b'2', b'x', b'g', b'L', b'3'] {
            let archive = tar_gzip(&[("unsupported", b"".as_slice(), kind)]);
            let mut decoder = GzDecoder::new(Cursor::new(archive));
            assert_eq!(
                parse_tar(&mut decoder, "package/bin/opencode2.exe", &target_path, 0),
                Err(NativeOpenCode2InstallError::ArchiveInvalid),
                "type flag {kind}"
            );
        }
    }

    #[test]
    fn archive_reader_rejects_duplicates_case_collisions_and_ancestors() {
        for entries in [
            vec![
                ("package/bin/opencode2.exe", b"a".as_slice(), b'0'),
                ("package/bin/opencode2.exe", b"b".as_slice(), b'0'),
            ],
            vec![
                ("package/bin/opencode2.exe", b"a".as_slice(), b'0'),
                ("PACKAGE/BIN/OPENCODE2.EXE", b"b".as_slice(), b'0'),
            ],
            vec![
                ("package", b"a".as_slice(), b'0'),
                ("package/bin/opencode2.exe", b"b".as_slice(), b'0'),
            ],
        ] {
            let archive = tar_gzip(&entries);
            let mut decoder = GzDecoder::new(Cursor::new(archive));
            let target_directory = tempfile::tempdir().unwrap();
            let target_path = target_directory.path().join("target");
            assert_eq!(
                parse_tar(&mut decoder, "package/bin/opencode2.exe", &target_path, 1),
                Err(NativeOpenCode2InstallError::ArchiveInvalid)
            );
        }
    }

    #[test]
    fn archive_reader_rejects_entry_count_overflow_without_payloads() {
        let archive = tar_many_entries(MAX_ARCHIVE_ENTRIES + 1);
        let mut reader = Cursor::new(archive);
        let target_directory = tempfile::tempdir().unwrap();
        let target_path = target_directory.path().join("opencode2.exe");
        assert_eq!(
            parse_tar(&mut reader, "package/bin/opencode2.exe", &target_path, 0),
            Err(NativeOpenCode2InstallError::ArchiveInvalid)
        );
    }

    #[test]
    fn archive_reader_rejects_expanded_size_overflow_before_reading_payload() {
        let archive = tar_declared_size("oversized", EXPANDED_ARCHIVE_BOUND + 1, b'0');
        let mut reader = Cursor::new(archive);
        let target_directory = tempfile::tempdir().unwrap();
        let target_path = target_directory.path().join("opencode2.exe");
        assert_eq!(
            parse_tar(&mut reader, "package/bin/opencode2.exe", &target_path, 0),
            Err(NativeOpenCode2InstallError::ArchiveInvalid)
        );
    }

    #[test]
    fn archive_reader_rejects_missing_and_mismatched_target() {
        let target_directory = tempfile::tempdir().unwrap();
        let target_path = target_directory.path().join("opencode2.exe");

        let archive = tar_gzip(&[("package/other", b"a".as_slice(), b'0')]);
        let mut decoder = GzDecoder::new(Cursor::new(archive));
        assert_eq!(
            parse_tar(&mut decoder, "package/bin/opencode2.exe", &target_path, 1),
            Err(NativeOpenCode2InstallError::ArchiveTargetMissing)
        );

        let archive = tar_gzip(&[("package/bin/opencode2.exe", b"a".as_slice(), b'0')]);
        let mut decoder = GzDecoder::new(Cursor::new(archive));
        assert_eq!(
            parse_tar(&mut decoder, "package/bin/opencode2.exe", &target_path, 2),
            Err(NativeOpenCode2InstallError::ArchiveTargetInvalid)
        );
    }

    #[test]
    fn archive_reader_rejects_bad_checksum_truncation_and_missing_terminal_block() {
        let mut decoded = tar_bytes(&[("package/bin/opencode2.exe", b"a".as_slice(), b'0')]);
        decoded[0] ^= 1;
        let mut decoder = GzDecoder::new(Cursor::new(gzip(&decoded)));
        let target_directory = tempfile::tempdir().unwrap();
        let target_path = target_directory.path().join("target");
        assert!(parse_tar(&mut decoder, "package/bin/opencode2.exe", &target_path, 1).is_err());

        let archive = tar_gzip(&[("package/bin/opencode2.exe", b"a".as_slice(), b'0')]);
        let mut decoded = Vec::new();
        GzDecoder::new(Cursor::new(archive))
            .read_to_end(&mut decoded)
            .unwrap();
        decoded.truncate(decoded.len() - 512);
        let mut decoder = GzDecoder::new(Cursor::new(gzip(&decoded)));
        assert!(parse_tar(&mut decoder, "package/bin/opencode2.exe", &target_path, 1).is_err());
    }

    #[test]
    fn archive_reader_rejects_trailing_compressed_data() {
        let first = gzip(&tar_bytes(&[(
            "package/bin/opencode2.exe",
            b"a".as_slice(),
            b'0',
        )]));
        let second = gzip(b"trailing");
        let mut decoder = GzDecoder::new(Cursor::new([first, second].concat()));
        let target_directory = tempfile::tempdir().unwrap();
        let target_path = target_directory.path().join("target");
        assert!(parse_tar(&mut decoder, "package/bin/opencode2.exe", &target_path, 1).is_ok());
        assert!(matches!(
            ensure_gzip_end(decoder),
            Err(NativeOpenCode2InstallError::ArchiveInvalid)
        ));
    }

    #[test]
    fn archive_reader_requires_gzip_crc_completion() {
        let mut archive = gzip(&tar_bytes(&[(
            "package/bin/opencode2.exe",
            b"a".as_slice(),
            b'0',
        )]));
        let crc_byte = archive.len() - 8;
        archive[crc_byte] ^= 1;
        let mut decoder = GzDecoder::new(Cursor::new(archive));
        let target_directory = tempfile::tempdir().unwrap();
        let target_path = target_directory.path().join("target");
        assert!(parse_tar(&mut decoder, "package/bin/opencode2.exe", &target_path, 1).is_ok());
        assert_eq!(
            ensure_gzip_end(decoder),
            Err(NativeOpenCode2InstallError::ArchiveInvalid)
        );
    }

    #[test]
    fn executable_size_and_hash_mismatches_map_to_one_path_free_error() {
        assert_eq!(
            map_executable_error(&NativeInstanceError::FileSizeMismatch),
            NativeOpenCode2InstallError::ExecutableInvalid
        );
        assert_eq!(
            map_executable_error(&NativeInstanceError::FileHashMismatch),
            NativeOpenCode2InstallError::ExecutableInvalid
        );
    }

    #[test]
    fn archive_custody_is_identity_fenced() {
        let directory = tempfile::tempdir().unwrap();
        let mut archive = Builder::new()
            .prefix(".opencode2-download-")
            .suffix(".tgz")
            .tempfile_in(directory.path())
            .unwrap();
        archive.as_file_mut().write_all(b"archive").unwrap();
        assert!(verify_archive_custody(&archive).is_ok());

        #[cfg(unix)]
        {
            let path = archive.path().to_path_buf();
            fs::remove_file(&path).unwrap();
            fs::write(&path, b"replacement").unwrap();
            assert_eq!(
                verify_archive_custody(&archive),
                Err(NativeOpenCode2InstallError::ArchiveIdentityChanged)
            );
        }
    }

    #[test]
    fn cleanup_removes_only_owned_staging_directories() {
        let (_root, paths, lock) = prepared_paths();
        let staging_path = paths
            .versions_root
            .join("staging-0123456789abcdef0123456789abcdef");
        fs::create_dir(&staging_path).unwrap();
        fs::write(staging_path.join("partial"), b"partial").unwrap();
        let generation_path = paths
            .versions_root
            .join("generation-0123456789abcdef0123456789abcdef");
        fs::create_dir(&generation_path).unwrap();
        fs::write(generation_path.join("sentinel"), b"keep").unwrap();
        let unrelated_path = paths.versions_root.join("staging-not-owned");
        fs::create_dir(&unrelated_path).unwrap();

        cleanup_staging(&paths, &lock).unwrap();

        assert!(!staging_path.exists());
        assert!(generation_path.join("sentinel").exists());
        assert!(unrelated_path.exists());
    }

    #[test]
    fn generation_collision_never_overwrites_existing_directory() {
        let (_root, paths, lock) = prepared_paths();
        let generation_id = "generation-0123456789abcdef0123456789abcdef";
        let destination = paths.versions_root.join(generation_id);
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("sentinel"), b"keep").unwrap();
        let staging_path = paths
            .versions_root
            .join("staging-fedcba9876543210fedcba9876543210");
        fs::create_dir(&staging_path).unwrap();
        fs::write(staging_path.join("opencode2.exe"), b"new").unwrap();
        let staging = StagingDirectory {
            path: staging_path.clone(),
            versions_root: paths.versions_root.clone(),
            published: false,
        };

        assert!(matches!(
            staging.publish(&paths, &lock, generation_id),
            Err(NativeOpenCode2InstallError::GenerationCollision)
        ));
        assert_eq!(fs::read(destination.join("sentinel")).unwrap(), b"keep");
        assert!(!staging_path.exists());
    }

    #[test]
    fn lock_rejects_a_directory_and_never_replaces_it() {
        let (_root, paths, lock) = prepared_paths();
        drop(lock);
        fs::remove_file(&paths.lock_path).unwrap();
        fs::create_dir(&paths.lock_path).unwrap();
        assert!(matches!(
            InstallLock::acquire(&paths),
            Err(NativeOpenCode2InstallError::LockUnavailable)
        ));
        assert!(paths.lock_path.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn lock_fence_rejects_path_identity_substitution() {
        let (_root, paths, lock) = prepared_paths();
        let replacement = paths.engine_root.join("replacement.lock");
        fs::write(&replacement, b"replacement").unwrap();
        fs::rename(&replacement, &paths.lock_path).unwrap();
        assert_eq!(
            lock.fence(&paths),
            Err(NativeOpenCode2InstallError::LockIdentityChanged)
        );
    }

    #[test]
    fn error_surfaces_do_not_contain_paths_or_urls() {
        let error = NativeOpenCode2InstallError::StateInvalid;
        assert!(!format!("{error}").contains("C:\\"));
        assert!(!format!("{error:?}").contains("registry.npmjs.org"));
    }

    fn tar_gzip(entries: &[(&str, &[u8], u8)]) -> Vec<u8> {
        gzip(&tar_bytes(entries))
    }

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    fn tar_bytes(entries: &[(&str, &[u8], u8)]) -> Vec<u8> {
        let mut archive = Vec::new();
        for (name, body, kind) in entries {
            let header = tar_header(name, body.len() as u64, *kind);
            archive.extend_from_slice(&header);
            archive.extend_from_slice(body);
            archive.resize(archive.len() + ((512 - body.len() % 512) % 512), 0);
        }
        archive.resize(archive.len() + 1024, 0);
        archive
    }

    fn tar_header(name: &str, size: u64, kind: u8) -> [u8; 512] {
        let mut header = [0_u8; 512];
        header[..name.len()].copy_from_slice(name.as_bytes());
        write_octal(&mut header[100..108], 0o644);
        write_octal(&mut header[108..116], 0);
        write_octal(&mut header[116..124], 0);
        write_octal(&mut header[124..136], size);
        write_octal(&mut header[136..148], 0);
        header[156] = kind;
        header[257..263].copy_from_slice(b"ustar\0");
        for byte in &mut header[148..156] {
            *byte = b' ';
        }
        let checksum = header.iter().map(|byte| u64::from(*byte)).sum();
        write_octal(&mut header[148..156], checksum);
        header
    }

    fn tar_many_entries(count: usize) -> Vec<u8> {
        let mut archive = Vec::with_capacity(count.saturating_mul(512).saturating_add(1024));
        for index in 0..count {
            let name = format!("entry-{index}");
            archive.extend_from_slice(&tar_header(&name, 0, b'0'));
        }
        archive.resize(archive.len() + 1024, 0);
        archive
    }

    fn tar_declared_size(name: &str, size: u64, kind: u8) -> Vec<u8> {
        let mut archive = tar_header(name, size, kind).to_vec();
        archive.resize(archive.len() + 1024, 0);
        archive
    }

    fn write_octal(field: &mut [u8], value: u64) {
        let text = format!("{value:o}");
        let start = field.len() - text.len() - 1;
        field.fill(0);
        field[start..start + text.len()].copy_from_slice(text.as_bytes());
        field[start + text.len()] = 0;
    }

    fn prepared_paths() -> (tempfile::TempDir, InstallPaths, InstallLock) {
        let root = tempfile::tempdir().unwrap();
        let database_parent = root.path().to_path_buf();
        let toolchain_root = database_parent.join("toolchain");
        let engine_root = toolchain_root.join("opencode2");
        let versions_root = engine_root.join("versions");
        fs::create_dir(&toolchain_root).unwrap();
        fs::create_dir(&engine_root).unwrap();
        fs::create_dir(&versions_root).unwrap();
        let paths = InstallPaths {
            database_parent,
            toolchain_root,
            engine_root: engine_root.clone(),
            versions_root,
            lock_path: engine_root.join("install.lock"),
        };
        let lock = InstallLock::acquire(&paths).unwrap();
        (root, paths, lock)
    }
}
