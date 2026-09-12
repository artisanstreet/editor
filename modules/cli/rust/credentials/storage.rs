use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use fs2::FileExt;
use zeroize::Zeroizing;

use super::keychain::RECONNECT_CAPABILITY_FILENAME;
#[cfg(windows)]
use super::validation::{
    hidden_output, resolve_current_identity, restrict_directory_windows, restrict_file_windows,
    verify_windows_dacl,
};
use super::{ForgeCredentialError, ForgeCredentialPaths};

#[cfg(all(test, windows))]
macro_rules! acl_diagnostic {
    ($expression:expr) => {
        $expression
    };
}

#[cfg(not(all(test, windows)))]
macro_rules! acl_diagnostic {
    ($expression:expr) => {};
}

pub(super) use acl_diagnostic;

pub(super) const MAX_MANIFEST_BYTES: usize = 4_096;

pub(super) const MAX_MANIFEST_READ_BYTES: usize = MAX_MANIFEST_BYTES + 1;

pub(super) const MAX_CAPABILITY_BYTES: usize = 32;

pub(super) const MAX_CAPABILITY_READ_BYTES: usize = MAX_CAPABILITY_BYTES + 1;

pub(super) const MAX_CERTIFICATE_BYTES: usize = 65_536;

pub(super) const MAX_CERTIFICATE_READ_BYTES: usize = MAX_CERTIFICATE_BYTES + 1;

pub(super) const MAX_PRIVATE_KEY_BYTES: usize = 65_536;

pub(super) const MAX_PRIVATE_KEY_READ_BYTES: usize = MAX_PRIVATE_KEY_BYTES + 1;

const SAFE_READ_CHUNK_BYTES: usize = 4_096;

pub(super) fn validate_home(home: &Path) -> Result<(), ForgeCredentialError> {
    if !home.is_absolute() {
        return Err(ForgeCredentialError::InvalidHome(home.to_path_buf()));
    }
    if home.as_os_str().is_empty() {
        return Err(ForgeCredentialError::InvalidHome(home.to_path_buf()));
    }
    Ok(())
}

pub(super) fn metadata_is_symlink_or_reparse(meta: &fs::Metadata) -> bool {
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

pub(super) fn check_ancestors_all(
    path: &Path,
    must_exist: bool,
) -> Result<(), ForgeCredentialError> {
    let parent = path.parent().unwrap_or(Path::new("/"));
    for ancestor in parent.ancestors() {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        match fs::symlink_metadata(ancestor) {
            Ok(meta) => {
                if metadata_is_symlink_or_reparse(&meta) {
                    return Err(ForgeCredentialError::UnsafePath(ancestor.to_path_buf()));
                }
                if !meta.is_dir() {
                    return Err(ForgeCredentialError::UnsafePath(ancestor.to_path_buf()));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if must_exist {
                    return Err(ForgeCredentialError::Io {
                        context: "inspect parent",
                        path: ancestor.to_path_buf(),
                    });
                }
            }
            Err(_) => {
                return Err(ForgeCredentialError::Io {
                    context: "inspect parent",
                    path: ancestor.to_path_buf(),
                });
            }
        }
    }
    Ok(())
}

pub(super) fn is_safe_filename(name: &str) -> bool {
    if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") {
        return false;
    }
    if name.contains(':') || name.contains('\0') {
        return false;
    }
    if Path::new(name).file_name().is_none_or(|base| base != name) {
        return false;
    }
    true
}

pub(super) fn encode_nonce_hex(nonce: &[u8; 16]) -> String {
    let mut encoded = String::with_capacity(32);
    for &byte in nonce {
        encoded.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('?'));
        encoded.push(char::from_digit(u32::from(byte & 0x0f), 16).unwrap_or('?'));
    }
    encoded
}

#[cfg(unix)]
fn check_dir_mode(path: &Path) -> Result<(), ForgeCredentialError> {
    use std::os::unix::fs::PermissionsExt;
    let meta = fs::symlink_metadata(path)
        .map_err(|_| ForgeCredentialError::UnsafePath(path.to_path_buf()))?;
    if metadata_is_symlink_or_reparse(&meta) {
        return Err(ForgeCredentialError::UnsafePath(path.to_path_buf()));
    }
    if !meta.is_dir() {
        return Err(ForgeCredentialError::UnsafePath(path.to_path_buf()));
    }
    let mode = meta.permissions().mode() & 0o777;
    if mode != 0o700 {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    Ok(())
}

#[cfg(unix)]
pub(super) fn check_file_mode(path: &Path) -> Result<(), ForgeCredentialError> {
    use std::os::unix::fs::PermissionsExt;
    let meta = fs::symlink_metadata(path)
        .map_err(|_| ForgeCredentialError::UnsafePath(path.to_path_buf()))?;
    if metadata_is_symlink_or_reparse(&meta) {
        return Err(ForgeCredentialError::UnsafePath(path.to_path_buf()));
    }
    if !meta.is_file() {
        return Err(ForgeCredentialError::UnsafePath(path.to_path_buf()));
    }
    let mode = meta.permissions().mode() & 0o777;
    if mode != 0o600 {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    Ok(())
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct FileId {
    dev: u64,
    ino: u64,
}

#[cfg(windows)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct FileId {
    volume: u64,
    index: u64,
}

#[cfg(not(any(unix, windows)))]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct FileId;

#[cfg(unix)]
pub(super) fn file_id(path: &Path) -> Result<FileId, ForgeCredentialError> {
    let mut file = File::open(path).map_err(|_| ForgeCredentialError::Io {
        context: "inspect file id",
        path: path.to_path_buf(),
    })?;
    file_id_from_file(&file)
}

#[cfg(windows)]
pub(super) fn file_id(path: &Path) -> Result<FileId, ForgeCredentialError> {
    let file = File::open(path).map_err(|_| ForgeCredentialError::Io {
        context: "inspect file id",
        path: path.to_path_buf(),
    })?;
    file_id_from_file(&file)
}

#[cfg(not(any(unix, windows)))]
pub(super) fn file_id(_path: &Path) -> Result<FileId, ForgeCredentialError> {
    Err(ForgeCredentialError::Provisioning)
}

#[cfg(unix)]
fn file_id_from_file(file: &File) -> Result<FileId, ForgeCredentialError> {
    use std::os::unix::fs::MetadataExt;
    let meta = file.metadata().map_err(|_| ForgeCredentialError::Io {
        context: "inspect file id",
        path: PathBuf::from("<handle>"),
    })?;
    Ok(FileId {
        dev: meta.dev(),
        ino: meta.ino(),
    })
}

#[cfg(windows)]
fn file_id_from_file(file: &File) -> Result<FileId, ForgeCredentialError> {
    let info = winapi_util::file::information(winapi_util::HandleRef::from_file(file))
        .map_err(|_| ForgeCredentialError::Provisioning)?;
    let volume = info.volume_serial_number();
    let index = info.file_index();
    if volume == 0 && index == 0 {
        return Err(ForgeCredentialError::Provisioning);
    }
    Ok(FileId { volume, index })
}

#[cfg(not(any(unix, windows)))]
fn file_id_from_file(_file: &File) -> Result<FileId, ForgeCredentialError> {
    Err(ForgeCredentialError::Provisioning)
}

pub(super) struct CreatedFile {
    path: PathBuf,
    pub(super) id: FileId,
    is_manifest: bool,
}

struct ScopedTemp {
    path: PathBuf,
    armed: bool,
}

impl ScopedTemp {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for ScopedTemp {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Validates or creates a directory with the same private permissions used by
/// Forge credential custody.
///
/// This seam is intentionally shared by other future credential-bearing
/// directories. It never repairs an existing directory with unsafe
/// permissions; callers receive the existing fail-closed error instead.
pub(crate) fn ensure_private_directory(dir: &Path) -> Result<(), ForgeCredentialError> {
    private_directory(dir, true)
}

/// Validates an already-existing private directory without creating it.
pub(crate) fn validate_private_directory(dir: &Path) -> Result<(), ForgeCredentialError> {
    private_directory(dir, false)
}

fn private_directory(dir: &Path, create_if_missing: bool) -> Result<(), ForgeCredentialError> {
    check_ancestors_all(dir, false)?;
    match fs::symlink_metadata(dir) {
        Ok(meta) if metadata_is_symlink_or_reparse(&meta) => {
            Err(ForgeCredentialError::UnsafePath(dir.to_path_buf()))
        }
        Ok(meta) if meta.is_dir() => {
            #[cfg(unix)]
            check_dir_mode(dir)?;
            #[cfg(windows)]
            {
                acl_diagnostic!(super::validation::acl_diagnostic::stage(
                    "DirectoryAclVerification"
                ));
                let identity = resolve_current_identity()?;
                verify_windows_dacl(dir, &identity.sid)?;
            }
            Ok(())
        }
        Ok(meta) if meta.is_file() => Err(ForgeCredentialError::UnsafePath(dir.to_path_buf())),
        Ok(_) => Err(ForgeCredentialError::UnsafePath(dir.to_path_buf())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !create_if_missing => {
            Err(ForgeCredentialError::Io {
                context: "inspect credentials directory",
                path: dir.to_path_buf(),
            })
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(dir).map_err(|_| ForgeCredentialError::Io {
                context: "create credentials directory",
                path: dir.to_path_buf(),
            })?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(|_| {
                    ForgeCredentialError::Io {
                        context: "restrict credentials directory",
                        path: dir.to_path_buf(),
                    }
                })?;
                sync_directory(dir.parent().unwrap_or(Path::new("/")))?;
                check_dir_mode(dir)?;
            }
            #[cfg(windows)]
            {
                restrict_directory_windows(dir)?;
            }
            Ok(())
        }
        Err(_) => Err(ForgeCredentialError::Io {
            context: "inspect credentials directory",
            path: dir.to_path_buf(),
        }),
    }
}

fn inspect_lock_path(lock_path: &Path) -> Result<(), ForgeCredentialError> {
    check_ancestors_all(lock_path, false)?;
    match fs::symlink_metadata(lock_path) {
        Ok(meta) if metadata_is_symlink_or_reparse(&meta) => {
            return Err(ForgeCredentialError::UnsafePath(lock_path.to_path_buf()));
        }
        Ok(meta) if meta.is_dir() => {
            return Err(ForgeCredentialError::UnsafePath(lock_path.to_path_buf()));
        }
        Ok(meta) if !meta.is_file() => {
            return Err(ForgeCredentialError::UnsafePath(lock_path.to_path_buf()));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {
            return Err(ForgeCredentialError::Io {
                context: "inspect lock",
                path: lock_path.to_path_buf(),
            });
        }
    }
    Ok(())
}

fn open_lock_file(lock_path: &Path) -> Result<File, ForgeCredentialError> {
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(lock_path)
        .map_err(|_| ForgeCredentialError::Io {
            context: "open credential lock",
            path: lock_path.to_path_buf(),
        })
}

fn validate_lock_after_open(lock_path: &Path, file: &File) -> Result<(), ForgeCredentialError> {
    match fs::symlink_metadata(lock_path) {
        Ok(meta) if metadata_is_symlink_or_reparse(&meta) => {
            return Err(ForgeCredentialError::UnsafePath(lock_path.to_path_buf()));
        }
        Ok(meta) if !meta.is_file() => {
            return Err(ForgeCredentialError::UnsafePath(lock_path.to_path_buf()));
        }
        Ok(_) => {}
        Err(_) => {
            return Err(ForgeCredentialError::Io {
                context: "inspect lock after open",
                path: lock_path.to_path_buf(),
            });
        }
    }
    {
        let open_id = file_id_from_file(file)?;
        let path_id = file_id(lock_path)?;
        if open_id != path_id {
            return Err(ForgeCredentialError::UnsafePath(lock_path.to_path_buf()));
        }
    }
    #[cfg(unix)]
    check_file_mode(lock_path)?;
    #[cfg(windows)]
    {
        acl_diagnostic!(super::validation::acl_diagnostic::stage(
            "ProvisionLockVerification"
        ));
        let identity = resolve_current_identity()?;
        let verified = verify_windows_dacl(lock_path, &identity.sid);
        if verified.is_err() {
            let grant = format!("*{}:F", identity.sid);
            let path_str = lock_path.to_string_lossy().to_string();
            let _ = hidden_output(
                "icacls.exe",
                &[&path_str, "/inheritance:r", "/grant:r", &grant],
                Duration::from_secs(5),
            );
            verify_windows_dacl(lock_path, &identity.sid)?;
        }
    }
    Ok(())
}

pub(super) fn acquire_lock(lock_path: &Path) -> Result<File, ForgeCredentialError> {
    inspect_lock_path(lock_path)?;
    let file = open_lock_file(lock_path)?;
    file.lock_exclusive()
        .map_err(|_| ForgeCredentialError::Io {
            context: "lock credential lock",
            path: lock_path.to_path_buf(),
        })?;
    validate_lock_after_open(lock_path, &file)?;
    Ok(file)
}

fn lock_error_is_contention(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::WouldBlock || matches!(error.raw_os_error(), Some(32 | 33))
}

pub(super) fn acquire_lock_with_timeout(
    lock_path: &Path,
    timeout: Duration,
) -> Result<File, ForgeCredentialError> {
    inspect_lock_path(lock_path)?;
    let file = open_lock_file(lock_path)?;
    let started = Instant::now();
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => break,
            Err(error) if lock_error_is_contention(&error) => {
                if started.elapsed() >= timeout {
                    return Err(ForgeCredentialError::CapabilityBusy);
                }
                std::thread::yield_now();
            }
            Err(_) => {
                return Err(ForgeCredentialError::Io {
                    context: "lock credential lock",
                    path: lock_path.to_path_buf(),
                });
            }
        }
    }
    validate_lock_after_open(lock_path, &file)?;
    Ok(file)
}

fn validate_private_file(path: &Path) -> Result<FileId, ForgeCredentialError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| ForgeCredentialError::UnsafePath(path.to_path_buf()))?;
    if metadata_is_symlink_or_reparse(&metadata) || !metadata.is_file() {
        return Err(ForgeCredentialError::UnsafePath(path.to_path_buf()));
    }
    let permission_before_id = file_id(path)?;
    #[cfg(unix)]
    check_file_mode(path)?;
    #[cfg(windows)]
    {
        acl_diagnostic!(super::validation::acl_diagnostic::stage(
            "BundleAclVerification"
        ));
        let identity = resolve_current_identity()?;
        verify_windows_dacl(path, &identity.sid)?;
    }
    let permission_after_id = file_id(path)?;
    if permission_before_id != permission_after_id {
        return Err(ForgeCredentialError::UnsafePath(path.to_path_buf()));
    }
    Ok(permission_after_id)
}

pub(super) fn validate_private_material(
    paths: &ForgeCredentialPaths,
    path: &Path,
) -> Result<FileId, ForgeCredentialError> {
    let credentials_dir = paths.credentials_dir();
    validate_private_directory(&credentials_dir)?;
    validate_private_file(path)
}

pub(super) struct BoundedMaterialRead {
    pub(super) bytes: Zeroizing<Vec<u8>>,
    pub(super) pre_id: FileId,
    pub(super) opened_id: FileId,
    pub(super) post_id: FileId,
}

pub(super) fn open_and_read_bounded(
    path: &Path,
    read_limit: usize,
    oversized: ForgeCredentialError,
    expected_id: FileId,
) -> Result<BoundedMaterialRead, ForgeCredentialError> {
    if read_limit == 0 {
        return Err(oversized);
    }
    let read_limit_u64 = u64::try_from(read_limit).map_err(|_| oversized.clone())?;
    check_ancestors_all(path, true)?;
    let pre_meta = fs::symlink_metadata(path).map_err(|_| ForgeCredentialError::Io {
        context: "inspect file",
        path: path.to_path_buf(),
    })?;
    if metadata_is_symlink_or_reparse(&pre_meta) || !pre_meta.is_file() {
        return Err(ForgeCredentialError::UnsafePath(path.to_path_buf()));
    }
    let pre_id = file_id(path)?;
    if pre_id != expected_id {
        return Err(ForgeCredentialError::UnsafePath(path.to_path_buf()));
    }
    let mut file =
        OpenOptions::new()
            .read(true)
            .open(path)
            .map_err(|_| ForgeCredentialError::Io {
                context: "open file",
                path: path.to_path_buf(),
            })?;
    let handle_meta = file.metadata().map_err(|_| ForgeCredentialError::Io {
        context: "inspect handle",
        path: path.to_path_buf(),
    })?;
    if metadata_is_symlink_or_reparse(&handle_meta) || !handle_meta.is_file() {
        return Err(ForgeCredentialError::UnsafePath(path.to_path_buf()));
    }
    let handle_id = file_id_from_file(&file)?;
    if handle_id != expected_id || handle_id != pre_id {
        return Err(ForgeCredentialError::UnsafePath(path.to_path_buf()));
    }
    if handle_meta.len() >= read_limit_u64 {
        return Err(oversized);
    }

    let mut bytes = Zeroizing::new(Vec::with_capacity(read_limit));
    let mut chunk = Zeroizing::new([0_u8; SAFE_READ_CHUNK_BYTES]);
    while bytes.len() < read_limit {
        let chunk_len = (read_limit - bytes.len()).min(SAFE_READ_CHUNK_BYTES);
        let read = file
            .read(&mut chunk[..chunk_len])
            .map_err(|_| ForgeCredentialError::Io {
                context: "read file",
                path: path.to_path_buf(),
            })?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..read]);
    }

    check_ancestors_all(path, true)?;
    let post_meta = fs::symlink_metadata(path).map_err(|_| ForgeCredentialError::Io {
        context: "inspect file",
        path: path.to_path_buf(),
    })?;
    if metadata_is_symlink_or_reparse(&post_meta) || !post_meta.is_file() {
        return Err(ForgeCredentialError::UnsafePath(path.to_path_buf()));
    }
    let post_id = file_id(path)?;
    if expected_id != pre_id || handle_id != pre_id || post_id != pre_id {
        return Err(ForgeCredentialError::UnsafePath(path.to_path_buf()));
    }
    if post_meta.len() >= read_limit_u64 {
        return Err(oversized);
    }
    Ok(BoundedMaterialRead {
        bytes,
        pre_id,
        opened_id: handle_id,
        post_id,
    })
}

pub(super) fn read_private_material(
    paths: &ForgeCredentialPaths,
    path: &Path,
    read_limit: usize,
    oversized: ForgeCredentialError,
) -> Result<Zeroizing<Vec<u8>>, ForgeCredentialError> {
    let permission_before_id = validate_private_material(paths, path)?;
    let read = open_and_read_bounded(path, read_limit, oversized, permission_before_id)?;
    let permission_after_id = validate_private_material(paths, path)?;
    let identity_chain = [
        permission_before_id,
        permission_before_id,
        read.pre_id,
        read.opened_id,
        read.post_id,
        permission_after_id,
        permission_after_id,
    ];
    if !private_material_identity_chain_matches(&identity_chain) {
        return Err(ForgeCredentialError::UnsafePath(path.to_path_buf()));
    }
    Ok(read.bytes)
}

pub(super) fn private_material_identity_chain_matches<T: PartialEq>(chain: &[T; 7]) -> bool {
    chain.windows(2).all(|pair| pair[0] == pair[1])
}

fn verify_atomic_target(path: &Path, expected_id: FileId) -> Result<(), ForgeCredentialError> {
    check_ancestors_all(path, true)?;
    let metadata =
        fs::symlink_metadata(path).map_err(|_| ForgeCredentialError::ReconnectStaleWriter)?;
    if metadata_is_symlink_or_reparse(&metadata) || !metadata.is_file() {
        return Err(ForgeCredentialError::ReconnectStaleWriter);
    }
    if file_id(path)? != expected_id {
        return Err(ForgeCredentialError::ReconnectStaleWriter);
    }
    Ok(())
}

fn validate_atomic_temporary(
    temporary_path: &Path,
    destination_path: &Path,
) -> Result<FileId, ForgeCredentialError> {
    let metadata = fs::symlink_metadata(temporary_path).map_err(|_| ForgeCredentialError::Io {
        context: "inspect temporary reconnect record",
        path: destination_path.to_path_buf(),
    })?;
    if metadata_is_symlink_or_reparse(&metadata) || !metadata.is_file() {
        return Err(ForgeCredentialError::UnsafePath(
            destination_path.to_path_buf(),
        ));
    }
    let temporary_id = file_id(temporary_path).map_err(|_| ForgeCredentialError::Io {
        context: "inspect temporary reconnect record",
        path: destination_path.to_path_buf(),
    })?;
    #[cfg(unix)]
    check_file_mode(temporary_path)
        .map_err(|_| ForgeCredentialError::UnsafePath(destination_path.to_path_buf()))?;
    Ok(temporary_id)
}

pub(super) fn atomic_replace_private_file(
    path: &Path,
    data: &[u8],
    expected_id: FileId,
    before_replace: impl FnOnce() -> Result<(), ForgeCredentialError>,
) -> Result<FileId, ForgeCredentialError> {
    let directory = path
        .parent()
        .ok_or_else(|| ForgeCredentialError::UnsafePath(path.to_path_buf()))?;
    validate_private_directory(directory)?;
    verify_atomic_target(path, expected_id)?;

    let mut nonce = [0_u8; 16];
    getrandom::fill(&mut nonce).map_err(|_| ForgeCredentialError::Provisioning)?;
    let temporary_name = format!(
        ".{RECONNECT_CAPABILITY_FILENAME}.{}.tmp",
        encode_nonce_hex(&nonce)
    );
    let temporary_path = directory.join(temporary_name);
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary_path)
        .map_err(|_| ForgeCredentialError::Io {
            context: "create temporary reconnect record",
            path: path.to_path_buf(),
        })?;
    let mut temporary_guard = ScopedTemp::new(temporary_path.clone());
    file.write_all(data).map_err(|_| ForgeCredentialError::Io {
        context: "write temporary reconnect record",
        path: path.to_path_buf(),
    })?;
    file.sync_all().map_err(|_| ForgeCredentialError::Io {
        context: "sync temporary reconnect record",
        path: path.to_path_buf(),
    })?;
    let temporary_id = file_id_from_file(&file).map_err(|_| ForgeCredentialError::Io {
        context: "inspect temporary reconnect record",
        path: path.to_path_buf(),
    })?;
    drop(file);
    let checked_temporary_id = validate_atomic_temporary(&temporary_path, path)?;
    if checked_temporary_id != temporary_id {
        return Err(ForgeCredentialError::ReconnectStaleWriter);
    }
    #[cfg(windows)]
    {
        let identity = resolve_current_identity()?;
        restrict_file_windows(&temporary_path, &identity.sid)?;
    }

    verify_atomic_target(path, expected_id)?;
    before_replace()?;
    verify_atomic_target(path, expected_id)?;
    fs::rename(&temporary_path, path).map_err(|_| ForgeCredentialError::Io {
        context: "replace reconnect record",
        path: path.to_path_buf(),
    })?;
    temporary_guard.disarm();

    check_ancestors_all(path, true)?;
    let metadata = fs::symlink_metadata(path).map_err(|_| ForgeCredentialError::Io {
        context: "inspect replaced reconnect record",
        path: path.to_path_buf(),
    })?;
    if metadata_is_symlink_or_reparse(&metadata) || !metadata.is_file() {
        return Err(ForgeCredentialError::UnsafePath(path.to_path_buf()));
    }
    let destination_id = file_id(path)?;
    if destination_id != temporary_id {
        return Err(ForgeCredentialError::ReconnectStaleWriter);
    }
    #[cfg(unix)]
    check_file_mode(path)?;
    #[cfg(windows)]
    {
        let identity = resolve_current_identity()?;
        verify_windows_dacl(path, &identity.sid)?;
    }
    sync_directory(directory)?;
    Ok(destination_id)
}

pub(super) fn ensure_credentials_dir(dir: &Path) -> Result<(), ForgeCredentialError> {
    ensure_private_directory(dir)
}

pub(super) fn install_atomic(
    dir: &Path,
    filename: &str,
    data: &[u8],
    created: &mut Vec<CreatedFile>,
) -> Result<(), ForgeCredentialError> {
    if !is_safe_filename(filename) {
        return Err(ForgeCredentialError::ManifestTraversal);
    }
    let dest = dir.join(filename);
    let is_manifest = filename == "manifest.json";
    match fs::symlink_metadata(&dest) {
        Ok(meta) if metadata_is_symlink_or_reparse(&meta) => {
            return Err(ForgeCredentialError::UnsafePath(dest));
        }
        Ok(meta) if meta.is_dir() => {
            return Err(ForgeCredentialError::UnsafePath(dest));
        }
        Ok(meta) if meta.is_file() => {
            return Err(ForgeCredentialError::PartialBundle);
        }
        Ok(_) => return Err(ForgeCredentialError::UnsafePath(dest)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {
            return Err(ForgeCredentialError::Io {
                context: "inspect destination",
                path: dest.clone(),
            });
        }
    }
    check_ancestors_all(&dest, false)?;
    let mut nonce = [0_u8; 16];
    getrandom::fill(&mut nonce).map_err(|_| ForgeCredentialError::Provisioning)?;
    let nonce_hex = encode_nonce_hex(&nonce);
    let temp_name = format!(".{filename}.{nonce_hex}.tmp");
    let temp_path = dir.join(&temp_name);
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temp_path)
        .map_err(|_| ForgeCredentialError::Io {
            context: "create temporary file",
            path: dest.clone(),
        })?;
    let mut temp_guard = ScopedTemp::new(temp_path.clone());
    file.write_all(data).map_err(|_| ForgeCredentialError::Io {
        context: "write temporary file",
        path: dest.clone(),
    })?;
    file.sync_all().map_err(|_| ForgeCredentialError::Io {
        context: "sync temporary file",
        path: dest.clone(),
    })?;
    let temp_id = file_id_from_file(&file)?;
    drop(file);
    fs::hard_link(&temp_path, &dest).map_err(|_| ForgeCredentialError::Io {
        context: "activate file",
        path: dest.clone(),
    })?;
    created.push(CreatedFile {
        path: dest.clone(),
        id: temp_id,
        is_manifest,
    });
    let dest_handle = File::open(&dest).map_err(|_| ForgeCredentialError::Io {
        context: "inspect file id",
        path: dest.clone(),
    })?;
    let dest_id = file_id_from_file(&dest_handle)?;
    drop(dest_handle);
    if dest_id != temp_id {
        return Err(ForgeCredentialError::Provisioning);
    }
    sync_directory(dir)?;
    #[cfg(unix)]
    {
        if let Err(e) = check_file_mode(&dest) {
            return Err(e);
        }
    }
    #[cfg(windows)]
    {
        let identity = resolve_current_identity()?;
        restrict_file_windows(&dest, &identity.sid)?;
    }
    if fs::remove_file(&temp_path).is_err() {
        return Err(ForgeCredentialError::Io {
            context: "remove temporary file",
            path: dest.clone(),
        });
    }
    temp_guard.disarm();
    Ok(())
}

pub(super) fn cleanup_created(mut created: Vec<CreatedFile>) {
    // Manifest last: non-manifest first, then manifest
    let mut non_manifest = Vec::new();
    let mut manifests = Vec::new();
    for entry in created.drain(..) {
        if entry.is_manifest {
            manifests.push(entry);
        } else {
            non_manifest.push(entry);
        }
    }
    for entry in non_manifest.into_iter().chain(manifests) {
        if file_id(&entry.path).is_ok_and(|current_id| current_id == entry.id) {
            let _ = fs::remove_file(&entry.path);
        }
    }
}

#[cfg(unix)]
fn sync_directory(dir: &Path) -> Result<(), ForgeCredentialError> {
    let file = File::open(dir).map_err(|_| ForgeCredentialError::Io {
        context: "sync directory",
        path: dir.to_path_buf(),
    })?;
    file.sync_all().map_err(|_| ForgeCredentialError::Io {
        context: "sync directory",
        path: dir.to_path_buf(),
    })
}

#[cfg(windows)]
fn sync_directory(dir: &Path) -> Result<(), ForgeCredentialError> {
    // Windows does not support File::sync_all on directory handles, even
    // when they are opened with the directory-handle backup flag. The
    // temporary file is flushed before publication, so retain a
    // post-publication directory safety check without claiming that the
    // directory was flushed.
    let metadata = fs::symlink_metadata(dir).map_err(|_| ForgeCredentialError::Io {
        context: "inspect directory",
        path: dir.to_path_buf(),
    })?;
    if metadata_is_symlink_or_reparse(&metadata) || !metadata.is_dir() {
        return Err(ForgeCredentialError::UnsafePath(dir.to_path_buf()));
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn sync_directory(_dir: &Path) -> Result<(), ForgeCredentialError> {
    Ok(())
}
