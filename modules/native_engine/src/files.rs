use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::Path,
};

use sha2::{Digest, Sha256};
use tempfile::Builder;

/// The bounded, path-free failures used by the native engine file authority.
///
/// Paths and operating-system error text are deliberately not retained. The
/// callers can classify these failures without making untrusted filesystem
/// data part of a diagnostic.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeFileError {
    NotFound,
    TooLarge,
    FileChanged,
    FileSizeMismatch,
    FileHashMismatch,
    Io,
    UnsafePath,
    PrivatePermissions,
}

impl std::fmt::Display for NativeFileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::NotFound => "native file is not present",
            Self::TooLarge => "native file exceeds its read bound",
            Self::FileChanged => "native file changed while it was read",
            Self::FileSizeMismatch => "native file size does not match",
            Self::FileHashMismatch => "native file hash does not match",
            Self::Io => "native engine file I/O failed",
            Self::UnsafePath => "native engine path is unsafe",
            Self::PrivatePermissions => "native engine private directory permissions are invalid",
        })
    }
}

impl std::error::Error for NativeFileError {}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct VerifiedFileIdentity {
    inner: NativeFileIdentity,
}

impl std::fmt::Debug for VerifiedFileIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VerifiedFileIdentity")
            .finish_non_exhaustive()
    }
}

impl std::fmt::Display for VerifiedFileIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("verified file identity")
    }
}

impl VerifiedFileIdentity {
    pub(crate) const fn new(inner: NativeFileIdentity) -> Self {
        Self { inner }
    }
}

#[cfg(any(unix, windows))]
#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) struct NativeFileIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(windows)]
    volume: u64,
    #[cfg(windows)]
    index: u64,
}

impl std::fmt::Debug for NativeFileIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("NativeFileIdentity(REDACTED)")
    }
}

#[cfg(not(any(unix, windows)))]
#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) struct NativeFileIdentity;

#[cfg(not(any(unix, windows)))]
impl std::fmt::Debug for NativeFileIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("NativeFileIdentity(REDACTED)")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AtomicReplaceOutcome {
    Committed,
    CommittedButUnverified,
}

pub(crate) fn metadata_is_symlink_or_reparse(meta: &fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

fn check_absolute(path: &Path) -> Result<(), NativeFileError> {
    if path.as_os_str().is_empty() || !path.is_absolute() {
        Err(NativeFileError::UnsafePath)
    } else {
        Ok(())
    }
}

fn check_ancestors_all(path: &Path, must_exist: bool) -> Result<(), NativeFileError> {
    check_absolute(path)?;
    let parent = path.parent().unwrap_or(path);
    for ancestor in parent.ancestors() {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        match fs::symlink_metadata(ancestor) {
            Ok(meta) => {
                if metadata_is_symlink_or_reparse(&meta) || !meta.is_dir() {
                    return Err(NativeFileError::UnsafePath);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && !must_exist => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(NativeFileError::Io);
            }
            Err(_) => return Err(NativeFileError::Io),
        }
    }
    Ok(())
}

pub fn verify_directory(path: &Path) -> Result<(), NativeFileError> {
    check_ancestors_all(path, true)?;
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            NativeFileError::NotFound
        } else {
            NativeFileError::Io
        }
    })?;
    if metadata_is_symlink_or_reparse(&metadata) || !metadata.is_dir() {
        return Err(NativeFileError::UnsafePath);
    }
    Ok(())
}

pub fn ensure_directory(path: &Path) -> Result<(), NativeFileError> {
    check_absolute(path)?;
    let parent = path.parent().ok_or(NativeFileError::UnsafePath)?;
    verify_directory(parent)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata_is_symlink_or_reparse(&metadata) || !metadata.is_dir() {
                return Err(NativeFileError::UnsafePath);
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path).map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    NativeFileError::UnsafePath
                } else {
                    NativeFileError::Io
                }
            })?;
        }
        Err(_) => return Err(NativeFileError::Io),
    }
    verify_directory(path)
}

fn native_file_id(path: &Path) -> Result<NativeFileIdentity, NativeFileError> {
    let file = File::open(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            NativeFileError::NotFound
        } else {
            NativeFileError::Io
        }
    })?;
    native_file_id_from_file(&file)
}

fn native_file_id_from_file(file: &File) -> Result<NativeFileIdentity, NativeFileError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        let metadata = file.metadata().map_err(|_| NativeFileError::Io)?;
        return Ok(NativeFileIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        });
    }
    #[cfg(windows)]
    {
        let info = winapi_util::file::information(winapi_util::HandleRef::from_file(file))
            .map_err(|_| NativeFileError::Io)?;
        let volume = info.volume_serial_number();
        let index = info.file_index();
        if volume == 0 && index == 0 {
            return Err(NativeFileError::Io);
        }
        return Ok(NativeFileIdentity { volume, index });
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = file;
        Err(NativeFileError::Io)
    }
}

pub(crate) fn file_identity(file: &File) -> Result<NativeFileIdentity, NativeFileError> {
    native_file_id_from_file(file)
}

pub(crate) fn path_identity(path: &Path) -> Result<NativeFileIdentity, NativeFileError> {
    native_file_id(path)
}

pub(crate) fn verify_regular_file(path: &Path) -> Result<(), NativeFileError> {
    check_ancestors_all(path, false)?;
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            NativeFileError::NotFound
        } else {
            NativeFileError::Io
        }
    })?;
    if metadata_is_symlink_or_reparse(&metadata) || !metadata.is_file() {
        return Err(NativeFileError::UnsafePath);
    }
    Ok(())
}

pub fn read_bounded(path: &Path, maximum_bytes: usize) -> Result<Vec<u8>, NativeFileError> {
    open_and_read_bounded(path, maximum_bytes)
}

fn open_and_read_bounded(path: &Path, maximum_bytes: usize) -> Result<Vec<u8>, NativeFileError> {
    check_ancestors_all(path, false)?;
    let pre_metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(NativeFileError::NotFound);
        }
        Err(_) => return Err(NativeFileError::Io),
    };
    if metadata_is_symlink_or_reparse(&pre_metadata) || !pre_metadata.is_file() {
        return Err(NativeFileError::UnsafePath);
    }
    check_ancestors_all(path, true)?;
    let pre_id = native_file_id(path)?;
    let mut file = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|_| NativeFileError::Io)?;
    let handle_metadata = file.metadata().map_err(|_| NativeFileError::Io)?;
    if metadata_is_symlink_or_reparse(&handle_metadata) || !handle_metadata.is_file() {
        return Err(NativeFileError::UnsafePath);
    }
    let handle_id = native_file_id_from_file(&file)?;
    if handle_id != pre_id {
        return Err(NativeFileError::FileChanged);
    }
    let read_limit = maximum_bytes.saturating_add(1);
    let mut bytes = Vec::new();
    std::io::Read::by_ref(&mut file)
        .take(u64::try_from(read_limit).unwrap_or(u64::MAX))
        .read_to_end(&mut bytes)
        .map_err(|_| NativeFileError::Io)?;
    check_ancestors_all(path, true)?;
    let post_metadata = fs::symlink_metadata(path).map_err(|_| NativeFileError::Io)?;
    if metadata_is_symlink_or_reparse(&post_metadata) || !post_metadata.is_file() {
        return Err(NativeFileError::UnsafePath);
    }
    let post_id = native_file_id(path)?;
    if post_id != pre_id || post_id != handle_id {
        return Err(NativeFileError::FileChanged);
    }
    if bytes.len() > maximum_bytes {
        return Err(NativeFileError::TooLarge);
    }
    Ok(bytes)
}

pub fn verify_file(
    path: &Path,
    expected_size: u64,
    expected_sha256: &[u8; 32],
) -> Result<VerifiedFileIdentity, NativeFileError> {
    check_ancestors_all(path, false)?;
    let pre_metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(NativeFileError::NotFound);
        }
        Err(_) => return Err(NativeFileError::Io),
    };
    if metadata_is_symlink_or_reparse(&pre_metadata) || !pre_metadata.is_file() {
        return Err(NativeFileError::UnsafePath);
    }
    if pre_metadata.len() != expected_size {
        return Err(NativeFileError::FileSizeMismatch);
    }
    check_ancestors_all(path, true)?;
    let pre_id = native_file_id(path)?;
    let mut file = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|_| NativeFileError::FileChanged)?;
    let handle_metadata = file.metadata().map_err(|_| NativeFileError::FileChanged)?;
    if metadata_is_symlink_or_reparse(&handle_metadata) || !handle_metadata.is_file() {
        return Err(NativeFileError::UnsafePath);
    }
    if handle_metadata.len() != expected_size {
        return Err(NativeFileError::FileSizeMismatch);
    }
    let handle_id = native_file_id_from_file(&file)?;
    if handle_id != pre_id {
        return Err(NativeFileError::FileChanged);
    }

    let stream_result = stream_and_verify(&mut file, expected_size, expected_sha256);

    let post_handle_metadata = file.metadata().map_err(|_| NativeFileError::FileChanged)?;
    if metadata_is_symlink_or_reparse(&post_handle_metadata) || !post_handle_metadata.is_file() {
        return Err(NativeFileError::UnsafePath);
    }
    if post_handle_metadata.len() != expected_size {
        return Err(NativeFileError::FileSizeMismatch);
    }
    let post_handle_id = native_file_id_from_file(&file)?;
    if post_handle_id != pre_id {
        return Err(NativeFileError::FileChanged);
    }

    check_ancestors_all(path, true)?;
    let post_metadata = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            NativeFileError::FileChanged
        } else {
            NativeFileError::Io
        }
    })?;
    if metadata_is_symlink_or_reparse(&post_metadata) || !post_metadata.is_file() {
        return Err(NativeFileError::UnsafePath);
    }
    if post_metadata.len() != expected_size {
        return Err(NativeFileError::FileSizeMismatch);
    }
    let post_id = native_file_id(path)?;
    if post_id != pre_id {
        return Err(NativeFileError::FileChanged);
    }
    stream_result?;
    Ok(VerifiedFileIdentity::new(pre_id))
}

fn stream_and_verify(
    reader: &mut File,
    expected_size: u64,
    expected_sha256: &[u8; 32],
) -> Result<(), NativeFileError> {
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer).map_err(|_| NativeFileError::Io)?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or(NativeFileError::FileSizeMismatch)?;
        if total > expected_size {
            return Err(NativeFileError::FileSizeMismatch);
        }
        hasher.update(&buffer[..read]);
    }
    if total != expected_size {
        return Err(NativeFileError::FileSizeMismatch);
    }
    let digest = hasher.finalize();
    if digest[..] != expected_sha256[..] {
        return Err(NativeFileError::FileHashMismatch);
    }
    Ok(())
}

fn inspect_destination(path: &Path) -> Result<Option<NativeFileIdentity>, NativeFileError> {
    check_ancestors_all(path, false)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata_is_symlink_or_reparse(&metadata) || metadata.is_dir() => {
            Err(NativeFileError::UnsafePath)
        }
        Ok(metadata) if metadata.is_file() => native_file_id(path).map(Some),
        Ok(_) => Err(NativeFileError::UnsafePath),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(NativeFileError::Io),
    }
}

fn verify_destination(
    path: &Path,
    previous: Option<NativeFileIdentity>,
) -> Result<(), NativeFileError> {
    check_ancestors_all(path, false)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata_is_symlink_or_reparse(&metadata) || metadata.is_dir() => {
            Err(NativeFileError::UnsafePath)
        }
        Ok(metadata) if metadata.is_file() => {
            if let Some(expected) = previous {
                if native_file_id(path)? == expected {
                    Ok(())
                } else {
                    Err(NativeFileError::UnsafePath)
                }
            } else {
                Err(NativeFileError::UnsafePath)
            }
        }
        Ok(_) => Err(NativeFileError::UnsafePath),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if previous.is_some() {
                Err(NativeFileError::UnsafePath)
            } else {
                Ok(())
            }
        }
        Err(_) => Err(NativeFileError::Io),
    }
}

fn sync_directory(directory: &Path) -> Result<(), NativeFileError> {
    #[cfg(unix)]
    {
        File::open(directory)
            .and_then(|file| file.sync_all())
            .map_err(|_| NativeFileError::Io)?;
    }
    #[cfg(windows)]
    {
        let metadata = fs::symlink_metadata(directory).map_err(|_| NativeFileError::Io)?;
        if metadata_is_symlink_or_reparse(&metadata) || !metadata.is_dir() {
            return Err(NativeFileError::UnsafePath);
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        File::open(directory)
            .and_then(|file| file.sync_all())
            .map_err(|_| NativeFileError::Io)?;
    }
    Ok(())
}

pub fn replace_file(path: &Path, bytes: &[u8]) -> Result<AtomicReplaceOutcome, NativeFileError> {
    let directory = path.parent().ok_or(NativeFileError::UnsafePath)?;
    check_ancestors_all(path, true)?;
    let previous = inspect_destination(path)?;
    let mut temporary = Builder::new()
        .prefix(".artisan-native-engine-")
        .suffix(".tmp")
        .tempfile_in(directory)
        .map_err(|_| NativeFileError::Io)?;
    if temporary.path().parent() != Some(directory) {
        return Err(NativeFileError::UnsafePath);
    }
    temporary
        .as_file_mut()
        .write_all(bytes)
        .map_err(|_| NativeFileError::Io)?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|_| NativeFileError::Io)?;
    let temporary_id = native_file_id_from_file(temporary.as_file())?;
    verify_temporary(temporary.path(), temporary_id)?;
    check_ancestors_all(path, true)?;
    verify_destination(path, previous)?;
    verify_temporary(temporary.path(), temporary_id)?;
    let Ok(persisted) = temporary.persist(path) else {
        return Err(NativeFileError::Io);
    };
    drop(persisted);

    let verified = (|| {
        check_ancestors_all(path, true)?;
        let destination_id = inspect_destination(path)?.ok_or(NativeFileError::NotFound)?;
        if destination_id != temporary_id {
            return Err(NativeFileError::UnsafePath);
        }
        sync_directory(directory)
    })();
    if verified.is_ok() {
        Ok(AtomicReplaceOutcome::Committed)
    } else {
        Ok(AtomicReplaceOutcome::CommittedButUnverified)
    }
}

fn verify_temporary(path: &Path, expected: NativeFileIdentity) -> Result<(), NativeFileError> {
    if inspect_destination(path)? == Some(expected) {
        Ok(())
    } else {
        Err(NativeFileError::UnsafePath)
    }
}

pub fn ensure_private_directory(path: &Path) -> Result<(), NativeFileError> {
    check_absolute(path)?;
    check_ancestors_all(path, false)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata_is_symlink_or_reparse(&metadata) => {
            Err(NativeFileError::UnsafePath)
        }
        Ok(metadata) if metadata.is_dir() => validate_private_directory(path),
        Ok(_) => Err(NativeFileError::UnsafePath),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path).map_err(|_| NativeFileError::Io)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;

                fs::set_permissions(path, fs::Permissions::from_mode(0o700))
                    .map_err(|_| NativeFileError::Io)?;
                sync_directory(path.parent().unwrap_or(Path::new("/")))?;
            }
            #[cfg(windows)]
            crate::windows_private::restrict_directory(path)?;
            validate_private_directory(path)
        }
        Err(_) => Err(NativeFileError::Io),
    }
}

pub fn validate_private_directory(path: &Path) -> Result<(), NativeFileError> {
    check_ancestors_all(path, true)?;
    let metadata = fs::symlink_metadata(path).map_err(|_| NativeFileError::Io)?;
    if metadata_is_symlink_or_reparse(&metadata) || !metadata.is_dir() {
        return Err(NativeFileError::UnsafePath);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        if metadata.permissions().mode() & 0o777 != 0o700 {
            return Err(NativeFileError::PrivatePermissions);
        }
    }
    #[cfg(windows)]
    crate::windows_private::validate_directory(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_errors_and_identity_debug_are_redacted() {
        assert_eq!(
            NativeFileError::FileHashMismatch.to_string(),
            "native file hash does not match"
        );
        let identity = VerifiedFileIdentity::new(NativeFileIdentity {
            #[cfg(unix)]
            device: 7,
            #[cfg(unix)]
            inode: 9,
            #[cfg(windows)]
            volume: 7,
            #[cfg(windows)]
            index: 9,
        });
        assert!(!format!("{identity:?}").contains('7'));
        assert_eq!(identity.to_string(), "verified file identity");
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn verification_rejects_size_hash_and_replacement_changes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("binary");
        let original = b"original";
        fs::write(&path, original).unwrap();
        let expected = Sha256::digest(original);
        let mut digest = [0_u8; 32];
        digest.copy_from_slice(&expected);
        let identity = verify_file(&path, original.len() as u64, &digest).unwrap();
        assert_eq!(
            verify_file(&path, (original.len() + 1) as u64, &digest),
            Err(NativeFileError::FileSizeMismatch)
        );
        assert_eq!(
            verify_file(&path, original.len() as u64, &[0; 32]),
            Err(NativeFileError::FileHashMismatch)
        );
        fs::rename(&path, directory.path().join("old")).unwrap();
        fs::write(&path, original).unwrap();
        let replacement = verify_file(&path, original.len() as u64, &digest).unwrap();
        assert_ne!(identity, replacement);
    }
}
