#![allow(clippy::too_many_lines)]

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr},
    path::{Path, PathBuf},
    time::Duration,
};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ForgeCredentialError {
    InvalidHome(PathBuf),
    UnsafePath(PathBuf),
    Io {
        context: &'static str,
        path: PathBuf,
    },
    ManifestMalformed,
    ManifestSchema,
    ManifestVersion,
    ManifestTraversal,
    ManifestUnknownField,
    ManifestDuplicateField,
    PartialBundle,
    InvalidCapability {
        path: PathBuf,
    },
    InvalidCertificate,
    KeyMismatch,
    WindowsAcl,
    Provisioning,
}

impl std::fmt::Display for ForgeCredentialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidHome(path) => write!(f, "invalid Artisan home: {}", path.display()),
            Self::UnsafePath(path) => write!(
                f,
                "refusing unsafe filesystem operation on {}",
                path.display()
            ),
            Self::Io { context, path } => write!(f, "{context} at {}: [REDACTED]", path.display()),
            Self::ManifestMalformed => write!(f, "invalid credential manifest: malformed"),
            Self::ManifestSchema => write!(f, "invalid credential manifest: schema"),
            Self::ManifestVersion => write!(f, "invalid credential manifest: version"),
            Self::ManifestTraversal => write!(f, "invalid credential manifest: traversal"),
            Self::ManifestUnknownField => {
                write!(f, "invalid credential manifest: unknown field")
            }
            Self::ManifestDuplicateField => {
                write!(f, "invalid credential manifest: duplicate field")
            }
            Self::PartialBundle => write!(f, "partial credential bundle"),
            Self::InvalidCapability { path } => write!(
                f,
                "capability at {} has invalid length (expected 32)",
                path.display()
            ),
            Self::InvalidCertificate => write!(f, "invalid certificate"),
            Self::KeyMismatch => write!(f, "private key does not match certificate"),
            Self::WindowsAcl => write!(f, "Windows ACL error"),
            Self::Provisioning => write!(f, "provisioning failed"),
        }
    }
}

impl std::fmt::Debug for ForgeCredentialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidHome(path) => f
                .debug_tuple("InvalidHome")
                .field(&path.display().to_string())
                .finish(),
            Self::UnsafePath(path) => f
                .debug_tuple("UnsafePath")
                .field(&path.display().to_string())
                .finish(),
            Self::Io { context, path } => f
                .debug_struct("Io")
                .field("context", context)
                .field("path", &path.display().to_string())
                .finish(),
            Self::ManifestMalformed => f.debug_tuple("ManifestMalformed").finish(),
            Self::ManifestSchema => f.debug_tuple("ManifestSchema").finish(),
            Self::ManifestVersion => f.debug_tuple("ManifestVersion").finish(),
            Self::ManifestTraversal => f.debug_tuple("ManifestTraversal").finish(),
            Self::ManifestUnknownField => f.debug_tuple("ManifestUnknownField").finish(),
            Self::ManifestDuplicateField => f.debug_tuple("ManifestDuplicateField").finish(),
            Self::PartialBundle => f.debug_tuple("PartialBundle").finish(),
            Self::InvalidCapability { path } => f
                .debug_struct("InvalidCapability")
                .field("path", &path.display().to_string())
                .finish(),
            Self::InvalidCertificate => f.debug_tuple("InvalidCertificate").finish(),
            Self::KeyMismatch => f.debug_tuple("KeyMismatch").finish(),
            Self::WindowsAcl => f.debug_tuple("WindowsAcl").finish(),
            Self::Provisioning => f.debug_tuple("Provisioning").finish(),
        }
    }
}

impl std::error::Error for ForgeCredentialError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ForgeCredentialPaths {
    manifest: PathBuf,
    capability: PathBuf,
    certificates: Vec<PathBuf>,
    private_key: PathBuf,
}

impl ForgeCredentialPaths {
    pub fn new(home: &Path) -> Result<Self, ForgeCredentialError> {
        validate_home(home)?;
        let credentials_dir = home.join("credentials");
        Ok(Self {
            manifest: credentials_dir.join("manifest.json"),
            capability: credentials_dir.join("bootstrap-capability.bin"),
            certificates: vec![credentials_dir.join("localhost-leaf.der")],
            private_key: credentials_dir.join("localhost-key.pkcs8.der"),
        })
    }

    pub fn from_home(home: &Path) -> Result<Self, ForgeCredentialError> {
        Self::new(home)
    }

    pub fn manifest_path(&self) -> &Path {
        &self.manifest
    }

    pub fn manifest(&self) -> &Path {
        &self.manifest
    }

    pub fn capability_path(&self) -> &Path {
        &self.capability
    }

    pub fn capability(&self) -> &Path {
        &self.capability
    }

    pub fn certificate_paths(&self) -> &[PathBuf] {
        &self.certificates
    }

    pub fn certificates(&self) -> &[PathBuf] {
        &self.certificates
    }

    pub fn private_key_path(&self) -> &Path {
        &self.private_key
    }

    pub fn private_key(&self) -> &Path {
        &self.private_key
    }

    pub fn credentials_dir(&self) -> PathBuf {
        self.manifest
            .parent()
            .expect("manifest has parent")
            .to_path_buf()
    }

    pub fn lock_path(&self) -> PathBuf {
        self.credentials_dir().join(".provision.lock")
    }
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct CredentialManifest {
    schema: String,
    version: u64,
    bootstrap_capability: String,
    certificate_chain: Vec<String>,
    private_key: String,
}

struct ProvisionalMaterial {
    capability: Zeroizing<[u8; 32]>,
    private_key: Zeroizing<Vec<u8>>,
    certificate: Vec<u8>,
}

impl std::fmt::Debug for ProvisionalMaterial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProvisionalMaterial")
            .field("capability", &"[REDACTED]")
            .field("private_key", &"[REDACTED]")
            .field("certificate", &"[REDACTED]")
            .finish()
    }
}

fn validate_home(home: &Path) -> Result<(), ForgeCredentialError> {
    if !home.is_absolute() {
        return Err(ForgeCredentialError::InvalidHome(home.to_path_buf()));
    }
    if home.as_os_str().is_empty() {
        return Err(ForgeCredentialError::InvalidHome(home.to_path_buf()));
    }
    Ok(())
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

fn check_ancestors_all(path: &Path, must_exist: bool) -> Result<(), ForgeCredentialError> {
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

fn reject_symlink_chain(path: &Path) -> Result<(), ForgeCredentialError> {
    check_ancestors_all(path, false)?;
    match fs::symlink_metadata(path) {
        Ok(meta) if metadata_is_symlink_or_reparse(&meta) => {
            Err(ForgeCredentialError::UnsafePath(path.to_path_buf()))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(ForgeCredentialError::Io {
            context: "inspect path",
            path: path.to_path_buf(),
        }),
    }
}

fn is_safe_filename(name: &str) -> bool {
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
fn check_file_mode(path: &Path) -> Result<(), ForgeCredentialError> {
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

#[cfg(not(unix))]
fn check_dir_mode(_path: &Path) -> Result<(), ForgeCredentialError> {
    Ok(())
}

#[cfg(not(unix))]
fn check_file_mode(_path: &Path) -> Result<(), ForgeCredentialError> {
    Ok(())
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileId {
    dev: u64,
    ino: u64,
}

#[cfg(windows)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileId {
    volume: u64,
    index_high: u64,
    index_low: u64,
}

#[cfg(unix)]
fn file_id(path: &Path) -> Result<FileId, ForgeCredentialError> {
    use std::os::unix::fs::MetadataExt;
    let meta = fs::metadata(path).map_err(|_| ForgeCredentialError::Io {
        context: "inspect file id",
        path: path.to_path_buf(),
    })?;
    Ok(FileId {
        dev: meta.dev(),
        ino: meta.ino(),
    })
}

#[cfg(windows)]
fn file_id(path: &Path) -> Result<FileId, ForgeCredentialError> {
    use std::os::windows::fs::MetadataExt;
    let meta = fs::metadata(path).map_err(|_| ForgeCredentialError::Io {
        context: "inspect file id",
        path: path.to_path_buf(),
    })?;
    let volume = meta
        .volume_serial_number()
        .ok_or(ForgeCredentialError::Provisioning)? as u64;
    let high = meta
        .file_index_high()
        .ok_or(ForgeCredentialError::Provisioning)? as u64;
    let low = meta
        .file_index_low()
        .ok_or(ForgeCredentialError::Provisioning)? as u64;
    if volume == 0 && high == 0 && low == 0 {
        return Err(ForgeCredentialError::Provisioning);
    }
    Ok(FileId {
        volume,
        index_high: high,
        index_low: low,
    })
}

#[cfg(not(any(unix, windows)))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FileId;

#[cfg(not(any(unix, windows)))]
fn file_id(_path: &Path) -> Result<FileId, ForgeCredentialError> {
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
    use std::os::windows::fs::MetadataExt;
    let meta = file.metadata().map_err(|_| ForgeCredentialError::Io {
        context: "inspect file id",
        path: PathBuf::from("<handle>"),
    })?;
    let volume = meta
        .volume_serial_number()
        .ok_or(ForgeCredentialError::Provisioning)? as u64;
    let high = meta
        .file_index_high()
        .ok_or(ForgeCredentialError::Provisioning)? as u64;
    let low = meta
        .file_index_low()
        .ok_or(ForgeCredentialError::Provisioning)? as u64;
    if volume == 0 && high == 0 && low == 0 {
        return Err(ForgeCredentialError::Provisioning);
    }
    Ok(FileId {
        volume,
        index_high: high,
        index_low: low,
    })
}

#[cfg(not(any(unix, windows)))]
fn file_id_from_file(_file: &File) -> Result<FileId, ForgeCredentialError> {
    Err(ForgeCredentialError::Provisioning)
}

struct CreatedFile {
    path: PathBuf,
    id: FileId,
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

#[cfg(windows)]
fn hidden_output(
    exe: &str,
    args: &[&str],
    timeout: Duration,
) -> Result<std::process::Output, ForgeCredentialError> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut command = std::process::Command::new(exe);
    for arg in args {
        command.arg(arg);
    }
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW);
    let mut child = command
        .spawn()
        .map_err(|_| ForgeCredentialError::Provisioning)?;
    let start = std::time::Instant::now();
    loop {
        if start.elapsed() > timeout {
            let _ = child.kill();
            let reap_start = std::time::Instant::now();
            let reap_deadline = Duration::from_secs(2);
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) => {
                        if reap_start.elapsed() > reap_deadline {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
            let _ = child.try_wait();
            return Err(ForgeCredentialError::Provisioning);
        }
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => return Err(ForgeCredentialError::Provisioning),
        }
    }
    let output = child
        .wait_with_output()
        .map_err(|_| ForgeCredentialError::Provisioning)?;
    Ok(output)
}

#[cfg(windows)]
fn resolve_current_sid() -> Result<String, ForgeCredentialError> {
    let output = hidden_output(
        "whoami.exe",
        &["/user", "/fo", "csv", "/nh"],
        Duration::from_secs(5),
    )?;
    if !output.status.success() {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    let text = String::from_utf8_lossy(&output.stdout).to_string();
    let line = text
        .lines()
        .next()
        .ok_or(ForgeCredentialError::WindowsAcl)?;
    let sid = line
        .split("\",\"")
        .nth(1)
        .or_else(|| line.split(',').nth(1))
        .ok_or(ForgeCredentialError::WindowsAcl)?;
    let sid = sid.trim().trim_matches('"').trim().to_string();
    if !is_valid_sid(&sid) {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    Ok(sid)
}

#[cfg(windows)]
fn is_valid_sid(sid: &str) -> bool {
    if !sid.starts_with("S-1-") {
        return false;
    }
    if sid.contains(' ') || sid.contains('/') || sid.contains('\\') {
        return false;
    }
    let parts: Vec<&str> = sid.split('-').collect();
    if parts.len() < 3 {
        return false;
    }
    if parts[0] != "S" || parts[1] != "1" {
        return false;
    }
    for part in &parts[2..] {
        if part.is_empty() || part.parse::<u64>().is_err() {
            return false;
        }
    }
    true
}

fn parse_icacls_output(output: &str, expected_sid: &str) -> Result<(), ForgeCredentialError> {
    parse_icacls_strict(output, expected_sid, true)
}

fn parse_icacls_strict(
    output: &str,
    expected_sid: &str,
    expect_dir: bool,
) -> Result<(), ForgeCredentialError> {
    let expected_lower = expected_sid.to_ascii_lowercase();
    let mut ace_lines: Vec<String> = Vec::new();
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !trimmed.contains(':') || !trimmed.contains('(') {
            continue;
        }
        if trimmed.to_ascii_lowercase().starts_with("successfully") {
            continue;
        }
        ace_lines.push(trimmed.to_string());
    }
    if ace_lines.is_empty() {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    if ace_lines.len() != 1 {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    let ace = ace_lines[0].to_ascii_lowercase();
    if !ace.contains(&expected_lower) {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    if ace.contains("deny") {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    if ace.contains("(i)") {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    if !ace.contains("(f)") {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    if expect_dir {
        if !ace.contains("(oi)") || !ace.contains("(ci)") {
            return Err(ForgeCredentialError::WindowsAcl);
        }
    } else if ace.contains("(oi)") || ace.contains("(ci)") {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    for forbidden in ["everyone", "builtin", "nt authority", "authenticated users"] {
        if ace.contains(forbidden) {
            return Err(ForgeCredentialError::WindowsAcl);
        }
    }
    let lower_output = output.to_ascii_lowercase();
    if lower_output.matches(':').count() != ace_lines.len() {
        let colon_count = lower_output.matches(':').count();
        if colon_count > ace_lines.len() {
            return Err(ForgeCredentialError::WindowsAcl);
        }
    }
    Ok(())
}

#[cfg(windows)]
fn verify_windows_dacl(path: &Path, expected_sid: &str) -> Result<(), ForgeCredentialError> {
    let path_str = path.to_string_lossy().to_string();
    let output = hidden_output("icacls.exe", &[&path_str], Duration::from_secs(5))?;
    if !output.status.success() {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    let text = String::from_utf8_lossy(&output.stdout).to_string();
    let is_dir = fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false);
    parse_icacls_strict(&text, expected_sid, is_dir)
}

#[cfg(windows)]
fn restrict_directory_windows(dir: &Path) -> Result<(), ForgeCredentialError> {
    let sid = resolve_current_sid()?;
    let dir_str = dir.to_string_lossy().to_string();
    let grant = format!("*{}:(OI)(CI)F", sid);
    let output = hidden_output(
        "icacls.exe",
        &[&dir_str, "/inheritance:r", "/grant:r", &grant],
        Duration::from_secs(5),
    )?;
    if !output.status.success() {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    verify_windows_dacl(dir, &sid)?;
    Ok(())
}

#[cfg(windows)]
fn restrict_file_windows(path: &Path, sid: &str) -> Result<(), ForgeCredentialError> {
    let path_str = path.to_string_lossy().to_string();
    let grant = format!("*{}:F", sid);
    let output = hidden_output(
        "icacls.exe",
        &[&path_str, "/inheritance:r", "/grant:r", &grant],
        Duration::from_secs(5),
    )?;
    if !output.status.success() {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    verify_windows_dacl(path, sid)
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

#[cfg(not(unix))]
fn sync_directory(_dir: &Path) -> Result<(), ForgeCredentialError> {
    Ok(())
}

fn ensure_credentials_dir(dir: &Path) -> Result<(), ForgeCredentialError> {
    check_ancestors_all(dir, false)?;
    match fs::symlink_metadata(dir) {
        Ok(meta) if metadata_is_symlink_or_reparse(&meta) => {
            return Err(ForgeCredentialError::UnsafePath(dir.to_path_buf()));
        }
        Ok(meta) if meta.is_dir() => {
            #[cfg(unix)]
            check_dir_mode(dir)?;
            #[cfg(windows)]
            {
                let sid = resolve_current_sid()?;
                verify_windows_dacl(dir, &sid)?;
            }
            Ok(())
        }
        Ok(meta) if meta.is_file() => Err(ForgeCredentialError::UnsafePath(dir.to_path_buf())),
        Ok(_) => Err(ForgeCredentialError::UnsafePath(dir.to_path_buf())),
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

fn acquire_lock(lock_path: &Path) -> Result<File, ForgeCredentialError> {
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
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(lock_path)
        .map_err(|_| ForgeCredentialError::Io {
            context: "open provision lock",
            path: lock_path.to_path_buf(),
        })?;
    file.lock_exclusive()
        .map_err(|_| ForgeCredentialError::Io {
            context: "lock provision lock",
            path: lock_path.to_path_buf(),
        })?;
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
    if let (Ok(open_meta), Ok(path_meta)) = (file.metadata(), fs::metadata(lock_path)) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if open_meta.dev() != path_meta.dev() || open_meta.ino() != path_meta.ino() {
                return Err(ForgeCredentialError::UnsafePath(lock_path.to_path_buf()));
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            let open_vol = open_meta
                .volume_serial_number()
                .ok_or(ForgeCredentialError::Provisioning)?;
            let path_vol = fs::metadata(lock_path)
                .map_err(|_| ForgeCredentialError::Provisioning)?
                .volume_serial_number()
                .ok_or(ForgeCredentialError::Provisioning)?;
            if open_vol != path_vol {
                return Err(ForgeCredentialError::UnsafePath(lock_path.to_path_buf()));
            }
            let open_idx = (
                open_meta
                    .file_index_high()
                    .ok_or(ForgeCredentialError::Provisioning)?,
                open_meta
                    .file_index_low()
                    .ok_or(ForgeCredentialError::Provisioning)?,
            );
            let path_idx = {
                let m = fs::metadata(lock_path).map_err(|_| ForgeCredentialError::Provisioning)?;
                (
                    m.file_index_high()
                        .ok_or(ForgeCredentialError::Provisioning)?,
                    m.file_index_low()
                        .ok_or(ForgeCredentialError::Provisioning)?,
                )
            };
            if open_idx != path_idx {
                return Err(ForgeCredentialError::UnsafePath(lock_path.to_path_buf()));
            }
        }
    } else {
        return Err(ForgeCredentialError::Provisioning);
    }
    #[cfg(unix)]
    check_file_mode(lock_path)?;
    #[cfg(windows)]
    {
        let sid = resolve_current_sid()?;
        let verified = verify_windows_dacl(lock_path, &sid);
        if verified.is_err() {
            let grant = format!("*{}:F", sid);
            let path_str = lock_path.to_string_lossy().to_string();
            let _ = hidden_output(
                "icacls.exe",
                &[&path_str, "/inheritance:r", "/grant:r", &grant],
                Duration::from_secs(5),
            );
            verify_windows_dacl(lock_path, &sid)?;
        }
    }
    Ok(file)
}

fn validate_manifest_bytes(
    bytes: &[u8],
    _paths: &ForgeCredentialPaths,
) -> Result<CredentialManifest, ForgeCredentialError> {
    let manifest: CredentialManifest =
        serde_json::from_slice(bytes).map_err(|_| ForgeCredentialError::ManifestMalformed)?;
    if manifest.schema != "artisan-forge-credentials-v1" {
        return Err(ForgeCredentialError::ManifestSchema);
    }
    if manifest.version != 1 {
        return Err(ForgeCredentialError::ManifestVersion);
    }
    if manifest.bootstrap_capability != "bootstrap-capability.bin" {
        return Err(ForgeCredentialError::ManifestTraversal);
    }
    if manifest.certificate_chain != vec!["localhost-leaf.der".to_string()] {
        return Err(ForgeCredentialError::ManifestTraversal);
    }
    if manifest.private_key != "localhost-key.pkcs8.der" {
        return Err(ForgeCredentialError::ManifestTraversal);
    }
    for name in std::iter::once(&manifest.bootstrap_capability)
        .chain(manifest.certificate_chain.iter())
        .chain(std::iter::once(&manifest.private_key))
    {
        if !is_safe_filename(name) {
            return Err(ForgeCredentialError::ManifestTraversal);
        }
    }
    Ok(manifest)
}

fn validate_cert_sans(cert_der: &[u8]) -> Result<(), ForgeCredentialError> {
    let (_, cert) = x509_parser::parse_x509_certificate(cert_der)
        .map_err(|_| ForgeCredentialError::InvalidCertificate)?;
    let has_san = cert
        .subject_alternative_name()
        .map_err(|_| ForgeCredentialError::InvalidCertificate)?;
    let Some(san) = has_san else {
        return Err(ForgeCredentialError::InvalidCertificate);
    };
    let mut has_dns_localhost = false;
    let mut has_ip_127 = false;
    for name in &san.value.general_names {
        match name {
            x509_parser::extensions::GeneralName::DNSName(dns) => {
                if *dns == "localhost" {
                    has_dns_localhost = true;
                }
            }
            x509_parser::extensions::GeneralName::IPAddress(bytes) => {
                if bytes.len() == 4 && bytes == &[127, 0, 0, 1] {
                    has_ip_127 = true;
                }
                if bytes.len() == 16
                    && bytes[12..] == [127, 0, 0, 1]
                    && bytes[..12] == [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
                {
                    has_ip_127 = true;
                }
            }
            _ => {}
        }
    }
    if !has_dns_localhost || !has_ip_127 {
        return Err(ForgeCredentialError::InvalidCertificate);
    }
    if cert.tbs_certificate.issuer != cert.tbs_certificate.subject {
        return Err(ForgeCredentialError::InvalidCertificate);
    }
    cert.verify_signature(None)
        .map_err(|_| ForgeCredentialError::InvalidCertificate)?;
    Ok(())
}

fn validate_key_matches_cert(key_der: &[u8], cert_der: &[u8]) -> Result<(), ForgeCredentialError> {
    let key_pair =
        rcgen::KeyPair::try_from(key_der).map_err(|_| ForgeCredentialError::InvalidCertificate)?;
    let cert_spki = {
        let (_, cert) = x509_parser::parse_x509_certificate(cert_der)
            .map_err(|_| ForgeCredentialError::InvalidCertificate)?;
        cert.tbs_certificate.subject_pki.raw.to_vec()
    };
    let key_spki = key_pair.subject_public_key_info();
    if cert_spki != key_spki {
        return Err(ForgeCredentialError::KeyMismatch);
    }
    let _ = rustls_pki_types::PrivateKeyDer::try_from(key_der.to_vec())
        .map_err(|_| ForgeCredentialError::InvalidCertificate)?;
    let _ = rustls_pki_types::CertificateDer::from(cert_der.to_vec());
    Ok(())
}

fn open_and_read(path: &Path) -> Result<Vec<u8>, ForgeCredentialError> {
    check_ancestors_all(path, true)?;
    let pre_meta = fs::symlink_metadata(path).map_err(|_| ForgeCredentialError::Io {
        context: "inspect file",
        path: path.to_path_buf(),
    })?;
    if metadata_is_symlink_or_reparse(&pre_meta) || !pre_meta.is_file() {
        return Err(ForgeCredentialError::UnsafePath(path.to_path_buf()));
    }
    let pre_id = file_id(path)?;
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
    if handle_id != pre_id {
        return Err(ForgeCredentialError::UnsafePath(path.to_path_buf()));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|_| ForgeCredentialError::Io {
            context: "read file",
            path: path.to_path_buf(),
        })?;
    check_ancestors_all(path, true)?;
    let post_meta = fs::symlink_metadata(path).map_err(|_| ForgeCredentialError::Io {
        context: "inspect file",
        path: path.to_path_buf(),
    })?;
    if metadata_is_symlink_or_reparse(&post_meta) || !post_meta.is_file() {
        return Err(ForgeCredentialError::UnsafePath(path.to_path_buf()));
    }
    let post_id = file_id(path)?;
    if post_id != pre_id || post_id != handle_id {
        return Err(ForgeCredentialError::UnsafePath(path.to_path_buf()));
    }
    Ok(bytes)
}

fn validate_existing_bundle(paths: &ForgeCredentialPaths) -> Result<bool, ForgeCredentialError> {
    let manifest_path = paths.manifest_path();
    let capability_path = paths.capability_path();
    let cert_path = &paths.certificate_paths()[0];
    let key_path = paths.private_key_path();

    let files = [manifest_path, capability_path, cert_path, key_path];
    let mut exists = Vec::new();
    let mut missing = Vec::new();
    for file in &files {
        match fs::symlink_metadata(file) {
            Ok(meta) if metadata_is_symlink_or_reparse(&meta) => {
                return Err(ForgeCredentialError::UnsafePath((*file).to_path_buf()));
            }
            Ok(meta) if meta.is_dir() => {
                return Err(ForgeCredentialError::UnsafePath((*file).to_path_buf()));
            }
            Ok(meta) if meta.is_file() => {
                check_ancestors_all(file, true)?;
                exists.push(*file);
            }
            Ok(_) => return Err(ForgeCredentialError::UnsafePath((*file).to_path_buf())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                missing.push(*file);
            }
            Err(_) => {
                return Err(ForgeCredentialError::Io {
                    context: "inspect bundle file",
                    path: (*file).to_path_buf(),
                });
            }
        }
    }
    if missing.len() == files.len() {
        return Ok(false);
    }
    if !missing.is_empty() {
        return Err(ForgeCredentialError::PartialBundle);
    }
    for file in &exists {
        #[cfg(unix)]
        check_file_mode(file)?;
        #[cfg(windows)]
        {
            let sid = resolve_current_sid()?;
            verify_windows_dacl(file, &sid)?;
        }
        check_ancestors_all(file, true)?;
    }
    let manifest_bytes = open_and_read(manifest_path)?;
    validate_manifest_bytes(&manifest_bytes, paths)?;
    let cap_bytes: Zeroizing<Vec<u8>> = Zeroizing::new(open_and_read(capability_path)?);
    if cap_bytes.len() != 32 {
        return Err(ForgeCredentialError::InvalidCapability {
            path: capability_path.to_path_buf(),
        });
    }
    let cert_der = open_and_read(cert_path)?;
    let key_bytes: Zeroizing<Vec<u8>> = Zeroizing::new(open_and_read(key_path)?);
    if cert_der.is_empty() || key_bytes.is_empty() {
        return Err(ForgeCredentialError::InvalidCertificate);
    }
    validate_cert_sans(&cert_der)?;
    validate_key_matches_cert(&key_bytes, &cert_der)?;
    let _ = rustls::crypto::ring::default_provider();
    Ok(true)
}

fn generate_material() -> Result<ProvisionalMaterial, ForgeCredentialError> {
    let mut cap = [0_u8; 32];
    getrandom::fill(&mut cap).map_err(|_| ForgeCredentialError::Provisioning)?;
    let key_pair = rcgen::KeyPair::generate().map_err(|_| ForgeCredentialError::Provisioning)?;
    let mut params = rcgen::CertificateParams::new(vec!["localhost".to_string()])
        .map_err(|_| ForgeCredentialError::Provisioning)?;
    params.distinguished_name.push(
        rcgen::DnType::CommonName,
        rcgen::DnValue::Utf8String("localhost".to_string()),
    );
    params.subject_alt_names = vec![
        rcgen::SanType::DnsName(
            "localhost"
                .try_into()
                .map_err(|_| ForgeCredentialError::Provisioning)?,
        ),
        rcgen::SanType::IpAddress(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))),
    ];
    let cert = params
        .self_signed(&key_pair)
        .map_err(|_| ForgeCredentialError::Provisioning)?;
    let cert_der = cert.der().to_vec();
    let key_der = key_pair.serialize_der();
    validate_cert_sans(&cert_der)?;
    validate_key_matches_cert(&key_der, &cert_der)?;
    Ok(ProvisionalMaterial {
        capability: Zeroizing::new(cap),
        private_key: Zeroizing::new(key_der),
        certificate: cert_der,
    })
}

fn install_atomic(
    dir: &Path,
    filename: &str,
    data: &[u8],
    created: &mut Vec<CreatedFile>,
) -> Result<(), ForgeCredentialError> {
    if !is_safe_filename(filename) {
        return Err(ForgeCredentialError::ManifestTraversal);
    }
    let dest = dir.join(filename);
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
    let nonce_hex: String = nonce.iter().map(|b| format!("{b:02x}")).collect();
    let temp_name = format!(".{filename}.{nonce_hex}.tmp");
    let temp_path = dir.join(&temp_name);
    let mut temp_guard = ScopedTemp::new(temp_path.clone());
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
            path: temp_path.clone(),
        })?;
    file.write_all(data).map_err(|_| ForgeCredentialError::Io {
        context: "write temporary file",
        path: temp_path.clone(),
    })?;
    file.sync_all().map_err(|_| ForgeCredentialError::Io {
        context: "sync temporary file",
        path: temp_path.clone(),
    })?;
    let temp_id = file_id_from_file(&file)?;
    drop(file);
    fs::hard_link(&temp_path, &dest).map_err(|_| ForgeCredentialError::Io {
        context: "activate file",
        path: dest.clone(),
    })?;
    let dest_id = file_id(&dest)?;
    if dest_id != temp_id {
        return Err(ForgeCredentialError::Provisioning);
    }
    created.push(CreatedFile {
        path: dest.clone(),
        id: dest_id,
    });
    sync_directory(dir).map_err(|e| {
        let _ = fs::remove_file(&dest);
        e
    })?;
    #[cfg(unix)]
    {
        if let Err(e) = check_file_mode(&dest) {
            let _ = fs::remove_file(&dest);
            return Err(e);
        }
    }
    #[cfg(windows)]
    {
        let sid = resolve_current_sid()?;
        if let Err(e) = restrict_file_windows(&dest, &sid) {
            let _ = fs::remove_file(&dest);
            return Err(e);
        }
    }
    if fs::remove_file(&temp_path).is_err() {
        return Err(ForgeCredentialError::Io {
            context: "remove temporary file",
            path: temp_path.clone(),
        });
    }
    temp_guard.disarm();
    Ok(())
}

fn cleanup_created(created: Vec<CreatedFile>) {
    for entry in created.into_iter().rev() {
        if let Ok(current_id) = file_id(&entry.path) {
            if current_id == entry.id {
                let _ = fs::remove_file(&entry.path);
            }
        }
    }
}

pub fn provision_or_load(home: &Path) -> Result<ForgeCredentialPaths, ForgeCredentialError> {
    validate_home(home)?;
    check_ancestors_all(home, true)?;
    let paths = ForgeCredentialPaths::new(home)?;
    let credentials_dir = paths.credentials_dir();
    ensure_credentials_dir(&credentials_dir)?;
    let lock_path = paths.lock_path();
    let _lock = acquire_lock(&lock_path)?;
    if validate_existing_bundle(&paths)? {
        return Ok(paths);
    }
    let material = generate_material()?;
    let mut created: Vec<CreatedFile> = Vec::new();
    let result = (|| -> Result<(), ForgeCredentialError> {
        install_atomic(
            &credentials_dir,
            "bootstrap-capability.bin",
            material.capability.as_ref(),
            &mut created,
        )?;
        install_atomic(
            &credentials_dir,
            "localhost-leaf.der",
            &material.certificate,
            &mut created,
        )?;
        install_atomic(
            &credentials_dir,
            "localhost-key.pkcs8.der",
            material.private_key.as_ref(),
            &mut created,
        )?;
        let manifest = CredentialManifest {
            schema: "artisan-forge-credentials-v1".to_string(),
            version: 1,
            bootstrap_capability: "bootstrap-capability.bin".to_string(),
            certificate_chain: vec!["localhost-leaf.der".to_string()],
            private_key: "localhost-key.pkcs8.der".to_string(),
        };
        let manifest_bytes = serde_json::to_vec_pretty(&manifest)
            .map_err(|_| ForgeCredentialError::ManifestMalformed)?;
        install_atomic(
            &credentials_dir,
            "manifest.json",
            &manifest_bytes,
            &mut created,
        )?;
        Ok(())
    })();
    if let Err(error) = result {
        cleanup_created(created);
        return Err(error);
    }
    match validate_existing_bundle(&paths) {
        Ok(true) => Ok(paths),
        Ok(false) => {
            cleanup_created(created);
            Err(ForgeCredentialError::Provisioning)
        }
        Err(error) => {
            cleanup_created(created);
            Err(error)
        }
    }
}

pub fn provision_credentials(home: &Path) -> Result<ForgeCredentialPaths, ForgeCredentialError> {
    provision_or_load(home)
}

pub fn ensure_credentials(home: &Path) -> Result<ForgeCredentialPaths, ForgeCredentialError> {
    provision_or_load(home)
}

// Tiny private parser test kept inside production file because DACL string shape cannot be
// exercised through the public credential facade without Windows `icacls` execution.
#[cfg(test)]
mod parser_tests {
    use super::{parse_icacls_output, parse_icacls_strict};

    #[test]
    fn windows_dacl_parser_strict() {
        let sid = "S-1-5-21-1-2-3-1000";
        let good_dir = format!("C:\\creds {sid}:(OI)(CI)(F)");
        assert!(parse_icacls_output(&good_dir, sid).is_ok());
        assert!(parse_icacls_strict(&good_dir, sid, true).is_ok());
        let good_file = format!("C:\\creds\\file {sid}:(F)");
        assert!(parse_icacls_strict(&good_file, sid, false).is_ok());
        let with_inherited = format!("C:\\creds {sid}:(I)(OI)(CI)(F)");
        assert!(parse_icacls_output(&with_inherited, sid).is_err());
        let two_sids = format!("C:\\creds {sid}:(F) S-1-5-21-1-2-3-1001:(F)");
        assert!(parse_icacls_output(&two_sids, sid).is_err());
        let deny = format!("C:\\creds {sid}:(DENY)(F)");
        assert!(parse_icacls_output(&deny, sid).is_err());
        let everyone = format!("C:\\creds BUILTIN\\Users:(F) {sid}:(F)");
        assert!(parse_icacls_output(&everyone, sid).is_err());
        let named_extra = format!("C:\\creds {sid}:(F)\nDOMAIN\\OtherUser:(F)");
        assert!(parse_icacls_output(&named_extra, sid).is_err());
        assert!(parse_icacls_strict(&named_extra, sid, false).is_err());
    }
}
