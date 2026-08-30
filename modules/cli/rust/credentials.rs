use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr},
    path::{Path, PathBuf},
    time::Duration,
};

use fs2::FileExt;
use rcgen::PublicKeyData;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

#[derive(Clone, Eq, PartialEq)]
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
    credentials_dir: PathBuf,
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
            credentials_dir: credentials_dir.clone(),
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
        self.credentials_dir.clone()
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

fn encode_nonce_hex(nonce: &[u8; 16]) -> String {
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
    index: u64,
}

#[cfg(unix)]
fn file_id(path: &Path) -> Result<FileId, ForgeCredentialError> {
    let mut file = File::open(path).map_err(|_| ForgeCredentialError::Io {
        context: "inspect file id",
        path: path.to_path_buf(),
    })?;
    file_id_from_file(&file)
}

#[cfg(windows)]
fn file_id(path: &Path) -> Result<FileId, ForgeCredentialError> {
    let file = File::open(path).map_err(|_| ForgeCredentialError::Io {
        context: "inspect file id",
        path: path.to_path_buf(),
    })?;
    file_id_from_file(&file)
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

struct CreatedFile {
    path: PathBuf,
    id: FileId,
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

#[cfg(all(test, windows))]
mod acl_diagnostic {
    use super::*;

    pub(super) const MAX_STREAM_BYTES: usize = 4096;
    pub(super) const MAX_EVENTS: usize = 16;
    const MAX_ARTIFACT_BYTES: usize = 16 * 1024;
    pub(super) const CANARIES: [&str; 7] = [
        "S-1-5-21-1-2-3-1000",
        "DOMAIN\\account-canary",
        "C:\\sensitive\\path",
        "whoami/icacls-output-canary",
        "credential-bytes-canary",
        "private-key-bytes-canary",
        "bootstrap-capability-bytes-canary",
    ];
    #[derive(Debug, Serialize)]
    struct AclDiagnosticEvent {
        stage: &'static str,
        kind: &'static str,
        value: &'static str,
    }

    #[derive(Debug, Serialize)]
    pub(super) struct AclDiagnosticRecord {
        schema_version: u8,
        outcome: &'static str,
        stage: &'static str,
        event_count: u8,
        overflow: bool,
        events: Vec<AclDiagnosticEvent>,
    }

    impl AclDiagnosticRecord {
        fn new() -> Self {
            Self {
                schema_version: 1,
                outcome: "Other",
                stage: "Unreached",
                event_count: 0,
                overflow: false,
                events: Vec::with_capacity(MAX_EVENTS),
            }
        }

        fn push(&mut self, stage: &'static str, kind: &'static str, value: &'static str) {
            if self.events.len() < MAX_EVENTS {
                self.events.push(AclDiagnosticEvent { stage, kind, value });
            } else {
                self.overflow = true;
            }
        }

        pub(super) fn finish(mut self, outcome: &'static str) -> Self {
            self.outcome = outcome;
            if outcome == "Success" {
                self.stage = "Completed";
            }
            self.event_count =
                u8::try_from(self.events.len()).expect("diagnostic event count fits in u8");
            self
        }
    }

    pub(super) enum PlannerClassification {
        InvalidValidatedIdentity,
        MissingAceSeparator,
        InheritedAce,
        CurrentIdentityMatch,
        DuplicateCurrentIdentity,
        SafeRemovableExtra,
        UnsafeNonmatchingExtra,
        DuplicateExtra,
        PlanComplete,
    }

    impl PlannerClassification {
        const fn as_str(self) -> &'static str {
            match self {
                Self::InvalidValidatedIdentity => "InvalidValidatedIdentity",
                Self::MissingAceSeparator => "MissingAceSeparator",
                Self::InheritedAce => "InheritedAce",
                Self::CurrentIdentityMatch => "CurrentIdentityMatch",
                Self::DuplicateCurrentIdentity => "DuplicateCurrentIdentity",
                Self::SafeRemovableExtra => "SafeRemovableExtra",
                Self::UnsafeNonmatchingExtra => "UnsafeNonmatchingExtra",
                Self::DuplicateExtra => "DuplicateExtra",
                Self::PlanComplete => "PlanComplete",
            }
        }
    }

    pub(super) enum ParserClassification {
        MalformedSuccessSummary,
        MissingSeparator,
        MissingOpeningToken,
        NonTokenContent,
        UnterminatedToken,
        EmptyToken,
        NoTokens,
        AcceptedAce,
        ParserComplete,
    }

    impl ParserClassification {
        const fn as_str(self) -> &'static str {
            match self {
                Self::MalformedSuccessSummary => "MalformedSuccessSummary",
                Self::MissingSeparator => "MissingSeparator",
                Self::MissingOpeningToken => "MissingOpeningToken",
                Self::NonTokenContent => "NonTokenContent",
                Self::UnterminatedToken => "UnterminatedToken",
                Self::EmptyToken => "EmptyToken",
                Self::NoTokens => "NoTokens",
                Self::AcceptedAce => "AcceptedAce",
                Self::ParserComplete => "ParserComplete",
            }
        }
    }

    thread_local! {
        static ACTIVE: std::cell::RefCell<Option<AclDiagnosticRecord>> = const {
            std::cell::RefCell::new(None)
        };
    }

    pub(super) fn capture<T>(operation: impl FnOnce() -> T) -> (T, AclDiagnosticRecord) {
        ACTIVE.with(|active| *active.borrow_mut() = Some(AclDiagnosticRecord::new()));
        let result = operation();
        let record = ACTIVE
            .with(|active| active.borrow_mut().take())
            .unwrap_or_else(AclDiagnosticRecord::new);
        (result, record)
    }

    pub(super) fn stage(stage: &'static str) {
        ACTIVE.with(|active| {
            if let Some(record) = active.borrow_mut().as_mut() {
                record.stage = stage;
            }
        });
    }

    fn current_stage() -> &'static str {
        ACTIVE.with(|active| active.borrow().as_ref().map_or("Unreached", |r| r.stage))
    }

    fn event_at(stage: &'static str, kind: &'static str, value: &'static str) {
        ACTIVE.with(|active| {
            if let Some(record) = active.borrow_mut().as_mut() {
                record.push(stage, kind, value);
            }
        });
    }

    pub(super) fn event(kind: &'static str, value: &'static str) {
        event_at(current_stage(), kind, value);
    }

    pub(super) fn planner(classification: PlannerClassification) {
        event("planner", classification.as_str());
    }

    pub(super) fn parser(classification: ParserClassification) {
        event("parser", classification.as_str());
    }

    fn bounded(text: &str) -> &str {
        let mut end = text.len().min(MAX_STREAM_BYTES);
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        &text[..end]
    }

    pub(super) fn stream_shape(bytes: &[u8]) -> &'static str {
        if bytes.len() > MAX_STREAM_BYTES {
            "Oversized"
        } else if bytes.is_empty() {
            "Empty"
        } else if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
            "Bom"
        } else if std::str::from_utf8(bytes).is_ok() {
            "Utf8"
        } else {
            "InvalidUtf8"
        }
    }

    fn identity_shape(fields: usize, sid: Option<&str>, account: Option<&str>) -> &'static str {
        if fields != 2 {
            return "FieldCount";
        }
        let (Some(sid), Some(account)) = (sid, account) else {
            return "Missing";
        };
        if sid.len() > MAX_STREAM_BYTES || account.len() > MAX_STREAM_BYTES {
            return "Oversized";
        }
        if sid.is_empty() || account.is_empty() {
            return "Empty";
        }
        let valid_account = account.matches('\\').count() == 1
            && !account.starts_with('\\')
            && !account.ends_with('\\')
            && !account
                .chars()
                .any(|c| c.is_control() || matches!(c, '/' | ':' | '"' | ','));
        if is_valid_sid(sid) && valid_account {
            "Valid"
        } else {
            "Malformed"
        }
    }

    pub(super) fn record_identity(
        stdout: &[u8],
        stderr: &[u8],
        fields: usize,
        sid: Option<&str>,
        account: Option<&str>,
    ) {
        for (kind, bytes) in [("stdout", stdout), ("stderr", stderr)] {
            event_at("IdentityResolution", kind, stream_shape(bytes));
        }
        event_at(
            "IdentityResolution",
            "identity",
            identity_shape(fields, sid, account),
        );
    }

    fn summary(output: &str) -> &'static str {
        let output = bounded(output).to_ascii_lowercase();
        if let Some(line) = output
            .lines()
            .find(|line| line.trim_start().starts_with("successfully processed"))
        {
            if line.contains("failed processing") && line.bytes().any(|b| b.is_ascii_digit()) {
                "Recognized"
            } else {
                "Malformed"
            }
        } else if output.contains("successfully")
            || output.contains("processed")
            || output.contains("processing")
            || output.contains("failed")
            || output.contains("files")
        {
            "LocalizedOrOther"
        } else {
            "Missing"
        }
    }

    pub(super) fn record_acl(output: &str, count: usize) {
        event(
            "acl_count",
            match count {
                0 => "Zero",
                1 => "One",
                2 => "Two",
                _ => "Many",
            },
        );
        event("acl_summary", summary(output));
    }

    pub(super) fn probe(exe: &str, args: &[&str]) -> Option<&'static str> {
        if exe.eq_ignore_ascii_case("whoami.exe") {
            Some("Whoami")
        } else if exe.eq_ignore_ascii_case("icacls.exe") {
            Some(if args.contains(&"/grant:r") || args.contains(&"/remove") {
                "IcaclsMutation"
            } else {
                "IcaclsQuery"
            })
        } else {
            None
        }
    }

    pub(super) fn subprocess(probe: Option<&'static str>, outcome: &'static str) {
        let Some(probe) = probe else {
            return;
        };
        event_at(
            if probe == "Whoami" {
                "IdentityResolution"
            } else {
                current_stage()
            },
            probe,
            outcome,
        );
    }

    pub(super) fn ace_reason(
        ace: &str,
        identity: &CurrentIdentity,
        expect_dir: bool,
    ) -> &'static str {
        let ace = bounded(ace);
        let lower = ace.to_ascii_lowercase();
        if lower.contains("deny") {
            return "Deny";
        }
        if lower.contains("(i)") {
            return "Inherited";
        }
        let Some(colon) = ace.find(':') else {
            return "MalformedAce";
        };
        let principal = ace[..colon].trim();
        let sid = bounded(&identity.sid);
        let account = bounded(&identity.account);
        let matches = (identity.sid.len() <= MAX_STREAM_BYTES
            && !sid.is_empty()
            && principal.eq_ignore_ascii_case(sid))
            || (identity.account.len() <= MAX_STREAM_BYTES
                && !account.is_empty()
                && principal.eq_ignore_ascii_case(account));
        if principal.is_empty() || !matches {
            return "IdentityMismatch";
        }
        let flags = &lower[colon + 1..];
        if !flags.contains("(f)") {
            return "MissingFullControl";
        }
        if flags.matches("(f)").count() != 1
            || flags.matches("(oi)").count() > 1
            || flags.matches("(ci)").count() > 1
            || flags
                .chars()
                .any(|c| c.is_ascii_alphabetic() && !matches!(c, 'f' | 'o' | 'i' | 'c'))
        {
            return "InvalidFlags";
        }
        if (expect_dir && (!flags.contains("(oi)") || !flags.contains("(ci)")))
            || (!expect_dir && (flags.contains("(oi)") || flags.contains("(ci)")))
        {
            return "InheritanceMismatch";
        }
        if ["everyone", "builtin", "nt authority", "authenticated users"]
            .iter()
            .any(|forbidden| lower.contains(forbidden))
        {
            "BroadPrincipal"
        } else {
            "Accepted"
        }
    }

    pub(super) fn write(path: &Path, record: &AclDiagnosticRecord) -> Option<()> {
        if !path.is_absolute() {
            return None;
        }
        let bytes = serde_json::to_vec(record).ok()?;
        if bytes.len() > MAX_ARTIFACT_BYTES {
            return None;
        }
        let parent = path.parent()?;
        let file_name = path.file_name()?;
        let mut nonce = [0_u8; 16];
        getrandom::fill(&mut nonce).ok()?;
        let temporary_path = parent.join(format!(
            ".{}.{}.tmp",
            file_name.to_string_lossy(),
            encode_nonce_hex(&nonce)
        ));
        let result = (|| -> Option<()> {
            let mut file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temporary_path)
                .ok()?;
            file.write_all(&bytes).ok()?;
            file.sync_all().ok()?;
            drop(file);
            fs::rename(&temporary_path, path).ok()?;
            Some(())
        })();
        if result.is_none() {
            let _ = fs::remove_file(&temporary_path);
        }
        result
    }

    pub(super) fn assert_redacted(record: &AclDiagnosticRecord) {
        let debug = format!("{record:?}");
        let bytes = serde_json::to_vec(record).expect("structural diagnostic serialization");
        assert!(bytes.len() <= MAX_ARTIFACT_BYTES);
        let text = String::from_utf8_lossy(&bytes);
        for canary in CANARIES {
            assert!(!debug.contains(canary));
            assert!(!text.contains(canary));
        }
    }
}

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

#[cfg(windows)]
fn hidden_output(
    exe: &str,
    args: &[&str],
    timeout: Duration,
) -> Result<std::process::Output, ForgeCredentialError> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    #[cfg(all(test, windows))]
    let probe = acl_diagnostic::probe(exe, args);
    let mut command = std::process::Command::new(exe);
    for arg in args {
        command.arg(arg);
    }
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW);
    let mut child = command.spawn().map_err(|_| {
        acl_diagnostic!(acl_diagnostic::subprocess(probe, "SpawnFailed"));
        ForgeCredentialError::Provisioning
    })?;
    let start = std::time::Instant::now();
    loop {
        if start.elapsed() > timeout {
            let _ = child.kill();
            let reap_start = std::time::Instant::now();
            let reap_deadline = Duration::from_secs(2);
            loop {
                match child.try_wait() {
                    Ok(Some(_)) | Err(_) => break,
                    Ok(None) => {
                        if reap_start.elapsed() > reap_deadline {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                }
            }
            let _ = child.try_wait();
            acl_diagnostic!(acl_diagnostic::subprocess(probe, "TimedOut"));
            return Err(ForgeCredentialError::Provisioning);
        }
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => std::thread::sleep(Duration::from_millis(10)),
            Err(_) => {
                acl_diagnostic!(acl_diagnostic::subprocess(probe, "WaitFailed"));
                return Err(ForgeCredentialError::Provisioning);
            }
        }
    }
    let output = child.wait_with_output().map_err(|_| {
        acl_diagnostic!(acl_diagnostic::subprocess(probe, "WaitFailed"));
        ForgeCredentialError::Provisioning
    })?;
    acl_diagnostic!(acl_diagnostic::subprocess(
        probe,
        if output.status.success() {
            "ExitedSuccess"
        } else {
            "ExitedNonZero"
        },
    ));
    Ok(output)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CurrentIdentity {
    sid: String,
    account: String,
}

#[cfg(windows)]
fn resolve_current_identity() -> Result<CurrentIdentity, ForgeCredentialError> {
    let output = hidden_output(
        "whoami.exe",
        &["/user", "/fo", "csv", "/nh"],
        Duration::from_secs(5),
    )?;
    if !output.status.success() {
        acl_diagnostic!(acl_diagnostic::record_identity(
            &output.stdout,
            &output.stderr,
            0,
            None,
            None
        ));
        return Err(ForgeCredentialError::WindowsAcl);
    }
    let text = String::from_utf8_lossy(&output.stdout).to_string();
    let Some(line) = text.lines().next() else {
        acl_diagnostic!(acl_diagnostic::record_identity(
            &output.stdout,
            &output.stderr,
            0,
            None,
            None
        ));
        return Err(ForgeCredentialError::WindowsAcl);
    };
    let mut parts: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for ch in line.chars() {
        if ch == '"' {
            in_quotes = !in_quotes;
        } else if ch == ',' && !in_quotes {
            parts.push(current.trim().to_string());
            current.clear();
        } else {
            current.push(ch);
        }
    }
    parts.push(current.trim().to_string());
    if parts.len() != 2 {
        acl_diagnostic!(acl_diagnostic::record_identity(
            &output.stdout,
            &output.stderr,
            parts.len(),
            None,
            None
        ));
        return Err(ForgeCredentialError::WindowsAcl);
    }
    let account = parts[0].trim().trim_matches('"').trim().to_string();
    let sid = parts[1].trim().trim_matches('"').trim().to_string();
    acl_diagnostic!(acl_diagnostic::record_identity(
        &output.stdout,
        &output.stderr,
        parts.len(),
        Some(&sid),
        Some(&account),
    ));
    if !is_valid_sid(&sid) {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    if !is_valid_account(&account) {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    Ok(CurrentIdentity { sid, account })
}

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

fn is_valid_account(account: &str) -> bool {
    if account.is_empty()
        || account.contains('\0')
        || account.contains('/')
        || account.contains(':')
        || account.contains('"')
        || account.contains(',')
        || account.chars().any(char::is_control)
    {
        return false;
    }
    if account.matches('\\').count() != 1 {
        return false;
    }
    let mut split = account.split('\\');
    let domain = split.next().unwrap_or("");
    let user = split.next().unwrap_or("");
    !domain.is_empty() && !user.is_empty() && split.next().is_none()
}

#[cfg(test)]
fn parse_icacls_output_with_path(
    output: &str,
    expected_sid: &str,
    queried_path: &str,
) -> Result<(), ForgeCredentialError> {
    let identity = CurrentIdentity {
        sid: expected_sid.to_string(),
        account: String::new(),
    };
    parse_icacls_strict_with_identity(output, &identity, true, queried_path)
}

#[cfg(test)]
fn parse_icacls_strict_with_path(
    output: &str,
    expected_sid: &str,
    expect_dir: bool,
    queried_path: &str,
) -> Result<(), ForgeCredentialError> {
    let identity = CurrentIdentity {
        sid: expected_sid.to_string(),
        account: String::new(),
    };
    parse_icacls_strict_with_identity(output, &identity, expect_dir, queried_path)
}

fn parse_icacls_strict_with_identity(
    output: &str,
    identity: &CurrentIdentity,
    expect_dir: bool,
    queried_path: &str,
) -> Result<(), ForgeCredentialError> {
    let ace_lines = collect_icacls_ace_lines(output, queried_path)?;
    match ace_lines.as_slice() {
        [ace] => validate_icacls_ace(ace, identity, expect_dir),
        _ => Err(ForgeCredentialError::WindowsAcl),
    }
}

fn collect_icacls_ace_lines(
    output: &str,
    queried_path: &str,
) -> Result<Vec<String>, ForgeCredentialError> {
    let mut ace_lines: Vec<String> = Vec::new();
    let queried_lower = queried_path.to_ascii_lowercase();
    let mut first_line = true;
    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("successfully") {
            if is_icacls_success_summary(&lower) {
                continue;
            }
            acl_diagnostic!(acl_diagnostic::parser(
                acl_diagnostic::ParserClassification::MalformedSuccessSummary
            ));
            acl_diagnostic!(acl_diagnostic::record_acl(output, ace_lines.len()));
            return Err(ForgeCredentialError::WindowsAcl);
        }
        // Exact queried path prefix handling: strip only the complete queried path
        // (case-insensitive) from the first ACE line if present.
        let mut candidate = trimmed.to_string();
        if !queried_path.is_empty() && lower.starts_with(&queried_lower) {
            let remainder = trimmed[queried_path.len()..].trim();
            if remainder.is_empty() {
                // Header line containing only the path, no ACE
                first_line = false;
                continue;
            }
            candidate = remainder.to_string();
        } else if first_line && trimmed.contains(":\\") && !trimmed.contains('(') {
            // Header without queried_path provided (parser test without path)
            first_line = false;
            continue;
        }
        first_line = false;
        if !candidate.contains(':') {
            acl_diagnostic!(acl_diagnostic::parser(
                acl_diagnostic::ParserClassification::MissingSeparator
            ));
            // Any non-summary, non-ACE line is a failure.
            acl_diagnostic!(acl_diagnostic::record_acl(output, ace_lines.len()));
            return Err(ForgeCredentialError::WindowsAcl);
        }
        if !candidate.contains('(') {
            if candidate
                .split_once(':')
                .is_some_and(|(_, flags)| flags.trim().is_empty())
            {
                acl_diagnostic!(acl_diagnostic::parser(
                    acl_diagnostic::ParserClassification::NoTokens
                ));
            } else {
                acl_diagnostic!(acl_diagnostic::parser(
                    acl_diagnostic::ParserClassification::MissingOpeningToken
                ));
            }
            // Any non-summary, non-ACE line is a failure.
            acl_diagnostic!(acl_diagnostic::record_acl(output, ace_lines.len()));
            return Err(ForgeCredentialError::WindowsAcl);
        }
        // Ensure no trailing junk after the parenthesized flags
        let Some(colon) = candidate.find(':') else {
            acl_diagnostic!(acl_diagnostic::parser(
                acl_diagnostic::ParserClassification::MissingSeparator
            ));
            acl_diagnostic!(acl_diagnostic::record_acl(output, ace_lines.len()));
            return Err(ForgeCredentialError::WindowsAcl);
        };
        let flags_part = &candidate[colon + 1..];
        // Flags must be exactly a sequence of (...) tokens with optional whitespace, nothing else
        let mut idx = 0;
        let chars: Vec<char> = flags_part.chars().collect();
        let mut has_content = false;
        while idx < chars.len() {
            while idx < chars.len() && chars[idx].is_whitespace() {
                idx += 1;
            }
            if idx >= chars.len() {
                break;
            }
            if chars[idx] != '(' {
                acl_diagnostic!(acl_diagnostic::parser(
                    acl_diagnostic::ParserClassification::NonTokenContent
                ));
                acl_diagnostic!(acl_diagnostic::record_acl(output, ace_lines.len()));
                return Err(ForgeCredentialError::WindowsAcl);
            }
            let mut tok = String::new();
            idx += 1;
            while idx < chars.len() && chars[idx] != ')' {
                tok.push(chars[idx]);
                idx += 1;
            }
            if idx >= chars.len() || chars[idx] != ')' {
                acl_diagnostic!(acl_diagnostic::parser(
                    acl_diagnostic::ParserClassification::UnterminatedToken
                ));
                acl_diagnostic!(acl_diagnostic::record_acl(output, ace_lines.len()));
                return Err(ForgeCredentialError::WindowsAcl);
            }
            idx += 1;
            has_content = true;
            if tok.is_empty() {
                acl_diagnostic!(acl_diagnostic::parser(
                    acl_diagnostic::ParserClassification::EmptyToken
                ));
                acl_diagnostic!(acl_diagnostic::record_acl(output, ace_lines.len()));
                return Err(ForgeCredentialError::WindowsAcl);
            }
        }
        if !has_content {
            acl_diagnostic!(acl_diagnostic::parser(
                acl_diagnostic::ParserClassification::NoTokens
            ));
            acl_diagnostic!(acl_diagnostic::record_acl(output, ace_lines.len()));
            return Err(ForgeCredentialError::WindowsAcl);
        }
        ace_lines.push(candidate);
        acl_diagnostic!(acl_diagnostic::parser(
            acl_diagnostic::ParserClassification::AcceptedAce
        ));
    }
    acl_diagnostic!(acl_diagnostic::record_acl(output, ace_lines.len()));
    acl_diagnostic!(acl_diagnostic::parser(
        acl_diagnostic::ParserClassification::ParserComplete
    ));
    Ok(ace_lines)
}

fn is_icacls_success_summary(line: &str) -> bool {
    let Some(rest) = line.strip_prefix("successfully processed ") else {
        return false;
    };
    let Some((processed, failed)) = rest.split_once("; failed processing ") else {
        return false;
    };
    let Some(processed) = processed.strip_suffix(" files") else {
        return false;
    };
    let failed = failed.strip_suffix('.').unwrap_or(failed);
    let Some(failed) = failed.strip_suffix(" files") else {
        return false;
    };
    let (Ok(processed), Ok(failed)) = (processed.parse::<u64>(), failed.parse::<u64>()) else {
        return false;
    };
    processed == 1 && failed == 0
}

fn plan_icacls_removals(
    output: &str,
    identity: &CurrentIdentity,
    queried_path: &str,
) -> Result<Vec<String>, ForgeCredentialError> {
    if !is_valid_sid(&identity.sid)
        || !is_valid_account(&identity.account)
        || identity.sid.eq_ignore_ascii_case(&identity.account)
    {
        acl_diagnostic!(acl_diagnostic::planner(
            acl_diagnostic::PlannerClassification::InvalidValidatedIdentity
        ));
        return Err(ForgeCredentialError::WindowsAcl);
    }
    let ace_lines = collect_icacls_ace_lines(output, queried_path)?;
    let mut removals = Vec::new();
    let mut current_identity_count = 0;
    for ace in ace_lines {
        let Some(colon) = ace.find(':') else {
            acl_diagnostic!(acl_diagnostic::planner(
                acl_diagnostic::PlannerClassification::MissingAceSeparator
            ));
            return Err(ForgeCredentialError::WindowsAcl);
        };
        let principal = ace[..colon].trim();
        let flags = ace[colon + 1..].to_ascii_lowercase();
        if flags.contains("(i)") {
            acl_diagnostic!(acl_diagnostic::planner(
                acl_diagnostic::PlannerClassification::InheritedAce
            ));
            return Err(ForgeCredentialError::WindowsAcl);
        }
        let is_deny = flags.contains("(deny)");
        let is_current_identity = principal.eq_ignore_ascii_case(&identity.sid)
            || principal.eq_ignore_ascii_case(&identity.account);
        if is_current_identity {
            current_identity_count += 1;
            if current_identity_count > 1 {
                acl_diagnostic!(acl_diagnostic::planner(
                    acl_diagnostic::PlannerClassification::DuplicateCurrentIdentity
                ));
                return Err(ForgeCredentialError::WindowsAcl);
            }
            acl_diagnostic!(acl_diagnostic::planner(
                acl_diagnostic::PlannerClassification::CurrentIdentityMatch
            ));
            if is_deny {
                removals.push(principal.to_owned());
            }
            continue;
        }
        if !is_safe_acl_principal(principal) {
            acl_diagnostic!(acl_diagnostic::planner(
                acl_diagnostic::PlannerClassification::UnsafeNonmatchingExtra
            ));
            return Err(ForgeCredentialError::WindowsAcl);
        }
        if removals
            .iter()
            .any(|candidate: &String| candidate.eq_ignore_ascii_case(principal))
        {
            acl_diagnostic!(acl_diagnostic::planner(
                acl_diagnostic::PlannerClassification::DuplicateExtra
            ));
            return Err(ForgeCredentialError::WindowsAcl);
        }
        acl_diagnostic!(acl_diagnostic::planner(
            acl_diagnostic::PlannerClassification::SafeRemovableExtra
        ));
        removals.push(principal.to_owned());
    }
    acl_diagnostic!(acl_diagnostic::planner(
        acl_diagnostic::PlannerClassification::PlanComplete
    ));
    Ok(removals)
}

fn is_safe_acl_principal(principal: &str) -> bool {
    if principal
        .chars()
        .any(|character| matches!(character, '*' | '(' | ')'))
    {
        return false;
    }
    if is_valid_sid(principal) || is_valid_account(principal) {
        return true;
    }
    matches!(
        principal.to_ascii_lowercase().as_str(),
        "everyone"
            | "creator owner"
            | "owner rights"
            | "all application packages"
            | "all restricted application packages"
            | "authenticated users"
            | "anonymous logon"
            | "interactive"
            | "local service"
            | "network service"
            | "administrators"
            | "users"
            | "system"
    )
}

fn validate_icacls_ace(
    ace: &str,
    identity: &CurrentIdentity,
    expect_dir: bool,
) -> Result<(), ForgeCredentialError> {
    acl_diagnostic!(acl_diagnostic::event(
        "ace",
        acl_diagnostic::ace_reason(ace, identity, expect_dir)
    ));
    let lower = ace.to_ascii_lowercase();
    if lower.contains("deny") {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    if lower.contains("(i)") {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    let colon = ace.find(':').ok_or(ForgeCredentialError::WindowsAcl)?;
    let principal = ace[..colon].trim();
    let matches_sid =
        !identity.sid.is_empty() && principal.eq_ignore_ascii_case(identity.sid.as_str());
    let matches_account =
        !identity.account.is_empty() && principal.eq_ignore_ascii_case(identity.account.as_str());
    if !matches_sid && !matches_account {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    let flags_part = &ace[colon + 1..];
    let flags_lower = flags_part.to_ascii_lowercase();
    if !flags_lower.contains("(f)") {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    let mut tokens: Vec<String> = Vec::new();
    let mut chars = flags_lower.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '(' {
            let mut tok = String::new();
            for c in chars.by_ref() {
                if c == ')' {
                    break;
                }
                tok.push(c);
            }
            tokens.push(tok);
        }
    }
    let mut seen_f = 0;
    let mut object_inherit_count = 0;
    let mut container_inherit_count = 0;
    for tok in &tokens {
        match tok.as_str() {
            "f" => seen_f += 1,
            "oi" => object_inherit_count += 1,
            "ci" => container_inherit_count += 1,
            _ => return Err(ForgeCredentialError::WindowsAcl),
        }
    }
    if seen_f != 1 {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    if expect_dir {
        if object_inherit_count != 1 || container_inherit_count != 1 {
            return Err(ForgeCredentialError::WindowsAcl);
        }
    } else if object_inherit_count != 0 || container_inherit_count != 0 {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    let mut stripped = flags_lower.clone();
    stripped = stripped.replace("(f)", "");
    stripped = stripped.replace("(oi)", "");
    stripped = stripped.replace("(ci)", "");
    stripped = stripped.replace([' ', '\t', ','], "");
    if !stripped.trim().is_empty() {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    for forbidden in ["everyone", "builtin", "nt authority", "authenticated users"] {
        if lower.contains(forbidden) {
            return Err(ForgeCredentialError::WindowsAcl);
        }
    }
    Ok(())
}

#[cfg(windows)]
fn verify_windows_dacl(path: &Path, expected_sid: &str) -> Result<(), ForgeCredentialError> {
    let identity = resolve_current_identity()?;
    if !identity.sid.eq_ignore_ascii_case(expected_sid) {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    let path_str = path.to_string_lossy().to_string();
    let output = hidden_output("icacls.exe", &[&path_str], Duration::from_secs(5))?;
    if !output.status.success() {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    let text = String::from_utf8_lossy(&output.stdout).to_string();
    let is_dir = fs::metadata(path).is_ok_and(|metadata| metadata.is_dir());
    parse_icacls_strict_with_identity(&text, &identity, is_dir, &path_str)
}

#[cfg(windows)]
fn restrict_directory_windows(dir: &Path) -> Result<(), ForgeCredentialError> {
    acl_diagnostic!(acl_diagnostic::stage("DirectoryAclMutation"));
    let identity = resolve_current_identity()?;
    let dir_str = dir.to_string_lossy().to_string();
    let grant = format!("*{}:(OI)(CI)F", identity.sid);
    let output = hidden_output(
        "icacls.exe",
        &[&dir_str, "/inheritance:r", "/grant:r", &grant],
        Duration::from_secs(5),
    )?;
    if !output.status.success() {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    acl_diagnostic!(acl_diagnostic::stage("DirectoryAclVerification"));
    let query = hidden_output("icacls.exe", &[&dir_str], Duration::from_secs(5))?;
    if !query.status.success() {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    let text = String::from_utf8_lossy(&query.stdout).to_string();
    let removals = plan_icacls_removals(&text, &identity, &dir_str)?;
    acl_diagnostic!(acl_diagnostic::stage("DirectoryAclMutation"));
    for principal in &removals {
        let removal = icacls_remove_argument(principal);
        let output = hidden_output(
            "icacls.exe",
            &[&dir_str, "/remove", removal.as_str()],
            Duration::from_secs(5),
        )?;
        if !output.status.success() {
            return Err(ForgeCredentialError::WindowsAcl);
        }
    }
    let output = hidden_output(
        "icacls.exe",
        &[&dir_str, "/grant:r", &grant],
        Duration::from_secs(5),
    )?;
    if !output.status.success() {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    acl_diagnostic!(acl_diagnostic::stage("DirectoryAclVerification"));
    verify_windows_dacl(dir, &identity.sid)?;
    Ok(())
}

#[cfg(windows)]
fn icacls_remove_argument(principal: &str) -> String {
    if is_valid_sid(principal) {
        format!("*{principal}")
    } else {
        principal.to_owned()
    }
}

#[cfg(windows)]
fn restrict_file_windows(path: &Path, sid: &str) -> Result<(), ForgeCredentialError> {
    acl_diagnostic!(acl_diagnostic::stage("BundleAclMutation"));
    let identity = resolve_current_identity()?;
    if !identity.sid.eq_ignore_ascii_case(sid) {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    let path_str = path.to_string_lossy().to_string();
    let grant = format!("*{sid}:F");
    let output = hidden_output(
        "icacls.exe",
        &[&path_str, "/inheritance:r", "/grant:r", &grant],
        Duration::from_secs(5),
    )?;
    if !output.status.success() {
        return Err(ForgeCredentialError::WindowsAcl);
    }
    acl_diagnostic!(acl_diagnostic::stage("BundleAclVerification"));
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

fn ensure_credentials_dir(dir: &Path) -> Result<(), ForgeCredentialError> {
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
                acl_diagnostic!(acl_diagnostic::stage("DirectoryAclVerification"));
                let identity = resolve_current_identity()?;
                verify_windows_dacl(dir, &identity.sid)?;
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
    {
        let open_id = file_id_from_file(&file)?;
        let path_id = file_id(lock_path)?;
        if open_id != path_id {
            return Err(ForgeCredentialError::UnsafePath(lock_path.to_path_buf()));
        }
    }
    #[cfg(unix)]
    check_file_mode(lock_path)?;
    #[cfg(windows)]
    {
        acl_diagnostic!(acl_diagnostic::stage("ProvisionLockVerification"));
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
            acl_diagnostic!(acl_diagnostic::stage("BundleAclVerification"));
            let identity = resolve_current_identity()?;
            verify_windows_dacl(file, &identity.sid)?;
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
        rcgen::SanType::IpAddress(IpAddr::V4(Ipv4Addr::LOCALHOST)),
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
            path: temp_path.clone(),
        });
    }
    temp_guard.disarm();
    Ok(())
}

fn cleanup_created(mut created: Vec<CreatedFile>) {
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
    use super::{
        CurrentIdentity, is_icacls_success_summary, parse_icacls_output_with_path,
        parse_icacls_strict_with_identity, parse_icacls_strict_with_path, plan_icacls_removals,
    };

    #[test]
    fn windows_dacl_parser_strict() {
        let sid = "S-1-5-21-1-2-3-1000";
        let account = "TEST\\User";
        let identity = CurrentIdentity {
            sid: sid.to_string(),
            account: account.to_string(),
        };
        let dir_path = "C:\\creds";
        let file_path = "C:\\creds\\file";
        let good_dir_sid = format!("{dir_path} {sid}:(OI)(CI)(F)");
        assert!(parse_icacls_output_with_path(&good_dir_sid, sid, dir_path).is_ok());
        assert!(parse_icacls_strict_with_path(&good_dir_sid, sid, true, dir_path).is_ok());
        let good_dir_account = format!("{dir_path} {account}:(OI)(CI)(F)");
        assert!(
            parse_icacls_strict_with_identity(&good_dir_account, &identity, true, dir_path).is_ok()
        );
        // A drive-letter path must be stripped only when the complete queried path matches.
        let drive_file = format!("{file_path} {sid}:(F)");
        assert!(parse_icacls_strict_with_path(&drive_file, sid, false, file_path).is_ok());
        assert!(
            parse_icacls_strict_with_path(&drive_file, sid, false, "C:\\creds\\other").is_err()
        );
        let sid_prefix = format!("{file_path} {sid}00:(F)");
        assert!(parse_icacls_strict_with_path(&sid_prefix, sid, false, file_path).is_err());
        let with_inherited = format!("{dir_path} {sid}:(I)(OI)(CI)(F)");
        assert!(parse_icacls_output_with_path(&with_inherited, sid, dir_path).is_err());
        let two_sids = format!("{dir_path} {sid}:(F)\nS-1-5-21-1-2-3-1001:(F)");
        assert!(parse_icacls_output_with_path(&two_sids, sid, dir_path).is_err());
        let deny = format!("{dir_path} {sid}:(DENY)(F)");
        assert!(parse_icacls_output_with_path(&deny, sid, dir_path).is_err());
        let everyone = format!("{dir_path} BUILTIN\\Users:(F) {sid}:(F)");
        assert!(parse_icacls_output_with_path(&everyone, sid, dir_path).is_err());
        let named_extra = format!("{dir_path} {sid}:(F)\nDOMAIN\\OtherUser:(F)");
        assert!(parse_icacls_output_with_path(&named_extra, sid, dir_path).is_err());
        assert!(parse_icacls_strict_with_path(&named_extra, sid, false, dir_path).is_err());
        let extra_flags = format!("{dir_path} {sid}:(OI)(CI)(F)(M)");
        assert!(parse_icacls_strict_with_path(&extra_flags, sid, true, dir_path).is_err());
        let broad = format!("{dir_path} {sid}:(OI)(CI)(M)");
        assert!(parse_icacls_strict_with_path(&broad, sid, true, dir_path).is_err());
        // Exact queried path containing spaces
        let spaced_path = "C:\\My Documents\\Artisan creds";
        let spaced_good = format!("{spaced_path} {sid}:(OI)(CI)(F)");
        assert!(parse_icacls_strict_with_path(&spaced_good, sid, true, spaced_path).is_ok());
        assert!(
            parse_icacls_strict_with_identity(&spaced_good, &identity, true, spaced_path).is_ok()
        );
        let spaced_wrong_prefix = format!("C:\\My Documents\\Other {sid}:(OI)(CI)(F)");
        assert!(
            parse_icacls_strict_with_path(&spaced_wrong_prefix, sid, true, spaced_path).is_err()
        );
        // Duplicate tokens
        let dup_f_file = format!("{file_path} {sid}:(F)(F)");
        assert!(parse_icacls_strict_with_path(&dup_f_file, sid, false, file_path).is_err());
        let dup_oi_dir = format!("{dir_path} {sid}:(OI)(OI)(CI)(F)");
        assert!(parse_icacls_strict_with_path(&dup_oi_dir, sid, true, dir_path).is_err());
        // Trailing junk
        let trailing = format!("{file_path} {sid}:(F) extra");
        assert!(parse_icacls_strict_with_path(&trailing, sid, false, file_path).is_err());
        let localized = format!("{dir_path} {sid}:(OI)(CI)(F)\nDacl access");
        assert!(parse_icacls_output_with_path(&localized, sid, dir_path).is_err());
    }

    #[test]
    fn icacls_success_summary_requires_one_successful_target() {
        for summary in [
            "successfully processed 1 files; failed processing 0 files",
            "successfully processed 1 files; failed processing 0 files.",
        ] {
            assert!(is_icacls_success_summary(summary));
        }
        for summary in [
            "successfully processed 2 files; failed processing 0 files",
            "successfully processed 1 files; failed processing 1 files",
            "successfully processed one files; failed processing 0 files",
            "successfully processed 1 files; failed processing 0 files. trailing",
            "successfully processed 1 files; failed processing 0 files..",
            "erfolgreich verarbeitet 1 files; failed processing 0 files",
        ] {
            assert!(!is_icacls_success_summary(summary));
        }
    }

    #[test]
    fn dacl_convergence_plans_all_extra_explicit_principals() {
        let sid = "S-1-5-21-1-2-3-1000";
        let account = "TEST\\User";
        let identity = CurrentIdentity {
            sid: sid.to_string(),
            account: account.to_string(),
        };
        let path = "C:\\creds";
        let output = format!(
            "{path} {sid}:(OI)(CI)(F)\nDOMAIN\\Runner:(OI)(CI)(F)\nBUILTIN\\Administrators:(F)\nEveryone:(F)"
        );
        assert_eq!(
            plan_icacls_removals(&output, &identity, path).unwrap(),
            vec![
                "DOMAIN\\Runner".to_string(),
                "BUILTIN\\Administrators".to_string(),
                "Everyone".to_string(),
            ]
        );
        assert!(parse_icacls_strict_with_identity(&output, &identity, true, path).is_err());

        let account_output = format!("{path} {account}:(OI)(CI)(F)\nDOMAIN\\Runner:(OI)(CI)(F)");
        assert_eq!(
            plan_icacls_removals(&account_output, &identity, path).unwrap(),
            vec!["DOMAIN\\Runner".to_string()]
        );

        let lowercase_sid = sid.to_ascii_lowercase();
        let converged_output = format!("{path} {lowercase_sid}:(OI)(CI)(F)");
        assert_eq!(
            plan_icacls_removals(&converged_output, &identity, path).unwrap(),
            Vec::<String>::new()
        );
        assert!(
            parse_icacls_strict_with_identity(&converged_output, &identity, true, path).is_ok()
        );
    }

    #[test]
    fn dacl_convergence_rejects_malformed_duplicate_and_ambiguous_identities() {
        let sid = "S-1-5-21-1-2-3-1000";
        let account = "TEST\\User";
        let identity = CurrentIdentity {
            sid: sid.to_string(),
            account: account.to_string(),
        };
        let path = "C:\\creds";
        for output in [
            format!("{path} DOMAIN/Runner:(F)"),
            format!("{path} DOMAIN\\Runner:(F)\nDOMAIN\\runner:(M)"),
            format!("{path} {sid}:(F)\n{account}:(F)"),
            format!("{path} {sid}:(I)(OI)(CI)(F)"),
        ] {
            assert!(plan_icacls_removals(&output, &identity, path).is_err());
        }

        let malformed_identity = CurrentIdentity {
            sid: "not-a-sid".into(),
            account: account.into(),
        };
        let exact = format!("{path} {sid}:(OI)(CI)(F)");
        assert!(plan_icacls_removals(&exact, &malformed_identity, path).is_err());
    }
}

#[cfg(all(test, windows))]
mod diagnostic_tests {
    use super::*;
    use std::path::PathBuf;

    fn assert_artifact(bytes: &[u8]) {
        assert!(bytes.len() <= 16 * 1024);
        let value: serde_json::Value = serde_json::from_slice(bytes).expect("diagnostic JSON");
        let object = value.as_object().expect("diagnostic object");
        assert_eq!(
            object
                .get("schema_version")
                .and_then(serde_json::Value::as_u64),
            Some(1)
        );
        let events = object
            .get("events")
            .and_then(serde_json::Value::as_array)
            .expect("diagnostic events");
        assert!(events.len() <= acl_diagnostic::MAX_EVENTS);
        assert_eq!(
            object
                .get("event_count")
                .and_then(serde_json::Value::as_u64),
            Some(events.len() as u64)
        );
        let text = String::from_utf8_lossy(bytes);
        for canary in acl_diagnostic::CANARIES {
            assert!(!text.contains(canary));
        }
    }

    fn assert_classification(
        kind: &'static str,
        expected: &'static str,
        expect_success: bool,
        allowed: &[&str],
        operation: impl FnOnce() -> Result<Vec<String>, ForgeCredentialError>,
    ) {
        let (result, captured) = acl_diagnostic::capture(operation);
        assert_eq!(result.is_ok(), expect_success);
        let record = captured.finish(if expect_success {
            "Success"
        } else {
            "WindowsAcl"
        });
        acl_diagnostic::assert_redacted(&record);
        let bytes = serde_json::to_vec(&record).expect("bounded classification diagnostic");
        assert_artifact(&bytes);
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("classification JSON");
        let events = value
            .get("events")
            .and_then(serde_json::Value::as_array)
            .expect("classification events");
        let classification_values: Vec<&str> = events
            .iter()
            .filter_map(|event| {
                let object = event.as_object()?;
                if object.get("kind").and_then(serde_json::Value::as_str) != Some(kind) {
                    return None;
                }
                object.get("value").and_then(serde_json::Value::as_str)
            })
            .collect();
        assert!(classification_values.contains(&expected));
        assert!(
            classification_values
                .iter()
                .all(|value| allowed.contains(value))
        );
    }

    fn assert_planner_classification(
        expected: &'static str,
        expect_success: bool,
        operation: impl FnOnce() -> Result<Vec<String>, ForgeCredentialError>,
    ) {
        let allowed = [
            "InvalidValidatedIdentity",
            "MissingAceSeparator",
            "InheritedAce",
            "CurrentIdentityMatch",
            "DuplicateCurrentIdentity",
            "SafeRemovableExtra",
            "UnsafeNonmatchingExtra",
            "DuplicateExtra",
            "PlanComplete",
        ];
        assert_classification("planner", expected, expect_success, &allowed, operation);
    }

    fn assert_parser_classification(
        expected: &'static str,
        expect_success: bool,
        operation: impl FnOnce() -> Result<Vec<String>, ForgeCredentialError>,
    ) {
        let allowed = [
            "MalformedSuccessSummary",
            "MissingSeparator",
            "MissingOpeningToken",
            "NonTokenContent",
            "UnterminatedToken",
            "EmptyToken",
            "NoTokens",
            "AcceptedAce",
            "ParserComplete",
        ];
        assert_classification("parser", expected, expect_success, &allowed, operation);
    }

    #[test]
    fn parser_diagnostic_classifications_are_bounded_and_redacted() {
        let sid = acl_diagnostic::CANARIES[0];
        let path = acl_diagnostic::CANARIES[2];
        let accepted = format!("{path} {sid}:(OI)(CI)(F)");

        assert_parser_classification("MalformedSuccessSummary", false, || {
            collect_icacls_ace_lines(
                &format!(
                    "{accepted}\nSuccessfully processed many files; Failed processing 0 files."
                ),
                path,
            )
        });
        assert_parser_classification("MissingSeparator", false, || {
            collect_icacls_ace_lines(&format!("{path} non-ace output"), path)
        });
        assert_parser_classification("MissingOpeningToken", false, || {
            collect_icacls_ace_lines(&format!("{path} {sid}:F"), path)
        });
        assert_parser_classification("NonTokenContent", false, || {
            collect_icacls_ace_lines(&format!("{path} {sid}:(F) trailing"), path)
        });
        assert_parser_classification("UnterminatedToken", false, || {
            collect_icacls_ace_lines(&format!("{path} {sid}:(F"), path)
        });
        assert_parser_classification("EmptyToken", false, || {
            collect_icacls_ace_lines(&format!("{path} {sid}:()"), path)
        });
        assert_parser_classification("NoTokens", false, || {
            collect_icacls_ace_lines(&format!("{path} {sid}:"), path)
        });
        assert_parser_classification("AcceptedAce", true, || {
            collect_icacls_ace_lines(&accepted, path)
        });
        assert_parser_classification("ParserComplete", true, || {
            collect_icacls_ace_lines(&accepted, path)
        });
    }

    #[test]
    fn planner_diagnostic_classifications_are_bounded_and_redacted() {
        let sid = acl_diagnostic::CANARIES[0];
        let account = acl_diagnostic::CANARIES[1];
        let path = acl_diagnostic::CANARIES[2];
        let identity = CurrentIdentity {
            sid: sid.to_string(),
            account: account.to_string(),
        };

        let invalid_identity = CurrentIdentity {
            sid: "not-a-sid".to_string(),
            account: account.to_string(),
        };
        assert_planner_classification("InvalidValidatedIdentity", false, || {
            plan_icacls_removals("", &invalid_identity, path)
        });
        assert_planner_classification("MissingAceSeparator", false, || {
            plan_icacls_removals(&format!("{path} non-ace output"), &identity, path)
        });
        assert_planner_classification("InheritedAce", false, || {
            plan_icacls_removals(&format!("{path} {sid}:(I)(OI)(CI)(F)"), &identity, path)
        });
        assert_planner_classification("CurrentIdentityMatch", true, || {
            plan_icacls_removals(&format!("{path} {sid}:(OI)(CI)(F)"), &identity, path)
        });
        assert_planner_classification("DuplicateCurrentIdentity", false, || {
            plan_icacls_removals(
                &format!("{path} {sid}:(OI)(CI)(F)\n{account}:(OI)(CI)(F)"),
                &identity,
                path,
            )
        });
        assert_planner_classification("SafeRemovableExtra", true, || {
            plan_icacls_removals(
                &format!("{path} {sid}:(OI)(CI)(F)\nEveryone:(F)"),
                &identity,
                path,
            )
        });
        assert_planner_classification("UnsafeNonmatchingExtra", false, || {
            plan_icacls_removals(
                &format!("{path} {sid}:(OI)(CI)(F)\nmalformed/principal:(F)"),
                &identity,
                path,
            )
        });
        assert_planner_classification("DuplicateExtra", false, || {
            plan_icacls_removals(
                &format!("{path} {sid}:(OI)(CI)(F)\nEveryone:(F)\nEVERYONE:(F)"),
                &identity,
                path,
            )
        });
        assert_planner_classification("PlanComplete", true, || {
            plan_icacls_removals(&format!("{path} {sid}:(OI)(CI)(F)"), &identity, path)
        });
    }

    #[test]
    fn diagnostic_capture_is_bounded_redacted_and_noninterfering() {
        for (bytes, shape) in [
            (&[][..], "Empty"),
            (&[0xef, 0xbb, 0xbf][..], "Bom"),
            (&[0xff][..], "InvalidUtf8"),
        ] {
            assert_eq!(acl_diagnostic::stream_shape(bytes), shape);
        }
        assert_eq!(
            acl_diagnostic::stream_shape(&[b'x'; acl_diagnostic::MAX_STREAM_BYTES]),
            "Utf8"
        );
        assert_eq!(
            acl_diagnostic::stream_shape(&[b'x'; acl_diagnostic::MAX_STREAM_BYTES + 1]),
            "Oversized"
        );

        let sid = "S-1-5-21-1-2-3-1000";
        let identity = CurrentIdentity {
            sid: sid.to_string(),
            account: "DOMAIN\\account-canary".to_string(),
        };
        let raw_path = "C:\\sensitive\\path";
        let raw_output = format!(
            "{raw_path} {sid}:(OI)(CI)(F)\nSuccessfully processed 1 files; Failed processing 0 files."
        );
        let raw_stderr = acl_diagnostic::CANARIES.join("\n");
        let (result, captured) = acl_diagnostic::capture(|| {
            acl_diagnostic::stage("DirectoryAclVerification");
            acl_diagnostic::record_identity(
                raw_output.as_bytes(),
                raw_stderr.as_bytes(),
                2,
                Some(sid),
                Some(identity.account.as_str()),
            );
            parse_icacls_strict_with_identity(&raw_output, &identity, true, raw_path)
        });
        assert!(result.is_ok());
        let record = captured.finish("Success");
        acl_diagnostic::assert_redacted(&record);
        let json = serde_json::to_string(&record).expect("structural diagnostic serialization");
        assert!(
            ["Completed", "identity", "acl_count", "ace", "Accepted"]
                .iter()
                .all(|value| json.contains(value))
        );

        for output in [
            format!("C:\\creds {sid}:(OI)(CI)(F)"),
            format!("C:\\creds {sid}:(DENY)(F)"),
            format!("C:\\creds {sid}:(F) extra"),
        ] {
            let expected = parse_icacls_strict_with_identity(&output, &identity, true, "C:\\creds");
            let (actual, captured) = acl_diagnostic::capture(|| {
                parse_icacls_strict_with_identity(&output, &identity, true, "C:\\creds")
            });
            let record = captured.finish("Other");
            if output.contains("DENY") {
                assert!(serde_json::to_string(&record).unwrap().contains("Deny"));
            }
            assert_eq!(actual, expected);
        }

        let ((), captured) = acl_diagnostic::capture(|| {
            for _ in 0..=acl_diagnostic::MAX_EVENTS {
                acl_diagnostic::event("probe", "ExitedNonZero");
            }
        });
        let overflow = captured.finish("WindowsAcl");
        let bytes = serde_json::to_vec(&overflow).expect("bounded diagnostic serialization");
        assert!(String::from_utf8_lossy(&bytes).contains("\"overflow\":true"));
        assert_artifact(&bytes);
    }

    #[test]
    fn retained_windows_acl_diagnostic() {
        if std::env::var_os("ARTISAN_NATIVE_ACL_DIAGNOSTIC").as_deref()
            != Some(std::ffi::OsStr::new("1"))
        {
            return;
        }
        let artifact_path = std::env::var_os("ARTISAN_NATIVE_ACL_DIAGNOSTIC_FILE").map_or_else(
            || panic!("ARTISAN_NATIVE_ACL_DIAGNOSTIC_FILE is required"),
            PathBuf::from,
        );
        assert!(artifact_path.is_absolute());
        let home = tempfile::tempdir().expect("temporary diagnostic home");
        let (result, captured) = acl_diagnostic::capture(|| provision_or_load(home.path()));
        let outcome = match &result {
            Ok(_) => "Success",
            Err(ForgeCredentialError::WindowsAcl) => "WindowsAcl",
            Err(ForgeCredentialError::Provisioning) => "Provisioning",
            Err(_) => "Other",
        };
        let record = captured.finish(outcome);
        acl_diagnostic::assert_redacted(&record);
        acl_diagnostic::write(&artifact_path, &record)
            .expect("unable to write retained ACL diagnostic artifact");
        let artifact = fs::read(&artifact_path).expect("retained ACL diagnostic artifact missing");
        assert_artifact(&artifact);
    }
}
