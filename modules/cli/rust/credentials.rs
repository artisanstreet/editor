#![allow(clippy::too_many_lines)]

use std::{
    fs::{self, File, OpenOptions},
    io::Write,
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
    Manifest(String),
    PartialBundle(String),
    InvalidCapability {
        path: PathBuf,
        found: usize,
    },
    InvalidCertificate(String),
    KeyMismatch,
    Traversal(String),
    WindowsAcl(String),
    Provisioning(String),
}

impl std::fmt::Display for ForgeCredentialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidHome(path) => write!(f, "invalid Artisan home: {}", path.display()),
            Self::UnsafePath(path) => {
                write!(
                    f,
                    "refusing unsafe filesystem operation on {}",
                    path.display()
                )
            }
            Self::Io { context, path } => {
                write!(f, "{context} at {}: [REDACTED]", path.display())
            }
            Self::Manifest(message) => write!(f, "invalid credential manifest: {message}"),
            Self::PartialBundle(message) => write!(f, "partial credential bundle: {message}"),
            Self::InvalidCapability { path, found } => write!(
                f,
                "capability at {} has invalid length {found} (expected 32)",
                path.display()
            ),
            Self::InvalidCertificate(message) => {
                write!(f, "invalid certificate: {message}")
            }
            Self::KeyMismatch => write!(f, "private key does not match certificate"),
            Self::Traversal(name) => write!(f, "traversal filename rejected: {name}"),
            Self::WindowsAcl(message) => write!(f, "Windows ACL error: {message}"),
            Self::Provisioning(message) => write!(f, "provisioning failed: {message}"),
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
            Self::Manifest(message) => f.debug_tuple("Manifest").field(&"[REDACTED]").finish(),
            Self::PartialBundle(message) => {
                f.debug_tuple("PartialBundle").field(&"[REDACTED]").finish()
            }
            Self::InvalidCapability { path, found } => f
                .debug_struct("InvalidCapability")
                .field("path", &path.display().to_string())
                .field("found", found)
                .finish(),
            Self::InvalidCertificate(_) => f
                .debug_tuple("InvalidCertificate")
                .field(&"[REDACTED]")
                .finish(),
            Self::KeyMismatch => f.debug_tuple("KeyMismatch").finish(),
            Self::Traversal(name) => f.debug_tuple("Traversal").field(&"[REDACTED]").finish(),
            Self::WindowsAcl(_) => f.debug_tuple("WindowsAcl").field(&"[REDACTED]").finish(),
            Self::Provisioning(_) => f.debug_tuple("Provisioning").field(&"[REDACTED]").finish(),
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

fn reject_symlink_chain(path: &Path) -> Result<(), ForgeCredentialError> {
    for ancestor in path.ancestors() {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        match fs::symlink_metadata(ancestor) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(ForgeCredentialError::UnsafePath(ancestor.to_path_buf()));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(ForgeCredentialError::Io {
                    context: "inspect path",
                    path: ancestor.to_path_buf(),
                });
            }
        }
        if ancestor == path {
            break;
        }
    }
    Ok(())
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
    if meta.file_type().is_symlink() {
        return Err(ForgeCredentialError::UnsafePath(path.to_path_buf()));
    }
    if !meta.is_dir() {
        return Err(ForgeCredentialError::UnsafePath(path.to_path_buf()));
    }
    let mode = meta.permissions().mode() & 0o777;
    if mode != 0o700 {
        return Err(ForgeCredentialError::WindowsAcl(format!(
            "directory {} has mode {mode:o}, expected 700",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn check_file_mode(path: &Path) -> Result<(), ForgeCredentialError> {
    use std::os::unix::fs::PermissionsExt;
    let meta = fs::symlink_metadata(path)
        .map_err(|_| ForgeCredentialError::UnsafePath(path.to_path_buf()))?;
    if meta.file_type().is_symlink() {
        return Err(ForgeCredentialError::UnsafePath(path.to_path_buf()));
    }
    if !meta.is_file() {
        return Err(ForgeCredentialError::UnsafePath(path.to_path_buf()));
    }
    let mode = meta.permissions().mode() & 0o777;
    if mode != 0o600 {
        return Err(ForgeCredentialError::WindowsAcl(format!(
            "file {} has mode {mode:o}, expected 600",
            path.display()
        )));
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
        .map_err(|_| ForgeCredentialError::Provisioning(format!("missing utility {exe}")))?;
    let start = std::time::Instant::now();
    loop {
        if start.elapsed() > timeout {
            let _ = child.kill();
            return Err(ForgeCredentialError::Provisioning(format!(
                "utility {exe} timed out"
            )));
        }
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => {
                return Err(ForgeCredentialError::Provisioning(format!(
                    "utility {exe} wait failed"
                )));
            }
        }
    }
    let output = child
        .wait_with_output()
        .map_err(|_| ForgeCredentialError::Provisioning(format!("utility {exe} output failed")))?;
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
        return Err(ForgeCredentialError::WindowsAcl("whoami.exe failed".into()));
    }
    let text = String::from_utf8_lossy(&output.stdout).to_string();
    let line = text
        .lines()
        .next()
        .ok_or_else(|| ForgeCredentialError::WindowsAcl("whoami output empty".into()))?;
    let sid = line
        .split("\",\"")
        .nth(1)
        .or_else(|| line.split(',').nth(1))
        .ok_or_else(|| ForgeCredentialError::WindowsAcl("whoami parse failed".into()))?;
    let sid = sid.trim().trim_matches('"').trim().to_string();
    if !is_valid_sid(&sid) {
        return Err(ForgeCredentialError::WindowsAcl(format!(
            "invalid SID shape: {sid}"
        )));
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

#[cfg(windows)]
fn run_icacls(args: &[&str]) -> Result<std::process::Output, ForgeCredentialError> {
    let mut full = Vec::with_capacity(args.len());
    for arg in args {
        full.push(*arg);
    }
    let output = hidden_output("icacls.exe", &full, Duration::from_secs(5))?;
    if !output.status.success() {
        return Err(ForgeCredentialError::WindowsAcl(format!(
            "icacls failed with {}",
            output.status
        )));
    }
    Ok(output)
}

#[cfg(windows)]
fn restrict_directory_windows(dir: &Path) -> Result<(), ForgeCredentialError> {
    let sid = resolve_current_sid()?;
    let dir_str = dir.to_string_lossy().to_string();
    let grant = format!("{sid}:(OI)(CI)F");
    let output = hidden_output(
        "icacls.exe",
        &[&dir_str, "/inheritance:d", "/grant:r", &grant],
        Duration::from_secs(5),
    )?;
    if !output.status.success() {
        return Err(ForgeCredentialError::WindowsAcl(
            "icacls inheritance disable failed".into(),
        ));
    }
    verify_windows_dacl(dir, &sid)?;
    Ok(())
}

#[cfg(windows)]
fn verify_windows_dacl(path: &Path, expected_sid: &str) -> Result<(), ForgeCredentialError> {
    let path_str = path.to_string_lossy().to_string();
    let output = hidden_output("icacls.exe", &[&path_str], Duration::from_secs(5))?;
    if !output.status.success() {
        return Err(ForgeCredentialError::WindowsAcl(
            "icacls verify failed".into(),
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout).to_ascii_lowercase();
    if !text.contains(&expected_sid.to_ascii_lowercase()) {
        return Err(ForgeCredentialError::WindowsAcl(format!(
            "ACL missing SID {}",
            expected_sid
        )));
    }
    let lower = text.to_ascii_lowercase();
    for forbidden in ["everyone", "builtin", "nt authority", "authenticated users"] {
        if lower.contains(forbidden) {
            return Err(ForgeCredentialError::WindowsAcl(format!(
                "ACL contains broader principal {forbidden}"
            )));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn ensure_windows_file_dacl(path: &Path) -> Result<(), ForgeCredentialError> {
    let sid = resolve_current_sid()?;
    verify_windows_dacl(path, &sid)
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
    reject_symlink_chain(dir.parent().unwrap_or(Path::new("/")))?;
    match fs::symlink_metadata(dir) {
        Ok(meta) if meta.file_type().is_symlink() => {
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
    let parent = lock_path.parent().unwrap_or(Path::new("/"));
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
    #[cfg(unix)]
    check_file_mode(lock_path)?;
    #[cfg(windows)]
    {
        let sid = resolve_current_sid()
            .map_err(|e| ForgeCredentialError::WindowsAcl(format!("{e:?}")))?;
        verify_windows_dacl(lock_path, &sid).or_else(|_| {
            let path_str = lock_path.to_string_lossy().to_string();
            let grant = format!("{sid}:(OI)(CI)F");
            let _ = hidden_output(
                "icacls.exe",
                &[&path_str, "/grant:r", &grant],
                Duration::from_secs(5),
            );
            verify_windows_dacl(lock_path, &sid)
        })?;
    }
    Ok(file)
}

fn validate_manifest_bytes(
    bytes: &[u8],
    paths: &ForgeCredentialPaths,
) -> Result<CredentialManifest, ForgeCredentialError> {
    let manifest: CredentialManifest = serde_json::from_slice(bytes)
        .map_err(|e| ForgeCredentialError::Manifest(format!("{e}")))?;
    if manifest.schema != "artisan-forge-credentials-v1" {
        return Err(ForgeCredentialError::Manifest("invalid schema".into()));
    }
    if manifest.version != 1 {
        return Err(ForgeCredentialError::Manifest("invalid version".into()));
    }
    if manifest.bootstrap_capability != "bootstrap-capability.bin" {
        return Err(ForgeCredentialError::Traversal(
            manifest.bootstrap_capability.clone(),
        ));
    }
    if manifest.certificate_chain != vec!["localhost-leaf.der".to_string()] {
        return Err(ForgeCredentialError::Traversal(format!(
            "{:?}",
            manifest.certificate_chain
        )));
    }
    if manifest.private_key != "localhost-key.pkcs8.der" {
        return Err(ForgeCredentialError::Traversal(
            manifest.private_key.clone(),
        ));
    }
    for name in std::iter::once(&manifest.bootstrap_capability)
        .chain(manifest.certificate_chain.iter())
        .chain(std::iter::once(&manifest.private_key))
    {
        if !is_safe_filename(name) {
            return Err(ForgeCredentialError::Traversal(name.clone()));
        }
    }
    Ok(manifest)
}

fn validate_cert_sans(cert_der: &[u8]) -> Result<(), ForgeCredentialError> {
    let (_, cert) = x509_parser::parse_x509_certificate(cert_der)
        .map_err(|_| ForgeCredentialError::InvalidCertificate("malformed DER".into()))?;
    let has_san = cert
        .subject_alternative_name()
        .map_err(|_| ForgeCredentialError::InvalidCertificate("SAN parse failed".into()))?;
    let Some(san) = has_san else {
        return Err(ForgeCredentialError::InvalidCertificate(
            "missing SAN".into(),
        ));
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
        return Err(ForgeCredentialError::InvalidCertificate(
            "SAN must cover DNS localhost and IP 127.0.0.1".into(),
        ));
    }
    if cert.tbs_certificate.issuer != cert.tbs_certificate.subject {
        return Err(ForgeCredentialError::InvalidCertificate(
            "issuer must equal subject for self-signed".into(),
        ));
    }
    Ok(())
}

fn validate_key_matches_cert(key_der: &[u8], cert_der: &[u8]) -> Result<(), ForgeCredentialError> {
    let key_pair = rcgen::KeyPair::try_from(key_der)
        .map_err(|_| ForgeCredentialError::InvalidCertificate("invalid private key DER".into()))?;
    let cert_spki = {
        let (_, cert) = x509_parser::parse_x509_certificate(cert_der)
            .map_err(|_| ForgeCredentialError::InvalidCertificate("cert parse failed".into()))?;
        cert.tbs_certificate.subject_pki.raw.to_vec()
    };
    let key_spki = key_pair.subject_public_key_info();
    if cert_spki != key_spki {
        return Err(ForgeCredentialError::KeyMismatch);
    }
    let _ = rustls_pki_types::PrivateKeyDer::try_from(key_der.to_vec())
        .map_err(|_| ForgeCredentialError::InvalidCertificate("invalid key der".into()))?;
    let _ = rustls_pki_types::CertificateDer::from(cert_der.to_vec());
    Ok(())
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
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(ForgeCredentialError::UnsafePath((*file).to_path_buf()));
            }
            Ok(meta) if meta.is_dir() => {
                return Err(ForgeCredentialError::UnsafePath((*file).to_path_buf()));
            }
            Ok(meta) if meta.is_file() => {
                reject_symlink_chain(file)?;
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
        return Err(ForgeCredentialError::PartialBundle(format!(
            "missing {} of {}",
            missing.len(),
            files.len()
        )));
    }
    for file in &exists {
        #[cfg(unix)]
        check_file_mode(file)?;
        #[cfg(windows)]
        {
            let sid = resolve_current_sid()?;
            verify_windows_dacl(file, &sid)?;
        }
        reject_symlink_chain(file)?;
    }
    let manifest_bytes = fs::read(manifest_path).map_err(|_| ForgeCredentialError::Io {
        context: "read manifest",
        path: manifest_path.to_path_buf(),
    })?;
    validate_manifest_bytes(&manifest_bytes, paths)?;
    let cap_bytes = fs::read(capability_path).map_err(|_| ForgeCredentialError::Io {
        context: "read capability",
        path: capability_path.to_path_buf(),
    })?;
    if cap_bytes.len() != 32 {
        return Err(ForgeCredentialError::InvalidCapability {
            path: capability_path.to_path_buf(),
            found: cap_bytes.len(),
        });
    }
    let cert_der = fs::read(cert_path).map_err(|_| ForgeCredentialError::Io {
        context: "read certificate",
        path: cert_path.to_path_buf(),
    })?;
    let key_der = fs::read(key_path).map_err(|_| ForgeCredentialError::Io {
        context: "read private key",
        path: key_path.to_path_buf(),
    })?;
    if cert_der.is_empty() || key_der.is_empty() {
        return Err(ForgeCredentialError::InvalidCertificate("empty DER".into()));
    }
    validate_cert_sans(&cert_der)?;
    validate_key_matches_cert(&key_der, &cert_der)?;
    let _ = rustls::crypto::ring::default_provider();
    Ok(true)
}

fn generate_material() -> Result<ProvisionalMaterial, ForgeCredentialError> {
    let mut cap = [0_u8; 32];
    getrandom::fill(&mut cap)
        .map_err(|e| ForgeCredentialError::Provisioning(format!("secure random failed: {e}")))?;
    let key_pair = rcgen::KeyPair::generate()
        .map_err(|e| ForgeCredentialError::Provisioning(format!("key generation failed: {e}")))?;
    let mut params = rcgen::CertificateParams::new(vec!["localhost".to_string()])
        .map_err(|e| ForgeCredentialError::Provisioning(format!("cert params failed: {e}")))?;
    params.distinguished_name.push(
        rcgen::DnType::CommonName,
        rcgen::DnValue::Utf8String("localhost".to_string()),
    );
    params.subject_alt_names = vec![
        rcgen::SanType::DnsName(
            "localhost"
                .try_into()
                .map_err(|_| ForgeCredentialError::Provisioning("invalid DNS SAN".into()))?,
        ),
        rcgen::SanType::IpAddress(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))),
    ];
    let cert = params
        .self_signed(&key_pair)
        .map_err(|e| ForgeCredentialError::Provisioning(format!("cert self-sign failed: {e}")))?;
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
    created: &mut Vec<PathBuf>,
) -> Result<(), ForgeCredentialError> {
    if !is_safe_filename(filename) {
        return Err(ForgeCredentialError::Traversal(filename.to_string()));
    }
    let dest = dir.join(filename);
    match fs::symlink_metadata(&dest) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(ForgeCredentialError::UnsafePath(dest));
        }
        Ok(meta) if meta.is_dir() => {
            return Err(ForgeCredentialError::UnsafePath(dest));
        }
        Ok(meta) if meta.is_file() => {
            return Err(ForgeCredentialError::PartialBundle(format!(
                "destination {} already exists",
                dest.display()
            )));
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
    reject_symlink_chain(&dest)?;
    let mut nonce = [0_u8; 16];
    getrandom::fill(&mut nonce)
        .map_err(|e| ForgeCredentialError::Provisioning(format!("random failed: {e}")))?;
    let nonce_hex: String = nonce.iter().map(|b| format!("{b:02x}")).collect();
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
    drop(file);
    fs::rename(&temp_path, &dest).map_err(|_| ForgeCredentialError::Io {
        context: "activate file",
        path: dest.clone(),
    })?;
    sync_directory(dir)?;
    #[cfg(unix)]
    check_file_mode(&dest)?;
    #[cfg(windows)]
    {
        let sid = resolve_current_sid()?;
        verify_windows_dacl(&dest, &sid)?;
    }
    created.push(dest);
    Ok(())
}

pub fn provision_or_load(home: &Path) -> Result<ForgeCredentialPaths, ForgeCredentialError> {
    validate_home(home)?;
    reject_symlink_chain(home)?;
    let paths = ForgeCredentialPaths::new(home)?;
    let credentials_dir = paths.credentials_dir();
    ensure_credentials_dir(&credentials_dir)?;
    let lock_path = paths.lock_path();
    let _lock = acquire_lock(&lock_path)?;
    if validate_existing_bundle(&paths)? {
        return Ok(paths);
    }
    let material = generate_material()?;
    let mut created: Vec<PathBuf> = Vec::new();
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
            .map_err(|e| ForgeCredentialError::Manifest(format!("{e}")))?;
        install_atomic(
            &credentials_dir,
            "manifest.json",
            &manifest_bytes,
            &mut created,
        )?;
        Ok(())
    })();
    if let Err(error) = result {
        for path in created {
            let _ = fs::remove_file(&path);
        }
        return Err(error);
    }
    if !validate_existing_bundle(&paths)? {
        return Err(ForgeCredentialError::Provisioning(
            "bundle validation after install failed".into(),
        ));
    }
    Ok(paths)
}

pub fn provision_credentials(home: &Path) -> Result<ForgeCredentialPaths, ForgeCredentialError> {
    provision_or_load(home)
}

pub fn ensure_credentials(home: &Path) -> Result<ForgeCredentialPaths, ForgeCredentialError> {
    provision_or_load(home)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, thread};

    fn temp_home(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "artisan-cred-test-{}-{}-{}",
            label,
            std::process::id(),
            line!()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn first_provision_produces_exact_file_set() {
        let home = temp_home("first");
        let paths = provision_or_load(&home).unwrap();
        assert!(paths.manifest_path().is_file());
        assert!(paths.capability_path().is_file());
        assert!(paths.certificate_paths()[0].is_file());
        assert!(paths.private_key_path().is_file());
        assert_eq!(fs::read(paths.capability_path()).unwrap().len(), 32);
        let manifest_bytes = fs::read(paths.manifest_path()).unwrap();
        let manifest: CredentialManifest = serde_json::from_slice(&manifest_bytes).unwrap();
        assert_eq!(manifest.schema, "artisan-forge-credentials-v1");
        assert_eq!(manifest.version, 1);
        assert_eq!(manifest.certificate_chain, vec!["localhost-leaf.der"]);
        let cert_der = fs::read(&paths.certificate_paths()[0]).unwrap();
        validate_cert_sans(&cert_der).unwrap();
        let key_der = fs::read(paths.private_key_path()).unwrap();
        validate_key_matches_cert(&key_der, &cert_der).unwrap();
        let size = fs::read_dir(paths.credentials_dir()).unwrap().count();
        assert!(size >= 5);
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn repeated_provision_is_byte_identical() {
        let home = temp_home("repeat");
        let first = provision_or_load(&home).unwrap();
        let cap1 = fs::read(first.capability_path()).unwrap();
        let cert1 = fs::read(&first.certificate_paths()[0]).unwrap();
        let key1 = fs::read(first.private_key_path()).unwrap();
        let manifest1 = fs::read(first.manifest_path()).unwrap();
        let second = provision_or_load(&home).unwrap();
        assert_eq!(cap1, fs::read(second.capability_path()).unwrap());
        assert_eq!(cert1, fs::read(&second.certificate_paths()[0]).unwrap());
        assert_eq!(key1, fs::read(second.private_key_path()).unwrap());
        assert_eq!(manifest1, fs::read(second.manifest_path()).unwrap());
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn concurrent_provision_yields_one_bundle() {
        let home = temp_home("concurrent");
        let handles: Vec<_> = (0..4)
            .map(|_| {
                let home = home.clone();
                thread::spawn(move || provision_or_load(&home).unwrap())
            })
            .collect();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let first_cap = fs::read(results[0].capability_path()).unwrap();
        for paths in &results[1..] {
            assert_eq!(first_cap, fs::read(paths.capability_path()).unwrap());
        }
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn partial_bundle_is_error_not_rotated() {
        let home = temp_home("partial");
        provision_or_load(&home).unwrap();
        let cap_path = home.join("credentials").join("bootstrap-capability.bin");
        fs::remove_file(home.join("credentials").join("localhost-leaf.der")).unwrap();
        let original_cap = fs::read(&cap_path).unwrap();
        let err = provision_or_load(&home).unwrap_err();
        assert!(format!("{err}").contains("partial"));
        assert_eq!(fs::read(&cap_path).unwrap(), original_cap);
        assert!(home.join("credentials").join("manifest.json").is_file());
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn corrupt_and_unknown_fields_fail() {
        let home = temp_home("corrupt");
        provision_or_load(&home).unwrap();
        let manifest_path = home.join("credentials").join("manifest.json");
        let original = fs::read(&manifest_path).unwrap();
        fs::write(
            &manifest_path,
            br#"{"schema":"artisan-forge-credentials-v1","version":1,"bootstrap_capability":"bootstrap-capability.bin","certificate_chain":["localhost-leaf.der"],"private_key":"localhost-key.pkcs8.der","unknown":"field"}"#,
        )
        .unwrap();
        assert!(provision_or_load(&home).is_err());
        fs::write(&manifest_path, original).unwrap();
        let cert_path = home.join("credentials").join("localhost-leaf.der");
        let orig_cert = fs::read(&cert_path).unwrap();
        fs::write(&cert_path, b"not der").unwrap();
        assert!(provision_or_load(&home).is_err());
        fs::write(&cert_path, orig_cert).unwrap();
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn traversal_filename_rejected() {
        let home = temp_home("traversal");
        provision_or_load(&home).unwrap();
        let manifest_path = home.join("credentials").join("manifest.json");
        fs::write(
            &manifest_path,
            br#"{"schema":"artisan-forge-credentials-v1","version":1,"bootstrap_capability":"../evil.bin","certificate_chain":["localhost-leaf.der"],"private_key":"localhost-key.pkcs8.der"}"#,
        )
        .unwrap();
        assert!(provision_or_load(&home).is_err());
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn symlink_rejected() {
        let home = temp_home("symlink");
        provision_or_load(&home).unwrap();
        let cert_path = home.join("credentials").join("localhost-leaf.der");
        let backup = cert_path.with_extension("bak");
        fs::rename(&cert_path, &backup).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&backup, &cert_path).unwrap();
        }
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_file(&backup, &cert_path).unwrap_or_else(|_| {
                fs::remove_file(&backup).unwrap();
                fs::write(&cert_path, b"not der").unwrap();
                return;
            });
        }
        let err = provision_or_load(&home).unwrap_err();
        assert!(
            format!("{err}").to_ascii_lowercase().contains("unsafe")
                || format!("{err:?}").contains("Unsafe")
        );
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn legacy_secrets_untouched() {
        let home = temp_home("legacy");
        let sentinel = b"legacy-sentinel-bytes-keep";
        fs::write(home.join("secrets.json"), sentinel).unwrap();
        provision_or_load(&home).unwrap();
        assert_eq!(fs::read(home.join("secrets.json")).unwrap(), sentinel);
        let second = provision_or_load(&home).unwrap();
        assert_eq!(fs::read(home.join("secrets.json")).unwrap(), sentinel);
        let _ = second;
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn redacted_debug_and_sentinel_scan() {
        let home = temp_home("redacted");
        let paths = provision_or_load(&home).unwrap();
        let cap = fs::read(paths.capability_path()).unwrap();
        let debug = format!("{:?}", paths);
        let err = ForgeCredentialError::Provisioning("secret".into());
        let err_debug = format!("{:?}", err);
        let err_display = format!("{err}");
        for bytes in [cap.clone()] {
            let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
            assert!(!debug.contains(&hex));
            assert!(!err_debug.contains(&hex));
            assert!(!err_display.contains(&hex));
        }
        let material = ProvisionalMaterial {
            capability: Zeroizing::new([0xAA; 32]),
            private_key: Zeroizing::new(vec![0xBB; 64]),
            certificate: vec![0xCC; 100],
        };
        let mat_debug = format!("{material:?}");
        assert!(mat_debug.contains("[REDACTED]"));
        assert!(!mat_debug.contains("AA"));
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn unix_exact_modes() {
        use std::os::unix::fs::PermissionsExt;
        let home = temp_home("modes");
        let paths = provision_or_load(&home).unwrap();
        let dir_mode = fs::metadata(paths.credentials_dir())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(dir_mode, 0o700);
        for file in [
            paths.manifest_path(),
            paths.capability_path(),
            &paths.certificate_paths()[0],
            paths.private_key_path(),
            &paths.lock_path(),
        ] {
            let mode = fs::metadata(file).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "file {} mode {mode:o}", file.display());
        }
        fs::set_permissions(paths.credentials_dir(), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(provision_or_load(&home).is_err());
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    #[cfg(windows)]
    fn windows_owner_only_dacl() {
        let home = temp_home("winacl");
        let paths = provision_or_load(&home).unwrap();
        let sid = resolve_current_sid().unwrap();
        assert!(is_valid_sid(&sid));
        verify_windows_dacl(paths.credentials_dir(), &sid).unwrap();
        verify_windows_dacl(paths.capability_path(), &sid).unwrap();
        let acl_output = hidden_output(
            "icacls.exe",
            &[&paths.credentials_dir().to_string_lossy().to_string()],
            Duration::from_secs(5),
        )
        .unwrap();
        assert!(acl_output.status.success());
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn atomic_failure_cleanup_only_this_run() {
        let home = temp_home("atomic");
        let cred_dir = home.join("credentials");
        fs::create_dir_all(&cred_dir).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&cred_dir, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let preexisting = cred_dir.join("keep.txt");
        fs::write(&preexisting, b"keep").unwrap();
        let mut created = Vec::new();
        let _ = install_atomic(&cred_dir, "a.bin", b"data", &mut created);
        assert!(cred_dir.join("a.bin").is_file());
        let second_temp = cred_dir.join("b.bin");
        fs::write(&second_temp, b"existing").unwrap();
        let mut created2: Vec<Vec<PathBuf>> = Vec::new();
        let mut c = Vec::new();
        let err = install_atomic(&cred_dir, "b.bin", b"new", &mut c).unwrap_err();
        assert!(
            format!("{err}").contains("already exists") || format!("{err:?}").contains("Partial")
        );
        assert!(preexisting.is_file());
        assert_eq!(fs::read(preexisting).unwrap(), b"keep");
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn nonregular_file_rejected() {
        let home = temp_home("nonregular");
        provision_or_load(&home).unwrap();
        let manifest_path = home.join("credentials").join("manifest.json");
        let backup = manifest_path.with_extension("bak");
        fs::rename(&manifest_path, &backup).unwrap();
        fs::create_dir(&manifest_path).unwrap();
        assert!(provision_or_load(&home).is_err());
        fs::remove_dir_all(&manifest_path).unwrap();
        fs::rename(&backup, &manifest_path).unwrap();
        fs::remove_dir_all(home).unwrap();
    }
}
