use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    num::NonZeroU32,
    path::{Path, PathBuf},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{CliError, Result, error::io, paths::Layout};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct InstanceConfig {
    pub data_root: PathBuf,
    pub listen_host: String,
    pub listen_port: u16,
    pub mode: ForgeMode,
    #[serde(default)]
    pub serve_frontend: bool,
    pub version: u8,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ForgeMode {
    Local,
    Headless,
}

#[derive(Clone, Debug, Deserialize)]
pub struct State {
    pub endpoint: String,
    pub instance_id: String,
    pub pid: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Secrets {
    pub auth_token: String,
    pub version: u8,
}

#[derive(Clone, Debug)]
pub struct InstancePaths {
    pub config: PathBuf,
    pub secrets: PathBuf,
    pub state: PathBuf,
    pub log: PathBuf,
}

pub fn paths(layout: &Layout) -> Result<InstancePaths> {
    migrate_legacy_profiles(layout)?;
    Ok(InstancePaths {
        config: layout.root.join("config.json"),
        secrets: layout.root.join("secrets.json"),
        state: layout.root.join("state.json"),
        log: layout.root.join("forge.log"),
    })
}

fn migrate_legacy_profiles(layout: &Layout) -> Result<()> {
    if layout.root.join("config.json").is_file() {
        return Ok(());
    }
    let profiles = layout.root.join("profiles");
    if !profiles.is_dir() {
        return Ok(());
    }
    let mut directories = Vec::new();
    for entry in fs::read_dir(&profiles).map_err(io("enumerate legacy Forge profiles"))? {
        let entry = entry.map_err(io("enumerate legacy Forge profile"))?;
        let file_type = entry
            .file_type()
            .map_err(io("inspect legacy Forge profile"))?;
        if file_type.is_dir() && !file_type.is_symlink() {
            directories.push(entry.path());
        }
    }
    match directories.as_slice() {
        [] => {
            let _ = fs::remove_dir_all(&profiles);
            Ok(())
        }
        [single] => {
            let name = single
                .file_name()
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_default();
            for entry in fs::read_dir(single).map_err(io("enumerate legacy Forge profile"))? {
                let entry = entry.map_err(io("enumerate legacy Forge profile"))?;
                let destination = layout.root.join(entry.file_name());
                if destination.exists() {
                    return Err(CliError::Installation(format!(
                        "cannot migrate legacy Forge profile `{name}`: {} already exists",
                        destination.display()
                    )));
                }
                fs::rename(entry.path(), &destination)
                    .map_err(io("migrate legacy Forge profile file"))?;
            }
            let _ = fs::remove_dir_all(&profiles);
            eprintln!(
                "migrated legacy Forge profile `{name}` into the Artisan home root at {}",
                layout.root.display()
            );
            Ok(())
        }
        many => {
            let names = many
                .iter()
                .filter_map(|path| path.file_name())
                .map(|name| name.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(", ");
            Err(CliError::Installation(format!(
                "this Artisan home has multiple legacy Forge profiles ({names}) and Artisan now \
                 runs one Forge per home; delete all but one directory under {} and retry",
                profiles.display()
            )))
        }
    }
}

pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = fs::read(path).map_err(io("read Forge instance file"))?;
    serde_json::from_slice(&bytes).map_err(|source| CliError::Json {
        path: path.to_path_buf(),
        source,
    })
}

pub fn load(layout: &Layout) -> Result<(InstancePaths, InstanceConfig, Secrets)> {
    let paths = paths(layout)?;
    if !paths.config.is_file() || !paths.secrets.is_file() {
        return Err(CliError::MissingInstance);
    }
    let config = read_json(&paths.config)?;
    let secrets = read_json(&paths.secrets)?;
    Ok((paths, config, secrets))
}

pub fn setup(
    layout: &Layout,
    mode: ForgeMode,
    port: u16,
    data_root: Option<&Path>,
    serve_frontend: bool,
) -> Result<()> {
    let paths = paths(layout)?;
    fs::create_dir_all(&layout.root).map_err(io("create Artisan home directory"))?;
    restrict_directory(&layout.root)?;
    let config = InstanceConfig {
        data_root: data_root.map_or_else(|| layout.root.join("data"), Path::to_path_buf),
        listen_host: "127.0.0.1".into(),
        listen_port: port,
        mode,
        serve_frontend,
        version: 1,
    };
    write_private_json(&paths.config, &config)?;
    if !paths.secrets.exists() {
        let mut token = [0_u8; 32];
        getrandom::fill(&mut token).map_err(|error| {
            CliError::Installation(format!("secure random source failed: {error}"))
        })?;
        write_private_json(
            &paths.secrets,
            &Secrets {
                auth_token: URL_SAFE_NO_PAD.encode(token),
                version: 1,
            },
        )?;
    }
    Ok(())
}

pub(crate) fn write_private_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|source| CliError::Json {
        path: path.to_path_buf(),
        source,
    })?;
    reject_unsafe_destination(path)?;
    let directory = path
        .parent()
        .ok_or_else(|| CliError::UnsafePath(path.to_path_buf()))?;
    let mut nonce = [0_u8; 16];
    getrandom::fill(&mut nonce)
        .map_err(|error| CliError::Installation(format!("secure random source failed: {error}")))?;
    let temporary = directory.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("instance"),
        URL_SAFE_NO_PAD.encode(nonce)
    ));
    let result = (|| -> Result<()> {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .map_err(io("create temporary instance file"))?;
        file.write_all(&bytes)
            .map_err(io("write temporary instance file"))?;
        file.sync_all()
            .map_err(io("sync temporary instance file"))?;
        drop(file);
        reject_unsafe_destination(path)?;
        fs::rename(&temporary, path).map_err(io("activate instance file"))?;
        sync_directory(directory)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub(crate) fn reject_unsafe_destination(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(CliError::UnsafePath(path.to_path_buf()))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(CliError::Io {
            context: "inspect instance file destination",
            source,
        }),
    }
}

#[cfg(unix)]
fn restrict_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(io("restrict Artisan home directory"))
}

#[cfg(not(unix))]
fn restrict_directory(path: &Path) -> Result<()> {
    fs::metadata(path)
        .map(|_| ())
        .map_err(io("inspect Artisan home directory"))
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    fs::File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(io("sync Artisan home directory"))
}

#[cfg(not(unix))]
fn sync_directory(path: &Path) -> Result<()> {
    fs::metadata(path)
        .map(|_| ())
        .map_err(io("inspect Artisan home directory"))
}

#[derive(Clone, Eq, PartialEq)]
pub enum NativeInstanceError {
    NotFound,
    TooLarge,
    FileChanged,
    FileSizeMismatch,
    FileHashMismatch,
    InvalidPath(PathBuf),
    Io {
        context: &'static str,
        path: PathBuf,
    },
    InvalidManifest,
    UnsafePath(PathBuf),
}

impl std::fmt::Display for NativeInstanceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => f.write_str("native file is not present"),
            Self::TooLarge => f.write_str("native file exceeds its read bound"),
            Self::FileChanged => f.write_str("native file changed while it was read"),
            Self::FileSizeMismatch => f.write_str("native file size does not match"),
            Self::FileHashMismatch => f.write_str("native file hash does not match"),
            Self::InvalidPath(path) => write!(f, "invalid absolute path: {}", path.display()),
            Self::Io { context, path } => {
                write!(f, "{context} at {}: [REDACTED]", path.display())
            }
            Self::InvalidManifest => write!(f, "invalid instance manifest"),
            Self::UnsafePath(path) => {
                write!(
                    f,
                    "refusing unsafe filesystem operation on {}",
                    path.display()
                )
            }
        }
    }
}

impl std::fmt::Debug for NativeInstanceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => f.debug_tuple("NotFound").finish(),
            Self::TooLarge => f.debug_tuple("TooLarge").finish(),
            Self::FileChanged => f.debug_tuple("FileChanged").finish(),
            Self::FileSizeMismatch => f.debug_tuple("FileSizeMismatch").finish(),
            Self::FileHashMismatch => f.debug_tuple("FileHashMismatch").finish(),
            Self::InvalidPath(path) => f
                .debug_tuple("InvalidPath")
                .field(&path.display().to_string())
                .finish(),
            Self::Io { context, path } => f
                .debug_struct("Io")
                .field("context", context)
                .field("path", &path.display().to_string())
                .finish(),
            Self::InvalidManifest => f.debug_tuple("InvalidManifest").finish(),
            Self::UnsafePath(path) => f
                .debug_tuple("UnsafePath")
                .field(&path.display().to_string())
                .finish(),
        }
    }
}

impl std::error::Error for NativeInstanceError {}

type NativeResult<T> = std::result::Result<T, NativeInstanceError>;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeInstanceFile {
    schema: String,
    version: u64,
    database_path: PathBuf,
    custody_path: PathBuf,
    readiness_path: PathBuf,
    credentials_manifest: PathBuf,
    listener: NativeListenerFile,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct NativeListenerFile {
    admission_timeout_ms: u64,
    handshake_timeout_ms: u64,
    request_timeout_ms: u64,
    drain_timeout_ms: u64,
    admission_capacity: NonZeroU32,
    requests_per_connection: NonZeroU32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeListenerConfig {
    admission_timeout_ms: u64,
    handshake_timeout_ms: u64,
    request_timeout_ms: u64,
    drain_timeout_ms: u64,
    admission_capacity: NonZeroU32,
    requests_per_connection: NonZeroU32,
}

impl NativeListenerConfig {
    pub fn new(
        admission_timeout_ms: u64,
        handshake_timeout_ms: u64,
        request_timeout_ms: u64,
        drain_timeout_ms: u64,
        admission_capacity: NonZeroU32,
        requests_per_connection: NonZeroU32,
    ) -> Self {
        Self {
            admission_timeout_ms,
            handshake_timeout_ms,
            request_timeout_ms,
            drain_timeout_ms,
            admission_capacity,
            requests_per_connection,
        }
    }

    pub fn admission_timeout_ms(&self) -> u64 {
        self.admission_timeout_ms
    }

    pub fn handshake_timeout_ms(&self) -> u64 {
        self.handshake_timeout_ms
    }

    pub fn request_timeout_ms(&self) -> u64 {
        self.request_timeout_ms
    }

    pub fn drain_timeout_ms(&self) -> u64 {
        self.drain_timeout_ms
    }

    pub fn admission_capacity(&self) -> NonZeroU32 {
        self.admission_capacity
    }

    pub fn requests_per_connection(&self) -> NonZeroU32 {
        self.requests_per_connection
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeInstanceConfig {
    database_path: PathBuf,
    custody_path: PathBuf,
    readiness_path: PathBuf,
    credentials_manifest: PathBuf,
    listener: NativeListenerConfig,
}

fn metadata_is_symlink_or_reparse(meta: &fs::Metadata) -> bool {
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

fn check_ancestors_all(path: &Path, must_exist: bool) -> NativeResult<()> {
    let parent = path.parent().unwrap_or(Path::new("/"));
    for ancestor in parent.ancestors() {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        match fs::symlink_metadata(ancestor) {
            Ok(meta) => {
                if metadata_is_symlink_or_reparse(&meta) {
                    return Err(NativeInstanceError::UnsafePath(ancestor.to_path_buf()));
                }
                if !meta.is_dir() {
                    return Err(NativeInstanceError::UnsafePath(ancestor.to_path_buf()));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if must_exist {
                    return Err(NativeInstanceError::Io {
                        context: "inspect parent",
                        path: ancestor.to_path_buf(),
                    });
                }
            }
            Err(_) => {
                return Err(NativeInstanceError::Io {
                    context: "inspect parent",
                    path: ancestor.to_path_buf(),
                });
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct NativeFileId {
    dev: u64,
    ino: u64,
}

#[cfg(windows)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct NativeFileId {
    volume: u64,
    index: u64,
}

#[cfg(unix)]
fn native_file_id(path: &Path) -> NativeResult<NativeFileId> {
    let file = fs::File::open(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            NativeInstanceError::NotFound
        } else {
            NativeInstanceError::Io {
                context: "inspect file id",
                path: path.to_path_buf(),
            }
        }
    })?;
    native_file_id_from_file(&file)
}

#[cfg(windows)]
fn native_file_id(path: &Path) -> NativeResult<NativeFileId> {
    let file = fs::File::open(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            NativeInstanceError::NotFound
        } else {
            NativeInstanceError::Io {
                context: "inspect file id",
                path: path.to_path_buf(),
            }
        }
    })?;
    native_file_id_from_file(&file)
}

#[cfg(not(any(unix, windows)))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct NativeFileId;

#[cfg(not(any(unix, windows)))]
fn native_file_id(_path: &Path) -> NativeResult<NativeFileId> {
    Err(NativeInstanceError::Io {
        context: "inspect file id",
        path: PathBuf::from("<unsupported>"),
    })
}

fn native_file_id_from_file(file: &fs::File) -> NativeResult<NativeFileId> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = file.metadata().map_err(|_| NativeInstanceError::Io {
            context: "inspect file id",
            path: PathBuf::from("<handle>"),
        })?;
        Ok(NativeFileId {
            dev: meta.dev(),
            ino: meta.ino(),
        })
    }
    #[cfg(windows)]
    {
        let info = winapi_util::file::information(winapi_util::HandleRef::from_file(file))
            .map_err(|_| NativeInstanceError::Io {
                context: "inspect file id",
                path: PathBuf::from("<handle>"),
            })?;
        let volume = info.volume_serial_number();
        let index = info.file_index();
        if volume == 0 && index == 0 {
            return Err(NativeInstanceError::Io {
                context: "inspect file id",
                path: PathBuf::from("<handle>"),
            });
        }
        Ok(NativeFileId { volume, index })
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = file;
        Err(NativeInstanceError::Io {
            context: "inspect file id",
            path: PathBuf::from("<unsupported>"),
        })
    }
}

fn open_and_read_native(path: &Path) -> NativeResult<Vec<u8>> {
    open_and_read_native_inner(path, None, false)
}

pub(crate) fn read_bounded_native_file(
    path: &Path,
    maximum_bytes: usize,
) -> std::result::Result<Vec<u8>, NativeInstanceError> {
    open_and_read_native_inner(path, Some(maximum_bytes), true)
}

fn open_and_read_native_inner(
    path: &Path,
    maximum_bytes: Option<usize>,
    classify_missing: bool,
) -> NativeResult<Vec<u8>> {
    check_ancestors_all(path, false)?;
    let pre_meta = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if classify_missing && error.kind() == std::io::ErrorKind::NotFound => {
            return Err(NativeInstanceError::NotFound);
        }
        Err(_) => {
            return Err(NativeInstanceError::Io {
                context: "inspect instance file",
                path: path.to_path_buf(),
            });
        }
    };
    if metadata_is_symlink_or_reparse(&pre_meta) || !pre_meta.is_file() {
        return Err(NativeInstanceError::UnsafePath(path.to_path_buf()));
    }
    check_ancestors_all(path, true)?;
    let pre_id = native_file_id(path)?;
    let mut file =
        OpenOptions::new()
            .read(true)
            .open(path)
            .map_err(|_| NativeInstanceError::Io {
                context: "open instance file",
                path: path.to_path_buf(),
            })?;
    let handle_meta = file.metadata().map_err(|_| NativeInstanceError::Io {
        context: "inspect handle",
        path: path.to_path_buf(),
    })?;
    if metadata_is_symlink_or_reparse(&handle_meta) || !handle_meta.is_file() {
        return Err(NativeInstanceError::UnsafePath(path.to_path_buf()));
    }
    let handle_id = native_file_id_from_file(&file)?;
    if handle_id != pre_id {
        return Err(NativeInstanceError::UnsafePath(path.to_path_buf()));
    }
    let mut bytes = Vec::new();
    match maximum_bytes {
        Some(maximum_bytes) => {
            let read_limit = maximum_bytes.saturating_add(1);
            std::io::Read::by_ref(&mut file)
                .take(u64::try_from(read_limit).unwrap_or(u64::MAX))
                .read_to_end(&mut bytes)
                .map_err(|_| NativeInstanceError::Io {
                    context: "read instance file",
                    path: path.to_path_buf(),
                })?;
        }
        None => {
            file.read_to_end(&mut bytes)
                .map_err(|_| NativeInstanceError::Io {
                    context: "read instance file",
                    path: path.to_path_buf(),
                })?;
        }
    }
    check_ancestors_all(path, true)?;
    let post_meta = fs::symlink_metadata(path).map_err(|_| NativeInstanceError::Io {
        context: "inspect instance file",
        path: path.to_path_buf(),
    })?;
    if metadata_is_symlink_or_reparse(&post_meta) || !post_meta.is_file() {
        return Err(NativeInstanceError::UnsafePath(path.to_path_buf()));
    }
    let post_id = native_file_id(path)?;
    if post_id != pre_id || post_id != handle_id {
        return Err(NativeInstanceError::UnsafePath(path.to_path_buf()));
    }
    if maximum_bytes.is_some_and(|maximum| bytes.len() > maximum) {
        return Err(NativeInstanceError::TooLarge);
    }
    Ok(bytes)
}

pub(crate) fn verify_native_file(
    path: &Path,
    expected_size: u64,
    expected_sha256: &[u8; 32],
) -> std::result::Result<NativeFileId, NativeInstanceError> {
    check_ancestors_all(path, false)?;
    let pre_meta = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(NativeInstanceError::NotFound);
        }
        Err(_) => {
            return Err(NativeInstanceError::Io {
                context: "inspect native executable",
                path: path.to_path_buf(),
            });
        }
    };
    if metadata_is_symlink_or_reparse(&pre_meta) || !pre_meta.is_file() {
        return Err(NativeInstanceError::UnsafePath(path.to_path_buf()));
    }
    if pre_meta.len() != expected_size {
        return Err(NativeInstanceError::FileSizeMismatch);
    }
    check_ancestors_all(path, true)?;
    let pre_id = native_file_id(path)?;
    let mut file = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|_| NativeInstanceError::FileChanged)?;
    let handle_meta = file
        .metadata()
        .map_err(|_| NativeInstanceError::FileChanged)?;
    if metadata_is_symlink_or_reparse(&handle_meta) || !handle_meta.is_file() {
        return Err(NativeInstanceError::UnsafePath(path.to_path_buf()));
    }
    if handle_meta.len() != expected_size {
        return Err(NativeInstanceError::FileSizeMismatch);
    }
    let handle_id = native_file_id_from_file(&file)?;
    if handle_id != pre_id {
        return Err(NativeInstanceError::FileChanged);
    }

    let stream_result = stream_and_verify(&mut file, expected_size, expected_sha256);

    let post_handle_meta = file
        .metadata()
        .map_err(|_| NativeInstanceError::FileChanged)?;
    if metadata_is_symlink_or_reparse(&post_handle_meta) || !post_handle_meta.is_file() {
        return Err(NativeInstanceError::UnsafePath(path.to_path_buf()));
    }
    if post_handle_meta.len() != expected_size {
        return Err(NativeInstanceError::FileSizeMismatch);
    }
    let post_handle_id = native_file_id_from_file(&file)?;
    if post_handle_id != pre_id {
        return Err(NativeInstanceError::FileChanged);
    }

    check_ancestors_all(path, true)?;
    let post_meta = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            NativeInstanceError::FileChanged
        } else {
            NativeInstanceError::Io {
                context: "inspect native executable",
                path: path.to_path_buf(),
            }
        }
    })?;
    if metadata_is_symlink_or_reparse(&post_meta) || !post_meta.is_file() {
        return Err(NativeInstanceError::UnsafePath(path.to_path_buf()));
    }
    if post_meta.len() != expected_size {
        return Err(NativeInstanceError::FileSizeMismatch);
    }
    let post_id = native_file_id(path)?;
    if post_id != pre_id {
        return Err(NativeInstanceError::FileChanged);
    }
    stream_result?;
    Ok(pre_id)
}

pub(crate) fn stream_and_verify<R: Read>(
    reader: &mut R,
    expected_size: u64,
    expected_sha256: &[u8; 32],
) -> NativeResult<()> {
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|_| NativeInstanceError::Io {
                context: "read native executable",
                path: PathBuf::from("<handle>"),
            })?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or(NativeInstanceError::FileSizeMismatch)?;
        if total > expected_size {
            return Err(NativeInstanceError::FileSizeMismatch);
        }
        hasher.update(&buffer[..read]);
    }
    if total != expected_size {
        return Err(NativeInstanceError::FileSizeMismatch);
    }
    let digest = hasher.finalize();
    if digest[..] != expected_sha256[..] {
        return Err(NativeInstanceError::FileHashMismatch);
    }
    Ok(())
}

impl NativeInstanceConfig {
    pub fn new(
        database_path: PathBuf,
        custody_path: PathBuf,
        readiness_path: PathBuf,
        credentials_manifest: PathBuf,
        listener: NativeListenerConfig,
    ) -> NativeResult<Self> {
        for path in [
            &database_path,
            &custody_path,
            &readiness_path,
            &credentials_manifest,
        ] {
            if !path.is_absolute() || path.as_os_str().is_empty() || path.parent().is_none() {
                return Err(NativeInstanceError::InvalidPath((*path).clone()));
            }
        }
        Ok(Self {
            database_path,
            custody_path,
            readiness_path,
            credentials_manifest,
            listener,
        })
    }

    pub fn database_path(&self) -> &Path {
        &self.database_path
    }

    pub fn custody_path(&self) -> &Path {
        &self.custody_path
    }

    pub fn readiness_path(&self) -> &Path {
        &self.readiness_path
    }

    pub fn credentials_manifest(&self) -> &Path {
        &self.credentials_manifest
    }

    pub fn listener(&self) -> &NativeListenerConfig {
        &self.listener
    }

    pub fn native_path(home: &Path) -> PathBuf {
        home.join("instance-v2.json")
    }

    pub fn load(path: &Path) -> NativeResult<Self> {
        let bytes = open_and_read_native(path)?;
        let file: NativeInstanceFile =
            serde_json::from_slice(&bytes).map_err(|_| NativeInstanceError::InvalidManifest)?;
        if file.schema != "artisan-instance-v2" {
            return Err(NativeInstanceError::InvalidManifest);
        }
        if file.version != 2 {
            return Err(NativeInstanceError::InvalidManifest);
        }
        Self::new(
            file.database_path,
            file.custody_path,
            file.readiness_path,
            file.credentials_manifest,
            NativeListenerConfig::new(
                file.listener.admission_timeout_ms,
                file.listener.handshake_timeout_ms,
                file.listener.request_timeout_ms,
                file.listener.drain_timeout_ms,
                file.listener.admission_capacity,
                file.listener.requests_per_connection,
            ),
        )
    }

    pub fn write(&self, path: &Path) -> NativeResult<()> {
        check_ancestors_all(path, false)?;
        match fs::symlink_metadata(path) {
            Ok(meta) if metadata_is_symlink_or_reparse(&meta) || meta.is_dir() => {
                return Err(NativeInstanceError::UnsafePath(path.to_path_buf()));
            }
            Ok(meta) if meta.is_file() => {}
            Ok(_) => return Err(NativeInstanceError::UnsafePath(path.to_path_buf())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {
                return Err(NativeInstanceError::Io {
                    context: "inspect instance destination",
                    path: path.to_path_buf(),
                });
            }
        }
        let file = NativeInstanceFile {
            schema: "artisan-instance-v2".to_string(),
            version: 2,
            database_path: self.database_path.clone(),
            custody_path: self.custody_path.clone(),
            readiness_path: self.readiness_path.clone(),
            credentials_manifest: self.credentials_manifest.clone(),
            listener: NativeListenerFile {
                admission_timeout_ms: self.listener.admission_timeout_ms,
                handshake_timeout_ms: self.listener.handshake_timeout_ms,
                request_timeout_ms: self.listener.request_timeout_ms,
                drain_timeout_ms: self.listener.drain_timeout_ms,
                admission_capacity: self.listener.admission_capacity,
                requests_per_connection: self.listener.requests_per_connection,
            },
        };
        let bytes =
            serde_json::to_vec_pretty(&file).map_err(|_| NativeInstanceError::InvalidManifest)?;
        write_native_atomic(path, &bytes)
    }

    pub fn load_from_home(home: &Path) -> NativeResult<Self> {
        Self::load(&Self::native_path(home))
    }

    pub fn write_to_home(&self, home: &Path) -> NativeResult<()> {
        self.write(&Self::native_path(home))
    }
}

pub fn load_native_config(path: &Path) -> NativeResult<NativeInstanceConfig> {
    NativeInstanceConfig::load(path)
}

pub fn write_native_config(path: &Path, config: &NativeInstanceConfig) -> NativeResult<()> {
    config.write(path)
}

struct NativeScopedTemp {
    path: PathBuf,
    armed: bool,
}

impl NativeScopedTemp {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for NativeScopedTemp {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn encode_nonce_hex(nonce: &[u8; 16]) -> String {
    let mut encoded = String::with_capacity(32);
    for &byte in nonce {
        encoded.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('?'));
        encoded.push(char::from_digit(u32::from(byte & 0x0f), 16).unwrap_or('?'));
    }
    encoded
}

fn inspect_native_destination(path: &Path) -> NativeResult<Option<NativeFileId>> {
    check_ancestors_all(path, false)?;
    match fs::symlink_metadata(path) {
        Ok(meta) if metadata_is_symlink_or_reparse(&meta) || meta.is_dir() => {
            Err(NativeInstanceError::UnsafePath(path.to_path_buf()))
        }
        Ok(meta) if meta.is_file() => native_file_id(path).map(Some),
        Ok(_) => Err(NativeInstanceError::UnsafePath(path.to_path_buf())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(NativeInstanceError::Io {
            context: "inspect instance destination",
            path: path.to_path_buf(),
        }),
    }
}

fn prepare_native_temp(
    directory: &Path,
    path: &Path,
    bytes: &[u8],
) -> NativeResult<(PathBuf, NativeScopedTemp, NativeFileId)> {
    let mut nonce = [0_u8; 16];
    getrandom::fill(&mut nonce).map_err(|_| NativeInstanceError::InvalidManifest)?;
    let nonce_hex = encode_nonce_hex(&nonce);
    let base_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("instance");
    let temporary = directory.join(format!(".{base_name}.{nonce_hex}.tmp"));
    let guard = NativeScopedTemp::new(temporary.clone());
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .map_err(|_| NativeInstanceError::Io {
            context: "create temporary instance file",
            path: temporary.clone(),
        })?;
    file.write_all(bytes).map_err(|_| NativeInstanceError::Io {
        context: "write temporary instance file",
        path: temporary.clone(),
    })?;
    file.sync_all().map_err(|_| NativeInstanceError::Io {
        context: "sync temporary instance file",
        path: temporary.clone(),
    })?;
    let temp_id = native_file_id_from_file(&file)?;
    drop(file);
    Ok((temporary, guard, temp_id))
}

fn verify_native_destination(
    path: &Path,
    pre_existing_id: Option<NativeFileId>,
) -> NativeResult<()> {
    check_ancestors_all(path, false)?;
    match fs::symlink_metadata(path) {
        Ok(meta) if metadata_is_symlink_or_reparse(&meta) || meta.is_dir() => {
            Err(NativeInstanceError::UnsafePath(path.to_path_buf()))
        }
        Ok(meta) if meta.is_file() => {
            if let Some(expected) = pre_existing_id {
                let current = native_file_id(path)?;
                if current == expected {
                    Ok(())
                } else {
                    Err(NativeInstanceError::UnsafePath(path.to_path_buf()))
                }
            } else {
                Err(NativeInstanceError::UnsafePath(path.to_path_buf()))
            }
        }
        Ok(_) => Err(NativeInstanceError::UnsafePath(path.to_path_buf())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if pre_existing_id.is_some() {
                Err(NativeInstanceError::UnsafePath(path.to_path_buf()))
            } else {
                Ok(())
            }
        }
        Err(_) => Err(NativeInstanceError::Io {
            context: "inspect instance destination",
            path: path.to_path_buf(),
        }),
    }
}

fn sync_native_directory(directory: &Path) -> NativeResult<()> {
    #[cfg(unix)]
    {
        fs::File::open(directory)
            .and_then(|dir| dir.sync_all())
            .map_err(|_| NativeInstanceError::Io {
                context: "sync directory",
                path: directory.to_path_buf(),
            })?;
    }
    #[cfg(windows)]
    {
        // The temporary file is flushed before atomic activation. Keep the
        // post-publication path/reparse check without claiming a directory
        // flush that the safe Windows file API cannot provide here.
        let metadata = fs::symlink_metadata(directory).map_err(|_| NativeInstanceError::Io {
            context: "inspect directory",
            path: directory.to_path_buf(),
        })?;
        if metadata_is_symlink_or_reparse(&metadata) || !metadata.is_dir() {
            return Err(NativeInstanceError::UnsafePath(directory.to_path_buf()));
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        fs::File::open(directory)
            .and_then(|dir| dir.sync_all())
            .map_err(|_| NativeInstanceError::Io {
                context: "sync directory",
                path: directory.to_path_buf(),
            })?;
    }
    Ok(())
}

fn write_native_atomic(path: &Path, bytes: &[u8]) -> NativeResult<()> {
    let directory = path
        .parent()
        .ok_or_else(|| NativeInstanceError::InvalidPath(path.to_path_buf()))?;
    let pre_existing_id = inspect_native_destination(path)?;
    fs::create_dir_all(directory).map_err(|_| NativeInstanceError::Io {
        context: "create directory",
        path: directory.to_path_buf(),
    })?;
    check_ancestors_all(path, false)?;
    let (temporary, mut guard, temp_id) = prepare_native_temp(directory, path, bytes)?;
    verify_native_destination(path, pre_existing_id)?;
    fs::rename(&temporary, path).map_err(|_| NativeInstanceError::Io {
        context: "activate instance file",
        path: path.to_path_buf(),
    })?;
    guard.disarm();
    let dest_id = native_file_id(path)?;
    if dest_id != temp_id {
        return Err(NativeInstanceError::UnsafePath(path.to_path_buf()));
    }
    check_ancestors_all(path, true)?;
    sync_native_directory(directory)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_layout(label: &str) -> Layout {
        let root = std::env::temp_dir().join(format!(
            "artisan-cli-{label}-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Layout {
            manifest: root.join("installation.json"),
            root,
        }
    }

    #[test]
    fn configs_without_the_flag_never_serve_the_frontend() {
        let config: InstanceConfig = serde_json::from_str(
            r#"{"data_root":"C:/data","listen_host":"127.0.0.1","listen_port":0,"mode":"local","version":1}"#,
        )
        .expect("legacy instance config");
        assert!(!config.serve_frontend);
    }

    #[test]
    fn private_writer_replaces_regular_files_without_leaving_temporary_files() {
        let directory =
            std::env::temp_dir().join(format!("artisan-cli-instance-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir(&directory).unwrap();
        let destination = directory.join("config.json");
        write_private_json(&destination, &serde_json::json!({ "version": 1 })).unwrap();
        write_private_json(&destination, &serde_json::json!({ "version": 2 })).unwrap();
        let value: serde_json::Value = read_json(&destination).unwrap();
        assert_eq!(value["version"], 2);
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn a_single_legacy_profile_migrates_to_the_home_root() {
        let layout = temporary_layout("migrate-single");
        let profile = layout.root.join("profiles").join("browser-dev");
        fs::create_dir_all(profile.join("data")).unwrap();
        fs::write(profile.join("config.json"), b"{\"version\":1}").unwrap();
        fs::write(profile.join("secrets.json"), b"{\"version\":1}").unwrap();
        fs::write(profile.join("data").join("artisan.sqlite"), b"db").unwrap();

        let paths = paths(&layout).unwrap();

        assert!(paths.config.is_file());
        assert!(layout.root.join("secrets.json").is_file());
        assert!(layout.root.join("data").join("artisan.sqlite").is_file());
        assert!(!layout.root.join("profiles").exists());
        fs::remove_dir_all(layout.root).unwrap();
    }

    #[test]
    fn multiple_legacy_profiles_fail_with_an_actionable_error() {
        let layout = temporary_layout("migrate-multiple");
        fs::create_dir_all(layout.root.join("profiles").join("default")).unwrap();
        fs::create_dir_all(layout.root.join("profiles").join("team-1")).unwrap();

        let error = paths(&layout).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("default"), "{message}");
        assert!(message.contains("team-1"), "{message}");
        assert!(message.contains("delete all but one"), "{message}");
        fs::remove_dir_all(layout.root).unwrap();
    }

    #[test]
    fn a_migrated_home_ignores_leftover_legacy_directories() {
        let layout = temporary_layout("migrate-noop");
        fs::write(layout.root.join("config.json"), b"{\"version\":1}").unwrap();
        fs::create_dir_all(layout.root.join("profiles").join("default")).unwrap();
        fs::create_dir_all(layout.root.join("profiles").join("team-1")).unwrap();

        let paths = paths(&layout).unwrap();

        assert!(paths.config.is_file());
        fs::remove_dir_all(layout.root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn private_writer_uses_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let directory = std::env::temp_dir().join(format!(
            "artisan-cli-instance-mode-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir(&directory).unwrap();
        let destination = directory.join("secrets.json");
        write_private_json(&destination, &serde_json::json!({ "token": "secret" })).unwrap();
        assert_eq!(
            fs::metadata(&destination).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn private_writer_rejects_symlink_destinations() {
        use std::os::unix::fs::symlink;

        let directory = std::env::temp_dir().join(format!(
            "artisan-cli-instance-link-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir(&directory).unwrap();
        let target = directory.join("target");
        fs::write(&target, "unchanged").unwrap();
        let destination = directory.join("secrets.json");
        symlink(&target, &destination).unwrap();
        assert!(write_private_json(&destination, &serde_json::json!({})).is_err());
        assert_eq!(fs::read_to_string(target).unwrap(), "unchanged");
        fs::remove_dir_all(directory).unwrap();
    }
}
