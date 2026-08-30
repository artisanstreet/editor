use std::{
    fs::{self, OpenOptions},
    io::Write,
    num::NonZeroU32,
    path::{Path, PathBuf},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};

use crate::{CliError, Result, error::io, paths::Layout};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct InstanceConfig {
    pub data_root: PathBuf,
    pub listen_host: String,
    pub listen_port: u16,
    pub mode: ForgeMode,
    /// Static web hosting is a development capability. Installed homes
    /// default to a control-surface-only Forge; the Electron editor renders
    /// the bundled frontend instead of a Forge-served page.
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

/// Resolves the home's single Forge instance files. This is the path
/// resolution choke point, so the legacy `profiles/<name>/` layout migrates
/// here before any caller reads or writes an instance file.
pub fn paths(layout: &Layout) -> Result<InstancePaths> {
    migrate_legacy_profiles(layout)?;
    Ok(InstancePaths {
        config: layout.root.join("config.json"),
        secrets: layout.root.join("secrets.json"),
        state: layout.root.join("state.json"),
        log: layout.root.join("forge.log"),
    })
}

/// Moves a single legacy `profiles/<name>/` directory's contents to the home
/// root. A home that already has a root `config.json` is current and skipped.
/// More than one legacy profile cannot be merged automatically, so the user
/// must delete all but one before any command proceeds.
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
    // The installer places ARTISAN_HOME in a current-user directory. Rust's
    // safe standard library has no Windows ACL API; do not broaden its ACL.
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeInstanceError {
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

impl NativeInstanceConfig {
    pub fn new(
        database_path: PathBuf,
        custody_path: PathBuf,
        readiness_path: PathBuf,
        credentials_manifest: PathBuf,
        listener: NativeListenerConfig,
    ) -> Result<Self, NativeInstanceError> {
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

    pub fn load(path: &Path) -> Result<Self, NativeInstanceError> {
        reject_native_symlink_parent(path)?;
        let pre_meta = fs::symlink_metadata(path).map_err(|_| NativeInstanceError::Io {
            context: "inspect instance file",
            path: path.to_path_buf(),
        })?;
        if pre_meta.file_type().is_symlink() || !pre_meta.is_file() {
            return Err(NativeInstanceError::UnsafePath(path.to_path_buf()));
        }
        let pre_id = native_file_id(path).ok();
        let bytes = fs::read(path).map_err(|_| NativeInstanceError::Io {
            context: "read instance file",
            path: path.to_path_buf(),
        })?;
        reject_native_symlink_parent(path)?;
        let post_meta = fs::symlink_metadata(path).map_err(|_| NativeInstanceError::Io {
            context: "inspect instance file",
            path: path.to_path_buf(),
        })?;
        if post_meta.file_type().is_symlink() || !post_meta.is_file() {
            return Err(NativeInstanceError::UnsafePath(path.to_path_buf()));
        }
        if let (Some(before), Some(after)) = (pre_id, native_file_id(path).ok()) {
            if before != after {
                return Err(NativeInstanceError::UnsafePath(path.to_path_buf()));
            }
        }
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

    pub fn write(&self, path: &Path) -> Result<(), NativeInstanceError> {
        reject_native_symlink_parent(path)?;
        match fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_symlink() || meta.is_dir() => {
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

    pub fn load_from_home(home: &Path) -> Result<Self, NativeInstanceError> {
        Self::load(&Self::native_path(home))
    }

    pub fn write_to_home(&self, home: &Path) -> Result<(), NativeInstanceError> {
        self.write(&Self::native_path(home))
    }
}

pub fn load_native_config(path: &Path) -> Result<NativeInstanceConfig, NativeInstanceError> {
    NativeInstanceConfig::load(path)
}

pub fn write_native_config(
    path: &Path,
    config: &NativeInstanceConfig,
) -> Result<(), NativeInstanceError> {
    config.write(path)
}

fn reject_native_symlink(path: &Path) -> Result<(), NativeInstanceError> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            Err(NativeInstanceError::UnsafePath(path.to_path_buf()))
        }
        Ok(meta) if !meta.is_file() => Err(NativeInstanceError::UnsafePath(path.to_path_buf())),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(NativeInstanceError::Io {
            context: "inspect instance file",
            path: path.to_path_buf(),
        }),
    }
}

fn reject_unsafe_native_destination(path: &Path) -> Result<(), NativeInstanceError> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_file() => {
            Err(NativeInstanceError::UnsafePath(path.to_path_buf()))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(NativeInstanceError::Io {
            context: "inspect instance destination",
            path: path.to_path_buf(),
        }),
    }
}

fn reject_native_symlink_parent(path: &Path) -> Result<(), NativeInstanceError> {
    let parent = path
        .parent()
        .ok_or_else(|| NativeInstanceError::InvalidPath(path.to_path_buf()))?;
    for ancestor in parent.ancestors() {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        match fs::symlink_metadata(ancestor) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(NativeInstanceError::UnsafePath(ancestor.to_path_buf()));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {
                return Err(NativeInstanceError::Io {
                    context: "inspect parent",
                    path: ancestor.to_path_buf(),
                });
            }
        }
        if ancestor == parent {
            break;
        }
    }
    Ok(())
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct NativeFileId {
    dev: u64,
    ino: u64,
}

#[cfg(windows)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct NativeFileId {
    volume: u64,
    high: u64,
    low: u64,
}

#[cfg(unix)]
fn native_file_id(path: &Path) -> Result<NativeFileId, NativeInstanceError> {
    use std::os::unix::fs::MetadataExt;
    let meta = fs::metadata(path).map_err(|_| NativeInstanceError::Io {
        context: "inspect file id",
        path: path.to_path_buf(),
    })?;
    Ok(NativeFileId {
        dev: meta.dev(),
        ino: meta.ino(),
    })
}

#[cfg(windows)]
fn native_file_id(path: &Path) -> Result<NativeFileId, NativeInstanceError> {
    use std::os::windows::fs::MetadataExt;
    let meta = fs::metadata(path).map_err(|_| NativeInstanceError::Io {
        context: "inspect file id",
        path: path.to_path_buf(),
    })?;
    Ok(NativeFileId {
        volume: meta.volume_serial_number().unwrap_or(0) as u64,
        high: meta.file_index_high().unwrap_or(0) as u64,
        low: meta.file_index_low().unwrap_or(0) as u64,
    })
}

#[cfg(not(any(unix, windows)))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct NativeFileId {
    dummy: u64,
}

#[cfg(not(any(unix, windows)))]
fn native_file_id(_path: &Path) -> Result<NativeFileId, NativeInstanceError> {
    Ok(NativeFileId { dummy: 0 })
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

fn write_native_atomic(path: &Path, bytes: &[u8]) -> Result<(), NativeInstanceError> {
    let directory = path
        .parent()
        .ok_or_else(|| NativeInstanceError::InvalidPath(path.to_path_buf()))?;
    reject_native_symlink_parent(path)?;
    let pre_existing_id = match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() || meta.is_dir() => {
            return Err(NativeInstanceError::UnsafePath(path.to_path_buf()));
        }
        Ok(meta) if meta.is_file() => Some(native_file_id(path)?),
        Ok(_) => return Err(NativeInstanceError::UnsafePath(path.to_path_buf())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => {
            return Err(NativeInstanceError::Io {
                context: "inspect instance destination",
                path: path.to_path_buf(),
            });
        }
    };
    fs::create_dir_all(directory).map_err(|_| NativeInstanceError::Io {
        context: "create directory",
        path: directory.to_path_buf(),
    })?;
    reject_native_symlink_parent(path)?;
    let mut nonce = [0_u8; 16];
    getrandom::fill(&mut nonce).map_err(|_| NativeInstanceError::InvalidManifest)?;
    let nonce_hex: String = nonce.iter().map(|b| format!("{b:02x}")).collect();
    let temp_name = format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("instance"),
        nonce_hex
    );
    let temporary = directory.join(temp_name);
    let mut guard = NativeScopedTemp::new(temporary.clone());
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
    drop(file);
    reject_native_symlink_parent(path)?;
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() || meta.is_dir() => {
            return Err(NativeInstanceError::UnsafePath(path.to_path_buf()));
        }
        Ok(meta) if meta.is_file() => {
            if let Some(expected) = pre_existing_id {
                let current = native_file_id(path)?;
                if current != expected {
                    return Err(NativeInstanceError::UnsafePath(path.to_path_buf()));
                }
            }
        }
        Ok(_) => return Err(NativeInstanceError::UnsafePath(path.to_path_buf())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if pre_existing_id.is_some() {
                return Err(NativeInstanceError::UnsafePath(path.to_path_buf()));
            }
        }
        Err(_) => {
            return Err(NativeInstanceError::Io {
                context: "inspect instance destination",
                path: path.to_path_buf(),
            });
        }
    }
    fs::rename(&temporary, path).map_err(|_| NativeInstanceError::Io {
        context: "activate instance file",
        path: path.to_path_buf(),
    })?;
    guard.disarm();
    #[cfg(unix)]
    {
        let _ = fs::File::open(directory).and_then(|dir| dir.sync_all());
    }
    Ok(())
}

#[cfg(test)]
mod native_tests {
    use super::*;
    use std::num::NonZeroU32;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "artisan-native-{label}-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn sample_listener() -> NativeListenerConfig {
        NativeListenerConfig::new(
            1000,
            2000,
            3000,
            4000,
            NonZeroU32::new(10).unwrap(),
            NonZeroU32::new(20).unwrap(),
        )
    }

    fn sample_config(home: &Path) -> NativeInstanceConfig {
        NativeInstanceConfig::new(
            home.join("data").join("artisan.sqlite"),
            home.join("custody").join("lock"),
            home.join("readiness").join("ready"),
            home.join("credentials").join("manifest.json"),
            sample_listener(),
        )
        .unwrap()
    }

    #[test]
    fn native_instance_exact_round_trip() {
        let home = temp_dir("roundtrip");
        let config = sample_config(&home);
        let path = NativeInstanceConfig::native_path(&home);
        config.write(&path).unwrap();
        let loaded = NativeInstanceConfig::load(&path).unwrap();
        assert_eq!(config, loaded);
        let bytes = fs::read(&path).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["schema"], "artisan-instance-v2");
        assert_eq!(value["version"], 2);
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn native_rejects_unknown_and_duplicate_fields() {
        let home = temp_dir("unknown");
        let path = home.join("instance-v2.json");
        fs::write(
            &path,
            br#"{"schema":"artisan-instance-v2","version":2,"database_path":"/tmp/a","custody_path":"/tmp/b","readiness_path":"/tmp/c","credentials_manifest":"/tmp/d","listener":{"admission_timeout_ms":1,"handshake_timeout_ms":1,"request_timeout_ms":1,"drain_timeout_ms":1,"admission_capacity":1,"requests_per_connection":1},"extra":"field"}"#,
        )
        .unwrap();
        assert!(NativeInstanceConfig::load(&path).is_err());
        fs::write(
            &path,
            br#"{"schema":"artisan-instance-v2","version":2,"version":2,"database_path":"/tmp/a","custody_path":"/tmp/b","readiness_path":"/tmp/c","credentials_manifest":"/tmp/d","listener":{"admission_timeout_ms":1,"handshake_timeout_ms":1,"request_timeout_ms":1,"drain_timeout_ms":1,"admission_capacity":1,"requests_per_connection":1}}"#,
        )
        .unwrap();
        assert!(NativeInstanceConfig::load(&path).is_err());
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn native_rejects_version_and_relative_paths() {
        let home = temp_dir("version");
        let path = home.join("instance-v2.json");
        fs::write(
            &path,
            br#"{"schema":"artisan-instance-v2","version":1,"database_path":"/tmp/a","custody_path":"/tmp/b","readiness_path":"/tmp/c","credentials_manifest":"/tmp/d","listener":{"admission_timeout_ms":1,"handshake_timeout_ms":1,"request_timeout_ms":1,"drain_timeout_ms":1,"admission_capacity":1,"requests_per_connection":1}}"#,
        )
        .unwrap();
        assert!(NativeInstanceConfig::load(&path).is_err());
        let listener = sample_listener();
        assert!(
            NativeInstanceConfig::new(
                PathBuf::from("relative/path"),
                PathBuf::from("/tmp/b"),
                PathBuf::from("/tmp/c"),
                PathBuf::from("/tmp/d"),
                listener
            )
            .is_err()
        );
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn native_rejects_zero_capacity() {
        let home = temp_dir("zero");
        let path = home.join("instance-v2.json");
        fs::write(
            &path,
            br#"{"schema":"artisan-instance-v2","version":2,"database_path":"/tmp/a","custody_path":"/tmp/b","readiness_path":"/tmp/c","credentials_manifest":"/tmp/d","listener":{"admission_timeout_ms":1,"handshake_timeout_ms":1,"request_timeout_ms":1,"drain_timeout_ms":1,"admission_capacity":0,"requests_per_connection":1}}"#,
        )
        .unwrap();
        assert!(NativeInstanceConfig::load(&path).is_err());
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn native_listener_values_unchanged() {
        let listener = NativeListenerConfig::new(
            111,
            222,
            333,
            444,
            NonZeroU32::new(5).unwrap(),
            NonZeroU32::new(6).unwrap(),
        );
        assert_eq!(listener.admission_timeout_ms(), 111);
        assert_eq!(listener.handshake_timeout_ms(), 222);
        assert_eq!(listener.request_timeout_ms(), 333);
        assert_eq!(listener.drain_timeout_ms(), 444);
        assert_eq!(listener.admission_capacity().get(), 5);
        assert_eq!(listener.requests_per_connection().get(), 6);
    }

    #[test]
    fn native_no_default_and_private_fields() {
        let home = temp_dir("private");
        let config = sample_config(&home);
        let debug = format!("{config:?}");
        assert!(debug.contains("database_path"));
        let path = NativeInstanceConfig::native_path(&home);
        config.write(&path).unwrap();
        let mtime_before = fs::metadata(&path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let loaded = NativeInstanceConfig::load(&path).unwrap();
        loaded.write(&path).unwrap();
        let mtime_after = fs::metadata(&path).unwrap().modified().unwrap();
        assert!(mtime_after >= mtime_before);
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn native_redacted_debug() {
        let err = NativeInstanceError::InvalidManifest;
        let debug = format!("{err:?}");
        assert!(debug.contains("InvalidManifest"));
        assert!(!debug.contains("secret"));
        let display = format!("{err}");
        assert!(!display.contains("secret"));
    }

    #[test]
    #[cfg(unix)]
    fn native_unix_modes() {
        use std::os::unix::fs::PermissionsExt;
        let home = temp_dir("native-modes");
        let config = sample_config(&home);
        let path = NativeInstanceConfig::native_path(&home);
        config.write(&path).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn native_parent_symlink_rejected() {
        let home = temp_dir("parent-symlink");
        let config = sample_config(&home);
        let path = NativeInstanceConfig::native_path(&home);
        config.write(&path).unwrap();
        let parent = path.parent().unwrap().to_path_buf();
        let real = home.join("real");
        fs::create_dir_all(&real).unwrap();
        fs::rename(&parent, &real).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&real, &parent).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&real, &parent).unwrap_or_else(|_| {
            fs::create_dir_all(&parent).unwrap();
            return;
        });
        assert!(NativeInstanceConfig::load(&path).is_err());
        let _ = fs::remove_file(&parent);
        let _ = fs::remove_dir_all(&real);
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn native_write_replaces_with_identity_fence() {
        let home = temp_dir("replace-fence");
        let config = sample_config(&home);
        let path = NativeInstanceConfig::native_path(&home);
        config.write(&path).unwrap();
        let first_id = native_file_id(&path).unwrap();
        let mut config2 = config.clone();
        config2 = NativeInstanceConfig::new(
            home.join("data2").join("artisan.sqlite"),
            config2.custody_path().to_path_buf(),
            config2.readiness_path().to_path_buf(),
            config2.credentials_manifest().to_path_buf(),
            sample_listener(),
        )
        .unwrap();
        config2.write(&path).unwrap();
        let second_id = native_file_id(&path).unwrap();
        assert!(first_id != second_id);
        let loaded = NativeInstanceConfig::load(&path).unwrap();
        assert_eq!(loaded.database_path(), config2.database_path());
        fs::remove_dir_all(home).unwrap();
    }
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
