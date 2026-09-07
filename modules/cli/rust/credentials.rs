use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr},
    num::NonZeroU32,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use fs2::FileExt;
use rcgen::PublicKeyData;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

const MAX_MANIFEST_BYTES: usize = 4_096;
const MAX_MANIFEST_READ_BYTES: usize = MAX_MANIFEST_BYTES + 1;
const MAX_CAPABILITY_BYTES: usize = 32;
const MAX_CAPABILITY_READ_BYTES: usize = MAX_CAPABILITY_BYTES + 1;
const MAX_CERTIFICATE_BYTES: usize = 65_536;
const MAX_CERTIFICATE_READ_BYTES: usize = MAX_CERTIFICATE_BYTES + 1;
const MAX_PRIVATE_KEY_BYTES: usize = 65_536;
const MAX_PRIVATE_KEY_READ_BYTES: usize = MAX_PRIVATE_KEY_BYTES + 1;
const SAFE_READ_CHUNK_BYTES: usize = 4_096;
const RECONNECT_RECORD_MAGIC: [u8; 8] = *b"ARTNRC01";
const RECONNECT_RECORD_VERSION: u8 = 1;
const RECONNECT_RECORD_BYTES: usize = 8
    + 1
    + 1
    + std::mem::size_of::<u64>()
    + 16
    + std::mem::size_of::<u16>()
    + 32
    + std::mem::size_of::<u32>()
    + 16
    + artisan_protocol::RECONNECT_CAPABILITY_BYTES;
const RECONNECT_CAPABILITY_FILENAME: &str = "reconnect-capability.bin";
const RECONNECT_LOCK_FILENAME: &str = ".reconnect-capability.lock";

/// Maximum time spent waiting for exclusive reconnect-capability ownership.
pub const RECONNECT_LOCK_TIMEOUT: Duration = Duration::from_millis(250);

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
    CapabilityBusy,
    ReconnectRecordMissing,
    ReconnectRecordMalformed,
    ReconnectCapabilityUnavailable,
    ReconnectBindingMismatch,
    ReconnectStaleWriter,
    ReconnectGenerationOverflow,
    ReconnectInvalidBinding,
    ReconnectAttemptComplete,
    ReconnectRecordExists,
    IdentityBundleMissing,
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
            Self::CapabilityBusy => write!(f, "reconnect capability store is busy"),
            Self::ReconnectRecordMissing => write!(f, "reconnect capability record is missing"),
            Self::ReconnectRecordMalformed => write!(f, "invalid reconnect capability record"),
            Self::ReconnectCapabilityUnavailable => {
                write!(f, "reconnect capability is unavailable")
            }
            Self::ReconnectBindingMismatch => write!(f, "reconnect binding mismatch"),
            Self::ReconnectStaleWriter => write!(f, "reconnect capability writer is stale"),
            Self::ReconnectGenerationOverflow => {
                write!(f, "reconnect capability generation overflow")
            }
            Self::ReconnectInvalidBinding => write!(f, "invalid reconnect binding"),
            Self::ReconnectAttemptComplete => write!(f, "reconnect attempt is already complete"),
            Self::ReconnectRecordExists => write!(f, "reconnect capability record already exists"),
            Self::IdentityBundleMissing => write!(f, "client identity bundle is missing"),
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
            Self::CapabilityBusy => f.debug_tuple("CapabilityBusy").finish(),
            Self::ReconnectRecordMissing => f.debug_tuple("ReconnectRecordMissing").finish(),
            Self::ReconnectRecordMalformed => f.debug_tuple("ReconnectRecordMalformed").finish(),
            Self::ReconnectCapabilityUnavailable => {
                f.debug_tuple("ReconnectCapabilityUnavailable").finish()
            }
            Self::ReconnectBindingMismatch => f.debug_tuple("ReconnectBindingMismatch").finish(),
            Self::ReconnectStaleWriter => f.debug_tuple("ReconnectStaleWriter").finish(),
            Self::ReconnectGenerationOverflow => {
                f.debug_tuple("ReconnectGenerationOverflow").finish()
            }
            Self::ReconnectInvalidBinding => f.debug_tuple("ReconnectInvalidBinding").finish(),
            Self::ReconnectAttemptComplete => f.debug_tuple("ReconnectAttemptComplete").finish(),
            Self::ReconnectRecordExists => f.debug_tuple("ReconnectRecordExists").finish(),
            Self::IdentityBundleMissing => f.debug_tuple("IdentityBundleMissing").finish(),
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

    /// Returns the rotated reconnect-capability record path.
    pub fn reconnect_capability_path(&self) -> PathBuf {
        self.credentials_dir().join(RECONNECT_CAPABILITY_FILENAME)
    }

    /// Returns the whole-session reconnect-capability lock path.
    pub fn reconnect_lock_path(&self) -> PathBuf {
        self.credentials_dir().join(RECONNECT_LOCK_FILENAME)
    }
}

pub struct NativeClientCredentials {
    paths: ForgeCredentialPaths,
    certificate: rustls_pki_types::CertificateDer<'static>,
    capability: artisan_protocol::LocalCapability,
}

impl NativeClientCredentials {
    pub fn paths(&self) -> &ForgeCredentialPaths {
        &self.paths
    }

    pub fn into_parts(
        self,
    ) -> (
        rustls_pki_types::CertificateDer<'static>,
        artisan_protocol::LocalCapability,
    ) {
        (self.certificate, self.capability)
    }
}

/// Existing client identity material validated without provisioning authority.
///
/// The private key is deliberately kept behind the validated credential paths;
/// callers that need it can use the existing private-file boundary owned by
/// the Forge launcher. This value never contains the bootstrap capability.
pub struct NativeClientIdentity {
    paths: ForgeCredentialPaths,
    certificate: rustls_pki_types::CertificateDer<'static>,
}

impl NativeClientIdentity {
    /// Returns the paths whose certificate and private key were validated.
    #[must_use]
    pub fn paths(&self) -> &ForgeCredentialPaths {
        &self.paths
    }

    /// Returns the validated leaf certificate.
    #[must_use]
    pub fn certificate(&self) -> &rustls_pki_types::CertificateDer<'static> {
        &self.certificate
    }

    /// Consumes the identity into its validated paths and leaf certificate.
    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        ForgeCredentialPaths,
        rustls_pki_types::CertificateDer<'static>,
    ) {
        (self.paths, self.certificate)
    }
}

/// The durable state of the reconnect-capability record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReconnectCapabilityState {
    /// A capability is available for one checkout.
    Ready,
    /// A capability is held by the current session lease.
    InFlight,
    /// The prior capability was abandoned or its outcome is ambiguous.
    Lost,
}

/// Non-secret identity that fences reconnect capability custody to one Forge
/// instance and one native client process.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct ReconnectBinding {
    pub instance_id: [u8; 16],
    pub endpoint_port: u16,
    pub certificate_sha256: [u8; 32],
    pub pid: NonZeroU32,
}

impl std::fmt::Debug for ReconnectBinding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ReconnectBinding")
            .field("instance_id", &"[REDACTED]")
            .field("endpoint_port", &self.endpoint_port)
            .field("certificate_sha256", &"[REDACTED]")
            .field("pid", &self.pid)
            .finish()
    }
}

impl ReconnectBinding {
    /// Constructs a binding after checking its nonzero identity, port, and PID.
    pub fn new(
        instance_id: [u8; 16],
        endpoint_port: u16,
        certificate_sha256: [u8; 32],
        pid: NonZeroU32,
    ) -> Result<Self, ForgeCredentialError> {
        let binding = Self {
            instance_id,
            endpoint_port,
            certificate_sha256,
            pid,
        };
        binding.validate()?;
        Ok(binding)
    }

    /// Constructs a binding using the conventional fallible constructor name.
    pub fn try_new(
        instance_id: [u8; 16],
        endpoint_port: u16,
        certificate_sha256: [u8; 32],
        pid: NonZeroU32,
    ) -> Result<Self, ForgeCredentialError> {
        Self::new(instance_id, endpoint_port, certificate_sha256, pid)
    }

    /// Validates every persisted binding component, including its identity.
    pub fn validate(self) -> Result<(), ForgeCredentialError> {
        if bytes_are_zero(&self.instance_id) || self.endpoint_port == 0 || self.pid.get() == 0 {
            return Err(ForgeCredentialError::ReconnectInvalidBinding);
        }
        Ok(())
    }
}

struct ReconnectRecord {
    state: ReconnectCapabilityState,
    generation: u64,
    binding: ReconnectBinding,
    owner_nonce: Zeroizing<[u8; 16]>,
    capability: Zeroizing<[u8; artisan_protocol::RECONNECT_CAPABILITY_BYTES]>,
}

impl ReconnectRecord {
    fn ready(
        binding: ReconnectBinding,
        generation: u64,
        capability: artisan_protocol::ReconnectCapability,
    ) -> Result<Self, ForgeCredentialError> {
        Self::ready_from_bytes(binding, generation, capability.into_zeroizing_bytes())
    }

    fn ready_from_bytes(
        binding: ReconnectBinding,
        generation: u64,
        capability: Zeroizing<[u8; artisan_protocol::RECONNECT_CAPABILITY_BYTES]>,
    ) -> Result<Self, ForgeCredentialError> {
        let record = Self {
            state: ReconnectCapabilityState::Ready,
            generation,
            binding,
            owner_nonce: Zeroizing::new([0_u8; 16]),
            capability,
        };
        validate_reconnect_record(&record)?;
        Ok(record)
    }

    fn in_flight(binding: ReconnectBinding, generation: u64, owner_nonce: &[u8; 16]) -> Self {
        Self {
            state: ReconnectCapabilityState::InFlight,
            generation,
            binding,
            owner_nonce: Zeroizing::new(*owner_nonce),
            capability: Zeroizing::new([0_u8; artisan_protocol::RECONNECT_CAPABILITY_BYTES]),
        }
    }

    fn lost(binding: ReconnectBinding, generation: u64) -> Self {
        Self {
            state: ReconnectCapabilityState::Lost,
            generation,
            binding,
            owner_nonce: Zeroizing::new([0_u8; 16]),
            capability: Zeroizing::new([0_u8; artisan_protocol::RECONNECT_CAPABILITY_BYTES]),
        }
    }
}

fn bytes_are_zero(bytes: &[u8]) -> bool {
    bytes.iter().all(|byte| *byte == 0)
}

fn validate_reconnect_record(record: &ReconnectRecord) -> Result<(), ForgeCredentialError> {
    if record.binding.validate().is_err() || record.generation == 0 {
        return Err(ForgeCredentialError::ReconnectRecordMalformed);
    }
    let nonce_is_zero = bytes_are_zero(record.owner_nonce.as_ref());
    let capability_is_zero = bytes_are_zero(record.capability.as_ref());
    let valid = match record.state {
        ReconnectCapabilityState::Ready => !capability_is_zero && nonce_is_zero,
        ReconnectCapabilityState::InFlight => capability_is_zero && !nonce_is_zero,
        ReconnectCapabilityState::Lost => capability_is_zero && nonce_is_zero,
    };
    if valid {
        Ok(())
    } else {
        Err(ForgeCredentialError::ReconnectRecordMalformed)
    }
}

fn encode_reconnect_record(
    record: &ReconnectRecord,
) -> Result<Zeroizing<[u8; RECONNECT_RECORD_BYTES]>, ForgeCredentialError> {
    validate_reconnect_record(record)?;
    let mut bytes = Zeroizing::new([0_u8; RECONNECT_RECORD_BYTES]);
    let mut offset = 0;
    bytes[offset..offset + RECONNECT_RECORD_MAGIC.len()].copy_from_slice(&RECONNECT_RECORD_MAGIC);
    offset += RECONNECT_RECORD_MAGIC.len();
    bytes[offset] = RECONNECT_RECORD_VERSION;
    offset += 1;
    bytes[offset] = match record.state {
        ReconnectCapabilityState::Ready => 0,
        ReconnectCapabilityState::InFlight => 1,
        ReconnectCapabilityState::Lost => 2,
    };
    offset += 1;
    bytes[offset..offset + std::mem::size_of::<u64>()]
        .copy_from_slice(&record.generation.to_le_bytes());
    offset += std::mem::size_of::<u64>();
    bytes[offset..offset + record.binding.instance_id.len()]
        .copy_from_slice(&record.binding.instance_id);
    offset += record.binding.instance_id.len();
    bytes[offset..offset + std::mem::size_of::<u16>()]
        .copy_from_slice(&record.binding.endpoint_port.to_le_bytes());
    offset += std::mem::size_of::<u16>();
    bytes[offset..offset + record.binding.certificate_sha256.len()]
        .copy_from_slice(&record.binding.certificate_sha256);
    offset += record.binding.certificate_sha256.len();
    bytes[offset..offset + std::mem::size_of::<u32>()]
        .copy_from_slice(&record.binding.pid.get().to_le_bytes());
    offset += std::mem::size_of::<u32>();
    bytes[offset..offset + 16].copy_from_slice(record.owner_nonce.as_ref());
    offset += 16;
    bytes[offset..offset + artisan_protocol::RECONNECT_CAPABILITY_BYTES]
        .copy_from_slice(record.capability.as_ref());
    Ok(bytes)
}

struct ReconnectRecordMetadata {
    state: ReconnectCapabilityState,
    generation: u64,
    binding: ReconnectBinding,
    owner_nonce: Zeroizing<[u8; 16]>,
}

fn decode_reconnect_record_metadata(
    bytes: &[u8],
) -> Result<ReconnectRecordMetadata, ForgeCredentialError> {
    if bytes.len() != RECONNECT_RECORD_BYTES {
        return Err(ForgeCredentialError::ReconnectRecordMalformed);
    }
    let mut offset = 0;
    if bytes[offset..offset + RECONNECT_RECORD_MAGIC.len()] != RECONNECT_RECORD_MAGIC {
        return Err(ForgeCredentialError::ReconnectRecordMalformed);
    }
    offset += RECONNECT_RECORD_MAGIC.len();
    if bytes[offset] != RECONNECT_RECORD_VERSION {
        return Err(ForgeCredentialError::ReconnectRecordMalformed);
    }
    offset += 1;
    let state = match bytes[offset] {
        0 => ReconnectCapabilityState::Ready,
        1 => ReconnectCapabilityState::InFlight,
        2 => ReconnectCapabilityState::Lost,
        _ => return Err(ForgeCredentialError::ReconnectRecordMalformed),
    };
    offset += 1;
    let mut generation_bytes = [0_u8; std::mem::size_of::<u64>()];
    let generation_len = generation_bytes.len();
    generation_bytes.copy_from_slice(&bytes[offset..offset + generation_len]);
    let generation = u64::from_le_bytes(generation_bytes);
    offset += generation_len;
    let mut instance_id = [0_u8; 16];
    let instance_len = instance_id.len();
    instance_id.copy_from_slice(&bytes[offset..offset + instance_len]);
    offset += instance_len;
    let mut port_bytes = [0_u8; std::mem::size_of::<u16>()];
    let port_len = port_bytes.len();
    port_bytes.copy_from_slice(&bytes[offset..offset + port_len]);
    let endpoint_port = u16::from_le_bytes(port_bytes);
    offset += port_len;
    let mut certificate_sha256 = [0_u8; 32];
    let certificate_len = certificate_sha256.len();
    certificate_sha256.copy_from_slice(&bytes[offset..offset + certificate_len]);
    offset += certificate_len;
    let mut pid_bytes = [0_u8; std::mem::size_of::<u32>()];
    let pid_len = pid_bytes.len();
    pid_bytes.copy_from_slice(&bytes[offset..offset + pid_len]);
    let pid = NonZeroU32::new(u32::from_le_bytes(pid_bytes))
        .ok_or(ForgeCredentialError::ReconnectRecordMalformed)?;
    offset += pid_len;
    let mut owner_nonce = Zeroizing::new([0_u8; 16]);
    owner_nonce[..].copy_from_slice(&bytes[offset..offset + 16]);
    offset += 16;
    let capability_bytes = &bytes[offset..offset + artisan_protocol::RECONNECT_CAPABILITY_BYTES];
    let binding = ReconnectBinding {
        instance_id,
        endpoint_port,
        certificate_sha256,
        pid,
    };
    if binding.validate().is_err() || generation == 0 {
        return Err(ForgeCredentialError::ReconnectRecordMalformed);
    }
    let nonce_is_zero = bytes_are_zero(owner_nonce.as_ref());
    let capability_is_zero = bytes_are_zero(capability_bytes);
    let valid = match state {
        ReconnectCapabilityState::Ready => !capability_is_zero && nonce_is_zero,
        ReconnectCapabilityState::InFlight => capability_is_zero && !nonce_is_zero,
        ReconnectCapabilityState::Lost => capability_is_zero && nonce_is_zero,
    };
    if !valid {
        return Err(ForgeCredentialError::ReconnectRecordMalformed);
    }
    Ok(ReconnectRecordMetadata {
        state,
        generation,
        binding,
        owner_nonce,
    })
}

fn decode_reconnect_record(bytes: &[u8]) -> Result<ReconnectRecord, ForgeCredentialError> {
    let metadata = decode_reconnect_record_metadata(bytes)?;
    let capability_offset = RECONNECT_RECORD_BYTES - artisan_protocol::RECONNECT_CAPABILITY_BYTES;
    let mut capability = Zeroizing::new([0_u8; artisan_protocol::RECONNECT_CAPABILITY_BYTES]);
    capability[..].copy_from_slice(&bytes[capability_offset..]);
    let record = ReconnectRecord {
        state: metadata.state,
        generation: metadata.generation,
        binding: metadata.binding,
        owner_nonce: metadata.owner_nonce,
        capability,
    };
    validate_reconnect_record(&record)?;
    Ok(record)
}

#[derive(Clone)]
pub struct ReconnectCapabilityStore {
    paths: ForgeCredentialPaths,
}

impl ReconnectCapabilityStore {
    /// Opens the reconnect store facade without creating credential material.
    pub fn new(home: &Path) -> Result<Self, ForgeCredentialError> {
        validate_home(home)?;
        check_ancestors_all(home, true)?;
        Ok(Self {
            paths: ForgeCredentialPaths::new(home)?,
        })
    }

    /// Opens the reconnect store from an existing Artisan home.
    pub fn from_home(home: &Path) -> Result<Self, ForgeCredentialError> {
        Self::new(home)
    }

    /// Returns the validated credential paths used by this store.
    #[must_use]
    pub fn paths(&self) -> &ForgeCredentialPaths {
        &self.paths
    }

    /// Checks out the ready capability and keeps the exclusive lease in flight.
    pub fn checkout(
        &self,
        binding: ReconnectBinding,
        timeout: Duration,
    ) -> Result<ReconnectAttempt, ForgeCredentialError> {
        binding.validate()?;
        self.validate_directory()?;
        let lock = acquire_lock_with_timeout(&self.paths.reconnect_lock_path(), timeout)?;
        let current = read_reconnect_record(&self.paths)?;
        if current.record.binding != binding {
            return Err(ForgeCredentialError::ReconnectBindingMismatch);
        }
        if current.record.state != ReconnectCapabilityState::Ready {
            return Err(ForgeCredentialError::ReconnectCapabilityUnavailable);
        }
        let credential =
            artisan_protocol::ReconnectCapability::from_bytes(*current.record.capability);
        let owner_nonce = random_owner_nonce()?;
        let desired = ReconnectRecord::in_flight(binding, current.record.generation, &owner_nonce);
        let record_file_id = replace_reconnect_record(
            &self.paths,
            current.file_id,
            ReconnectCapabilityState::Ready,
            current.record.generation,
            &[0_u8; 16],
            binding,
            &desired,
        )?;
        Ok(ReconnectAttempt {
            store: self.clone(),
            lock: Some(lock),
            binding,
            generation: current.record.generation,
            owner_nonce,
            record_file_id,
            credential: Some(credential),
        })
    }

    /// Creates generation one only for an absent record, under the store lock.
    ///
    /// This is the owner-side bootstrap for the rotated-capability store. It
    /// never provisions or reads the one-shot bootstrap capability.
    pub fn initialize_owner_only(
        &self,
        binding: ReconnectBinding,
        next: artisan_protocol::ReconnectCapability,
        timeout: Duration,
    ) -> Result<(), ForgeCredentialError> {
        binding.validate()?;
        self.validate_directory()?;
        let _lock = acquire_lock_with_timeout(&self.paths.reconnect_lock_path(), timeout)?;
        self.initialize_owner_only_locked(binding, next)
    }

    fn initialize_owner_only_locked(
        &self,
        binding: ReconnectBinding,
        next: artisan_protocol::ReconnectCapability,
    ) -> Result<(), ForgeCredentialError> {
        let record_path = self.paths.reconnect_capability_path();
        if reconnect_record_presence(&record_path)?.is_some() {
            return Err(ForgeCredentialError::ReconnectRecordExists);
        }
        let record = ReconnectRecord::ready(binding, 1, next)?;
        let encoded = encode_reconnect_record(&record)?;
        let mut created = Vec::new();
        let result = install_atomic(
            &self.paths.credentials_dir(),
            RECONNECT_CAPABILITY_FILENAME,
            encoded.as_ref(),
            &mut created,
        );
        if let Err(error) = result {
            cleanup_created(created);
            return Err(error);
        }
        Ok(())
    }

    fn validate_directory(&self) -> Result<(), ForgeCredentialError> {
        validate_private_directory(&self.paths.credentials_dir())
    }

    /// Initializes the owner lease after an authenticated welcome.
    ///
    /// The returned lease retains the exclusive reconnect lock for the whole
    /// native session. An existing record is never replaced for the same
    /// binding. A different binding is fenced by the existing record file
    /// identity and generation, while its prior capability is discarded
    /// without being materialized as a reconnect credential.
    pub fn initialize_owner_lease(
        &self,
        binding: ReconnectBinding,
        capability: artisan_protocol::ReconnectCapability,
        timeout: Duration,
    ) -> Result<ReconnectSessionLease, ForgeCredentialError> {
        binding.validate()?;
        self.validate_directory()?;
        let lock = acquire_lock_with_timeout(&self.paths.reconnect_lock_path(), timeout)?;
        let (generation, record_file_id) = match read_reconnect_record_metadata(&self.paths) {
            Ok(current) => {
                if current.metadata.binding == binding {
                    return Err(ForgeCredentialError::ReconnectRecordExists);
                }
                let generation = current
                    .metadata
                    .generation
                    .checked_add(1)
                    .ok_or(ForgeCredentialError::ReconnectGenerationOverflow)?;
                let desired = ReconnectRecord::ready(binding, generation, capability)?;
                let record_file_id = replace_reconnect_record_for_rebind(
                    &self.paths,
                    current.file_id,
                    current.metadata.state,
                    current.metadata.generation,
                    &current.metadata.owner_nonce,
                    current.metadata.binding,
                    &desired,
                )?;
                (generation, record_file_id)
            }
            Err(ForgeCredentialError::ReconnectRecordMissing) => {
                let record = ReconnectRecord::ready(binding, 1, capability)?;
                let encoded = encode_reconnect_record(&record)?;
                let mut created = Vec::new();
                let result = install_atomic(
                    &self.paths.credentials_dir(),
                    RECONNECT_CAPABILITY_FILENAME,
                    encoded.as_ref(),
                    &mut created,
                );
                if let Err(error) = result {
                    cleanup_created(created);
                    return Err(error);
                }
                let record_file_id = created
                    .first()
                    .map(|file| file.id)
                    .ok_or(ForgeCredentialError::Provisioning)?;
                (1, record_file_id)
            }
            Err(error) => return Err(error),
        };
        Ok(ReconnectSessionLease {
            store: self.clone(),
            lock: Some(lock),
            binding,
            generation,
            record_file_id,
        })
    }
}

pub struct ReconnectSessionLease {
    store: ReconnectCapabilityStore,
    lock: Option<File>,
    binding: ReconnectBinding,
    generation: u64,
    record_file_id: FileId,
}

impl ReconnectSessionLease {
    /// Begins a reconnect attempt while retaining the whole-session lease.
    pub fn begin_reconnect(mut self) -> Result<ReconnectAttempt, ForgeCredentialError> {
        if self.lock.is_none() {
            return Err(ForgeCredentialError::ReconnectAttemptComplete);
        }
        let current = read_reconnect_record(&self.store.paths)?;
        if current.file_id != self.record_file_id {
            return Err(ForgeCredentialError::ReconnectStaleWriter);
        }
        if current.record.binding != self.binding {
            return Err(ForgeCredentialError::ReconnectBindingMismatch);
        }
        if current.record.state != ReconnectCapabilityState::Ready
            || current.record.generation != self.generation
            || !bytes_are_zero(current.record.owner_nonce.as_ref())
        {
            return Err(ForgeCredentialError::ReconnectStaleWriter);
        }
        let credential =
            artisan_protocol::ReconnectCapability::from_bytes(*current.record.capability);
        let owner_nonce = random_owner_nonce()?;
        let desired = ReconnectRecord::in_flight(self.binding, self.generation, &owner_nonce);
        let record_file_id = replace_reconnect_record(
            &self.store.paths,
            current.file_id,
            ReconnectCapabilityState::Ready,
            self.generation,
            &[0_u8; 16],
            self.binding,
            &desired,
        )?;
        let Some(lock) = self.lock.take() else {
            return Err(ForgeCredentialError::ReconnectAttemptComplete);
        };
        Ok(ReconnectAttempt {
            store: self.store.clone(),
            lock: Some(lock),
            binding: self.binding,
            generation: self.generation,
            owner_nonce,
            record_file_id,
            credential: Some(credential),
        })
    }

    /// Quarantines the ready capability and releases the owner lock.
    pub fn quarantine(self) -> Result<(), ForgeCredentialError> {
        self.quarantine_for_shutdown().map(|_| ())
    }

    /// Quarantines the ready capability while retaining the owner lock until
    /// the caller has completed the dependent Forge shutdown step.
    pub fn quarantine_for_shutdown(mut self) -> Result<Self, ForgeCredentialError> {
        if self.lock.is_none() {
            return Err(ForgeCredentialError::ReconnectAttemptComplete);
        }
        let current = read_reconnect_record_metadata(&self.store.paths)?;
        if current.file_id != self.record_file_id {
            return Err(ForgeCredentialError::ReconnectStaleWriter);
        }
        if current.metadata.binding != self.binding {
            return Err(ForgeCredentialError::ReconnectBindingMismatch);
        }
        if current.metadata.state != ReconnectCapabilityState::Ready
            || current.metadata.generation != self.generation
            || !bytes_are_zero(current.metadata.owner_nonce.as_ref())
        {
            return Err(ForgeCredentialError::ReconnectStaleWriter);
        }
        let desired = ReconnectRecord::lost(self.binding, self.generation);
        self.record_file_id = replace_reconnect_record(
            &self.store.paths,
            current.file_id,
            ReconnectCapabilityState::Ready,
            self.generation,
            &[0_u8; 16],
            self.binding,
            &desired,
        )?;
        Ok(self)
    }

    /// Returns the generation held by this session lease.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }
}

impl Drop for ReconnectSessionLease {
    fn drop(&mut self) {
        self.lock.take();
    }
}

pub struct ReconnectAttempt {
    store: ReconnectCapabilityStore,
    lock: Option<File>,
    binding: ReconnectBinding,
    generation: u64,
    owner_nonce: Zeroizing<[u8; 16]>,
    record_file_id: FileId,
    credential: Option<artisan_protocol::ReconnectCapability>,
}

impl ReconnectAttempt {
    /// Takes the single-use reconnect capability out of this attempt.
    pub fn take_credential(
        &mut self,
    ) -> Result<artisan_protocol::ReconnectCapability, ForgeCredentialError> {
        if self.lock.is_none() {
            return Err(ForgeCredentialError::ReconnectAttemptComplete);
        }
        self.credential
            .take()
            .ok_or(ForgeCredentialError::ReconnectCapabilityUnavailable)
    }

    /// Restores a capability after a failure known to precede the handshake.
    pub fn restore_before_handshake(
        mut self,
        credential: artisan_protocol::ReconnectCapability,
    ) -> Result<ReconnectSessionLease, ForgeCredentialError> {
        self.ensure_active()?;
        let desired = ReconnectRecord::ready(self.binding, self.generation, credential)?;
        let record_file_id = replace_reconnect_record(
            &self.store.paths,
            self.record_file_id,
            ReconnectCapabilityState::InFlight,
            self.generation,
            &self.owner_nonce,
            self.binding,
            &desired,
        )?;
        self.credential.take();
        let Some(lock) = self.lock.take() else {
            return Err(ForgeCredentialError::ReconnectAttemptComplete);
        };
        Ok(ReconnectSessionLease {
            store: self.store.clone(),
            lock: Some(lock),
            binding: self.binding,
            generation: self.generation,
            record_file_id,
        })
    }

    /// Publishes the rotated next capability after a successful handshake.
    pub fn publish_next(
        mut self,
        binding: ReconnectBinding,
        next: artisan_protocol::ReconnectCapability,
    ) -> Result<ReconnectSessionLease, ForgeCredentialError> {
        self.ensure_active()?;
        binding.validate()?;
        if binding != self.binding {
            return Err(ForgeCredentialError::ReconnectBindingMismatch);
        }
        let next_generation = self
            .generation
            .checked_add(1)
            .ok_or(ForgeCredentialError::ReconnectGenerationOverflow)?;
        let desired = ReconnectRecord::ready(binding, next_generation, next)?;
        let record_file_id = replace_reconnect_record(
            &self.store.paths,
            self.record_file_id,
            ReconnectCapabilityState::InFlight,
            self.generation,
            &self.owner_nonce,
            self.binding,
            &desired,
        )?;
        self.credential.take();
        let Some(lock) = self.lock.take() else {
            return Err(ForgeCredentialError::ReconnectAttemptComplete);
        };
        Ok(ReconnectSessionLease {
            store: self.store.clone(),
            lock: Some(lock),
            binding,
            generation: next_generation,
            record_file_id,
        })
    }

    /// Quarantines the in-flight capability after an ambiguous outcome.
    pub fn quarantine(mut self) -> Result<(), ForgeCredentialError> {
        self.ensure_active()?;
        quarantine_reconnect_record(
            &self.store.paths,
            self.record_file_id,
            self.generation,
            &self.owner_nonce,
            self.binding,
        )?;
        self.credential.take();
        self.lock.take();
        Ok(())
    }

    /// Returns the generation held by this attempt.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    fn ensure_active(&self) -> Result<(), ForgeCredentialError> {
        if self.lock.is_some() {
            Ok(())
        } else {
            Err(ForgeCredentialError::ReconnectAttemptComplete)
        }
    }
}

impl Drop for ReconnectAttempt {
    fn drop(&mut self) {
        if self.lock.is_none() {
            return;
        }
        let _ = quarantine_reconnect_record(
            &self.store.paths,
            self.record_file_id,
            self.generation,
            &self.owner_nonce,
            self.binding,
        );
        self.credential.take();
        self.lock.take();
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
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct FileId {
    dev: u64,
    ino: u64,
}

#[cfg(windows)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
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
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
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

    #[derive(Clone, Copy)]
    pub(super) enum PlannerClassification {
        InvalidValidatedIdentity,
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

    #[derive(Clone, Copy)]
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

fn validate_icacls_flag_tokens(
    flags_part: &str,
    output: &str,
    ace_count: usize,
) -> Result<(), ForgeCredentialError> {
    let _ = (output, ace_count);
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
            acl_diagnostic!(acl_diagnostic::record_acl(output, ace_count));
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
            acl_diagnostic!(acl_diagnostic::record_acl(output, ace_count));
            return Err(ForgeCredentialError::WindowsAcl);
        }
        idx += 1;
        has_content = true;
        if tok.is_empty() {
            acl_diagnostic!(acl_diagnostic::parser(
                acl_diagnostic::ParserClassification::EmptyToken
            ));
            acl_diagnostic!(acl_diagnostic::record_acl(output, ace_count));
            return Err(ForgeCredentialError::WindowsAcl);
        }
    }
    if !has_content {
        acl_diagnostic!(acl_diagnostic::parser(
            acl_diagnostic::ParserClassification::NoTokens
        ));
        acl_diagnostic!(acl_diagnostic::record_acl(output, ace_count));
        return Err(ForgeCredentialError::WindowsAcl);
    }
    Ok(())
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
        validate_icacls_flag_tokens(flags_part, output, ace_lines.len())?;
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
                acl_diagnostic!(acl_diagnostic::stage("DirectoryAclVerification"));
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
    Ok(())
}

fn acquire_lock(lock_path: &Path) -> Result<File, ForgeCredentialError> {
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

fn acquire_lock_with_timeout(
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
        acl_diagnostic!(acl_diagnostic::stage("BundleAclVerification"));
        let identity = resolve_current_identity()?;
        verify_windows_dacl(path, &identity.sid)?;
    }
    let permission_after_id = file_id(path)?;
    if permission_before_id != permission_after_id {
        return Err(ForgeCredentialError::UnsafePath(path.to_path_buf()));
    }
    Ok(permission_after_id)
}

fn validate_private_material(
    paths: &ForgeCredentialPaths,
    path: &Path,
) -> Result<FileId, ForgeCredentialError> {
    let credentials_dir = paths.credentials_dir();
    validate_private_directory(&credentials_dir)?;
    validate_private_file(path)
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

fn classify_manifest_length(length: usize) -> Result<(), ForgeCredentialError> {
    if length <= MAX_MANIFEST_BYTES {
        Ok(())
    } else {
        Err(ForgeCredentialError::ManifestMalformed)
    }
}

fn classify_capability_length(length: usize, path: &Path) -> Result<(), ForgeCredentialError> {
    if length == MAX_CAPABILITY_BYTES {
        Ok(())
    } else {
        Err(ForgeCredentialError::InvalidCapability {
            path: path.to_path_buf(),
        })
    }
}

fn classify_certificate_length(length: usize) -> Result<(), ForgeCredentialError> {
    if (1..=MAX_CERTIFICATE_BYTES).contains(&length) {
        Ok(())
    } else {
        Err(ForgeCredentialError::InvalidCertificate)
    }
}

fn classify_private_key_length(length: usize) -> Result<(), ForgeCredentialError> {
    if (1..=MAX_PRIVATE_KEY_BYTES).contains(&length) {
        Ok(())
    } else {
        Err(ForgeCredentialError::InvalidCertificate)
    }
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

fn private_material_identity_chain_matches<T: PartialEq>(chain: &[T; 7]) -> bool {
    chain.windows(2).all(|pair| pair[0] == pair[1])
}

struct BoundedMaterialRead {
    bytes: Zeroizing<Vec<u8>>,
    pre_id: FileId,
    opened_id: FileId,
    post_id: FileId,
}

fn open_and_read_bounded(
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

fn read_private_material(
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

fn random_owner_nonce() -> Result<Zeroizing<[u8; 16]>, ForgeCredentialError> {
    let mut nonce = Zeroizing::new([0_u8; 16]);
    getrandom::fill(&mut *nonce).map_err(|_| ForgeCredentialError::Provisioning)?;
    if bytes_are_zero(nonce.as_ref()) {
        return Err(ForgeCredentialError::Provisioning);
    }
    Ok(nonce)
}

fn reconnect_record_presence(path: &Path) -> Result<Option<FileId>, ForgeCredentialError> {
    check_ancestors_all(path, false)?;
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata_is_symlink_or_reparse(&metadata) => {
            Err(ForgeCredentialError::UnsafePath(path.to_path_buf()))
        }
        Ok(metadata) if !metadata.is_file() => {
            Err(ForgeCredentialError::UnsafePath(path.to_path_buf()))
        }
        Ok(_) => file_id(path).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(ForgeCredentialError::Io {
            context: "inspect reconnect record",
            path: path.to_path_buf(),
        }),
    }
}

struct ReconnectRecordRead {
    record: ReconnectRecord,
    file_id: FileId,
}

struct ReconnectRecordMetadataRead {
    metadata: ReconnectRecordMetadata,
    file_id: FileId,
}

fn read_reconnect_bytes(
    paths: &ForgeCredentialPaths,
) -> Result<(Zeroizing<Vec<u8>>, FileId), ForgeCredentialError> {
    let directory = paths.credentials_dir();
    validate_private_directory(&directory)?;
    let path = paths.reconnect_capability_path();
    if reconnect_record_presence(&path)?.is_none() {
        return Err(ForgeCredentialError::ReconnectRecordMissing);
    }
    let permission_before_id = validate_private_material(paths, &path)?;
    let read = open_and_read_bounded(
        &path,
        RECONNECT_RECORD_BYTES + 1,
        ForgeCredentialError::ReconnectRecordMalformed,
        permission_before_id,
    )?;
    let permission_after_id = validate_private_material(paths, &path)?;
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
        return Err(ForgeCredentialError::ReconnectStaleWriter);
    }
    Ok((read.bytes, permission_after_id))
}

fn read_reconnect_record(
    paths: &ForgeCredentialPaths,
) -> Result<ReconnectRecordRead, ForgeCredentialError> {
    let (bytes, file_id) = read_reconnect_bytes(paths)?;
    let record = decode_reconnect_record(&bytes)?;
    Ok(ReconnectRecordRead { record, file_id })
}

fn read_reconnect_record_metadata(
    paths: &ForgeCredentialPaths,
) -> Result<ReconnectRecordMetadataRead, ForgeCredentialError> {
    let (bytes, file_id) = read_reconnect_bytes(paths)?;
    let metadata = decode_reconnect_record_metadata(&bytes)?;
    Ok(ReconnectRecordMetadataRead { metadata, file_id })
}

fn revalidate_reconnect_record(
    paths: &ForgeCredentialPaths,
    expected_file_id: FileId,
    expected_state: ReconnectCapabilityState,
    expected_generation: u64,
    expected_owner_nonce: &[u8; 16],
    expected_binding: ReconnectBinding,
) -> Result<(), ForgeCredentialError> {
    let current = read_reconnect_record(paths)?;
    if current.file_id != expected_file_id {
        return Err(ForgeCredentialError::ReconnectStaleWriter);
    }
    if current.record.binding != expected_binding {
        return Err(ForgeCredentialError::ReconnectBindingMismatch);
    }
    if current.record.state != expected_state
        || current.record.generation != expected_generation
        || current.record.owner_nonce.as_ref() != expected_owner_nonce
    {
        return Err(ForgeCredentialError::ReconnectStaleWriter);
    }
    Ok(())
}

fn revalidate_reconnect_record_metadata(
    paths: &ForgeCredentialPaths,
    expected_file_id: FileId,
    expected_state: ReconnectCapabilityState,
    expected_generation: u64,
    expected_owner_nonce: &[u8; 16],
    expected_binding: ReconnectBinding,
) -> Result<(), ForgeCredentialError> {
    let current = read_reconnect_record_metadata(paths)?;
    if current.file_id != expected_file_id {
        return Err(ForgeCredentialError::ReconnectStaleWriter);
    }
    if current.metadata.binding != expected_binding {
        return Err(ForgeCredentialError::ReconnectBindingMismatch);
    }
    if current.metadata.state != expected_state
        || current.metadata.generation != expected_generation
        || current.metadata.owner_nonce.as_ref() != expected_owner_nonce
    {
        return Err(ForgeCredentialError::ReconnectStaleWriter);
    }
    Ok(())
}

fn replace_reconnect_record_for_rebind(
    paths: &ForgeCredentialPaths,
    expected_file_id: FileId,
    expected_state: ReconnectCapabilityState,
    expected_generation: u64,
    expected_owner_nonce: &[u8; 16],
    expected_binding: ReconnectBinding,
    desired: &ReconnectRecord,
) -> Result<FileId, ForgeCredentialError> {
    let encoded = encode_reconnect_record(desired)?;
    let path = paths.reconnect_capability_path();
    let result = atomic_replace_private_file(&path, encoded.as_ref(), expected_file_id, || {
        revalidate_reconnect_record_metadata(
            paths,
            expected_file_id,
            expected_state,
            expected_generation,
            expected_owner_nonce,
            expected_binding,
        )
    });
    match result {
        Ok(file_id) => Ok(file_id),
        Err(error) => {
            if desired.state == ReconnectCapabilityState::Ready {
                quarantine_ready_after_failed_replacement(
                    paths,
                    desired.generation,
                    desired.binding,
                );
            }
            Err(error)
        }
    }
}

fn replace_reconnect_record(
    paths: &ForgeCredentialPaths,
    expected_file_id: FileId,
    expected_state: ReconnectCapabilityState,
    expected_generation: u64,
    expected_owner_nonce: &[u8; 16],
    expected_binding: ReconnectBinding,
    desired: &ReconnectRecord,
) -> Result<FileId, ForgeCredentialError> {
    if desired.binding != expected_binding {
        return Err(ForgeCredentialError::ReconnectBindingMismatch);
    }
    let desired_state = desired.state;
    let desired_generation = desired.generation;
    let desired_owner_nonce = Zeroizing::new(*desired.owner_nonce);
    let encoded = encode_reconnect_record(desired)?;
    let path = paths.reconnect_capability_path();
    let result = atomic_replace_private_file(&path, encoded.as_ref(), expected_file_id, || {
        revalidate_reconnect_record(
            paths,
            expected_file_id,
            expected_state,
            expected_generation,
            expected_owner_nonce,
            expected_binding,
        )
    });
    match result {
        Ok(file_id) => Ok(file_id),
        Err(error) => {
            if desired_state == ReconnectCapabilityState::Ready {
                quarantine_ready_after_failed_replacement(
                    paths,
                    desired_generation,
                    expected_binding,
                );
            } else if desired_state == ReconnectCapabilityState::InFlight {
                quarantine_in_flight_after_failed_replacement(
                    paths,
                    desired_generation,
                    expected_binding,
                    &desired_owner_nonce,
                );
            }
            Err(error)
        }
    }
}

fn quarantine_reconnect_record(
    paths: &ForgeCredentialPaths,
    expected_file_id: FileId,
    generation: u64,
    owner_nonce: &[u8; 16],
    binding: ReconnectBinding,
) -> Result<(), ForgeCredentialError> {
    let desired = ReconnectRecord::lost(binding, generation);
    replace_reconnect_record(
        paths,
        expected_file_id,
        ReconnectCapabilityState::InFlight,
        generation,
        owner_nonce,
        binding,
        &desired,
    )
    .map(|_| ())
}

fn quarantine_ready_after_failed_replacement(
    paths: &ForgeCredentialPaths,
    generation: u64,
    binding: ReconnectBinding,
) {
    let Ok(current) = read_reconnect_record_metadata(paths) else {
        return;
    };
    if current.metadata.state != ReconnectCapabilityState::Ready
        || current.metadata.generation != generation
        || current.metadata.binding != binding
    {
        return;
    }
    let _ = replace_reconnect_record(
        paths,
        current.file_id,
        ReconnectCapabilityState::Ready,
        generation,
        &[0_u8; 16],
        binding,
        &ReconnectRecord::lost(binding, generation),
    );
}

fn quarantine_in_flight_after_failed_replacement(
    paths: &ForgeCredentialPaths,
    generation: u64,
    binding: ReconnectBinding,
    owner_nonce: &[u8; 16],
) {
    let Ok(current) = read_reconnect_record_metadata(paths) else {
        return;
    };
    if current.metadata.state != ReconnectCapabilityState::InFlight
        || current.metadata.generation != generation
        || current.metadata.binding != binding
        || current.metadata.owner_nonce.as_ref() != owner_nonce
    {
        return;
    }
    let _ = replace_reconnect_record(
        paths,
        current.file_id,
        ReconnectCapabilityState::InFlight,
        generation,
        owner_nonce,
        binding,
        &ReconnectRecord::lost(binding, generation),
    );
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

fn atomic_replace_private_file(
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
    let manifest_bytes = read_private_material(
        paths,
        manifest_path,
        MAX_MANIFEST_READ_BYTES,
        ForgeCredentialError::ManifestMalformed,
    )?;
    classify_manifest_length(manifest_bytes.len())?;
    validate_manifest_bytes(&manifest_bytes, paths)?;
    let cap_bytes = read_private_material(
        paths,
        capability_path,
        MAX_CAPABILITY_READ_BYTES,
        ForgeCredentialError::InvalidCapability {
            path: capability_path.to_path_buf(),
        },
    )?;
    classify_capability_length(cap_bytes.len(), capability_path)?;
    let cert_der = read_private_material(
        paths,
        cert_path,
        MAX_CERTIFICATE_READ_BYTES,
        ForgeCredentialError::InvalidCertificate,
    )?;
    classify_certificate_length(cert_der.len())?;
    let key_bytes = read_private_material(
        paths,
        key_path,
        MAX_PRIVATE_KEY_READ_BYTES,
        ForgeCredentialError::InvalidCertificate,
    )?;
    classify_private_key_length(key_bytes.len())?;
    validate_cert_sans(&cert_der)?;
    validate_key_matches_cert(&key_bytes, &cert_der)?;
    let _ = rustls::crypto::ring::default_provider();
    Ok(true)
}

fn validate_existing_identity_bundle(
    paths: &ForgeCredentialPaths,
) -> Result<bool, ForgeCredentialError> {
    let directory = paths.credentials_dir();
    check_ancestors_all(&directory, false)?;
    match fs::symlink_metadata(&directory) {
        Ok(_) => validate_private_directory(&directory)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(_) => {
            return Err(ForgeCredentialError::Io {
                context: "inspect credentials directory",
                path: directory,
            });
        }
    }
    let manifest_path = paths.manifest_path();
    let cert_path = &paths.certificate_paths()[0];
    let key_path = paths.private_key_path();
    let files = [manifest_path, cert_path.as_path(), key_path];
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
                    context: "inspect identity file",
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
    let manifest_bytes = read_private_material(
        paths,
        manifest_path,
        MAX_MANIFEST_READ_BYTES,
        ForgeCredentialError::ManifestMalformed,
    )?;
    classify_manifest_length(manifest_bytes.len())?;
    validate_manifest_bytes(&manifest_bytes, paths)?;
    let cert_der = read_private_material(
        paths,
        cert_path,
        MAX_CERTIFICATE_READ_BYTES,
        ForgeCredentialError::InvalidCertificate,
    )?;
    classify_certificate_length(cert_der.len())?;
    let key_bytes = read_private_material(
        paths,
        key_path,
        MAX_PRIVATE_KEY_READ_BYTES,
        ForgeCredentialError::InvalidCertificate,
    )?;
    classify_private_key_length(key_bytes.len())?;
    validate_cert_sans(&cert_der)?;
    validate_key_matches_cert(&key_bytes, &cert_der)?;
    let _ = rustls::crypto::ring::default_provider();
    Ok(true)
}

fn generate_material() -> Result<ProvisionalMaterial, ForgeCredentialError> {
    let mut cap = Zeroizing::new([0_u8; MAX_CAPABILITY_BYTES]);
    getrandom::fill(&mut *cap).map_err(|_| ForgeCredentialError::Provisioning)?;
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
        capability: cap,
        private_key: Zeroizing::new(key_der),
        certificate: cert_der,
    })
}

fn ensure_credentials_dir(dir: &Path) -> Result<(), ForgeCredentialError> {
    ensure_private_directory(dir)
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

fn local_capability_from_bytes(
    bytes: &[u8],
    path: &Path,
) -> Result<artisan_protocol::LocalCapability, ForgeCredentialError> {
    classify_capability_length(bytes.len(), path)?;
    let mut exact = Zeroizing::new([0_u8; MAX_CAPABILITY_BYTES]);
    exact[..].copy_from_slice(bytes);
    Ok(artisan_protocol::LocalCapability::from_bytes(*exact))
}

pub fn load_client_credentials(
    home: &Path,
) -> Result<NativeClientCredentials, ForgeCredentialError> {
    let paths = provision_or_load(home)?;
    let certificate_path = &paths.certificate_paths()[0];
    let certificate_bytes = read_private_material(
        &paths,
        certificate_path,
        MAX_CERTIFICATE_READ_BYTES,
        ForgeCredentialError::InvalidCertificate,
    )?;
    classify_certificate_length(certificate_bytes.len())?;
    validate_cert_sans(&certificate_bytes)?;
    let certificate = rustls_pki_types::CertificateDer::from(certificate_bytes.as_slice().to_vec());

    let capability_path = paths.capability_path();
    let capability_bytes = read_private_material(
        &paths,
        capability_path,
        MAX_CAPABILITY_READ_BYTES,
        ForgeCredentialError::InvalidCapability {
            path: capability_path.to_path_buf(),
        },
    )?;
    let capability = local_capability_from_bytes(capability_bytes.as_slice(), capability_path)?;

    Ok(NativeClientCredentials {
        paths,
        certificate,
        capability,
    })
}

/// Loads existing client identity material without provisioning or reading
/// bootstrap authority.
pub fn load_existing_client_identity(
    home: &Path,
) -> Result<NativeClientIdentity, ForgeCredentialError> {
    validate_home(home)?;
    check_ancestors_all(home, true)?;
    let paths = ForgeCredentialPaths::new(home)?;
    if !validate_existing_identity_bundle(&paths)? {
        return Err(ForgeCredentialError::IdentityBundleMissing);
    }
    let certificate_path = &paths.certificate_paths()[0];
    let certificate_bytes = read_private_material(
        &paths,
        certificate_path,
        MAX_CERTIFICATE_READ_BYTES,
        ForgeCredentialError::InvalidCertificate,
    )?;
    classify_certificate_length(certificate_bytes.len())?;
    validate_cert_sans(&certificate_bytes)?;
    let certificate = rustls_pki_types::CertificateDer::from(certificate_bytes.as_slice().to_vec());
    Ok(NativeClientIdentity { paths, certificate })
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

#[cfg(test)]
mod client_credentials_tests {
    use super::*;

    #[test]
    fn load_returns_exact_leaf_capability_and_paths() {
        let home = tempfile::tempdir().expect("temporary credential home");
        let expected_paths = provision_or_load(home.path()).expect("provision credentials");
        let expected_certificate =
            fs::read(expected_paths.certificate_paths()[0].as_path()).expect("read certificate");
        let capability_bytes = Zeroizing::new(
            fs::read(expected_paths.capability_path()).expect("read capability fixture"),
        );
        let mut expected_capability_bytes = Zeroizing::new([0_u8; MAX_CAPABILITY_BYTES]);
        expected_capability_bytes[..].copy_from_slice(capability_bytes.as_slice());
        let expected_capability =
            artisan_protocol::LocalCapability::from_bytes(*expected_capability_bytes);

        let loaded = load_client_credentials(home.path()).expect("load credentials");
        assert_eq!(loaded.paths(), &expected_paths);
        let (certificate, capability) = loaded.into_parts();
        assert_eq!(certificate.as_ref(), expected_certificate.as_slice());
        assert!(capability.constant_time_eq(&expected_capability));
    }

    #[test]
    fn capability_length_is_exact_with_zeroizing_material() {
        let path = Path::new("capability.bin");
        for (length, valid) in [(31, false), (32, true), (33, false)] {
            let material = Zeroizing::new(vec![0xa5_u8; length]);
            let result = local_capability_from_bytes(material.as_slice(), path);
            if valid {
                assert!(result.is_ok());
            } else {
                assert!(matches!(
                    result,
                    Err(ForgeCredentialError::InvalidCapability { .. })
                ));
            }
        }
    }

    #[test]
    fn certificate_length_classifier_is_bounded_without_parsing_fixture_bytes() {
        assert!(matches!(
            classify_certificate_length(0),
            Err(ForgeCredentialError::InvalidCertificate)
        ));
        assert!(classify_certificate_length(MAX_CERTIFICATE_BYTES).is_ok());
        assert!(matches!(
            classify_certificate_length(MAX_CERTIFICATE_BYTES + 1),
            Err(ForgeCredentialError::InvalidCertificate)
        ));
    }

    #[test]
    fn bundle_validation_enforces_manifest_and_private_key_bounds() {
        let manifest_home = tempfile::tempdir().expect("temporary manifest home");
        let manifest_paths =
            provision_or_load(manifest_home.path()).expect("provision manifest fixture");
        fs::write(
            manifest_paths.manifest_path(),
            vec![b'm'; MAX_MANIFEST_BYTES + 1],
        )
        .expect("write oversized manifest fixture");
        assert!(matches!(
            validate_existing_bundle(&manifest_paths),
            Err(ForgeCredentialError::ManifestMalformed)
        ));

        let key_home = tempfile::tempdir().expect("temporary key home");
        let key_paths = provision_or_load(key_home.path()).expect("provision key fixture");
        fs::write(
            key_paths.private_key_path(),
            vec![b'k'; MAX_PRIVATE_KEY_BYTES + 1],
        )
        .expect("write oversized private key fixture");
        assert!(matches!(
            validate_existing_bundle(&key_paths),
            Err(ForgeCredentialError::InvalidCertificate)
        ));
    }

    #[test]
    fn file_identity_decision_rejects_mismatch_at_every_chain_position() {
        let chain = [7_u8; 7];
        assert!(private_material_identity_chain_matches(&chain));
        for position in 0..chain.len() {
            let mut mismatch = chain;
            mismatch[position] = 6;
            assert!(
                !private_material_identity_chain_matches(&mismatch),
                "identity mismatch at chain position {position}"
            );
        }
    }

    #[test]
    fn bounded_reader_rejects_directories() {
        let root = tempfile::tempdir().expect("temporary reader home");
        let directory = root.path().join("directory");
        fs::create_dir(&directory).expect("create directory fixture");
        assert!(matches!(
            open_and_read_bounded(
                &directory,
                MAX_CERTIFICATE_READ_BYTES,
                ForgeCredentialError::InvalidCertificate,
                FileId::default(),
            ),
            Err(ForgeCredentialError::UnsafePath(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn bounded_reader_rejects_symlink_targets_and_unsafe_ancestors() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().expect("temporary reader home");
        let real_file = root.path().join("real-file");
        fs::write(&real_file, b"safe").expect("write real file");
        let symlink_file = root.path().join("symlink-file");
        symlink(&real_file, &symlink_file).expect("create symlink fixture");
        assert!(matches!(
            open_and_read_bounded(
                &symlink_file,
                MAX_CERTIFICATE_READ_BYTES,
                ForgeCredentialError::InvalidCertificate,
                FileId::default(),
            ),
            Err(ForgeCredentialError::UnsafePath(_))
        ));

        let real_directory = root.path().join("real-directory");
        fs::create_dir(&real_directory).expect("create real directory");
        let nested_file = real_directory.join("nested-file");
        fs::write(&nested_file, b"safe").expect("write nested file");
        let symlink_directory = root.path().join("symlink-directory");
        symlink(&real_directory, &symlink_directory).expect("create ancestor symlink");
        let substituted_path = symlink_directory.join("nested-file");
        assert!(matches!(
            open_and_read_bounded(
                &substituted_path,
                MAX_CERTIFICATE_READ_BYTES,
                ForgeCredentialError::InvalidCertificate,
                FileId::default(),
            ),
            Err(ForgeCredentialError::UnsafePath(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn existing_private_modes_fail_closed_without_repair() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().expect("temporary mode home");
        let paths = ForgeCredentialPaths::new(root.path()).expect("temporary mode paths");
        let credentials = paths.credentials_dir();
        fs::create_dir(&credentials).expect("create credentials directory");
        fs::set_permissions(&credentials, fs::Permissions::from_mode(0o755))
            .expect("set unsafe directory mode");
        assert!(matches!(
            validate_private_directory(&credentials),
            Err(ForgeCredentialError::WindowsAcl)
        ));
        assert_eq!(
            fs::symlink_metadata(&credentials)
                .expect("inspect directory mode")
                .permissions()
                .mode()
                & 0o777,
            0o755
        );

        fs::set_permissions(&credentials, fs::Permissions::from_mode(0o700))
            .expect("restore directory mode");
        let file = paths.manifest_path().to_path_buf();
        fs::write(&file, b"material").expect("write material");
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644))
            .expect("set unsafe file mode");
        assert!(matches!(
            read_private_material(
                &paths,
                &file,
                MAX_MANIFEST_READ_BYTES,
                ForgeCredentialError::ManifestMalformed,
            ),
            Err(ForgeCredentialError::WindowsAcl)
        ));
        assert_eq!(
            fs::symlink_metadata(&file)
                .expect("inspect file mode")
                .permissions()
                .mode()
                & 0o777,
            0o644
        );
    }

    #[test]
    fn credential_material_errors_redact_all_injected_canaries() {
        const CAPABILITY_CANARY: &[u8] = b"bootstrap-capability-bytes-canary";
        const CERTIFICATE_CANARY: &[u8] = b"certificate-bytes-canary";
        const KEY_CANARY: &[u8] = b"private-key-bytes-canary";
        let Err(capability_error) = local_capability_from_bytes(
            Zeroizing::new(CAPABILITY_CANARY.to_vec()).as_slice(),
            Path::new("capability.bin"),
        ) else {
            panic!("canary capability must fail length validation");
        };
        let certificate_error =
            validate_cert_sans(CERTIFICATE_CANARY).expect_err("canary certificate must fail");
        let key_error = validate_key_matches_cert(KEY_CANARY, CERTIFICATE_CANARY)
            .expect_err("canary key must fail parsing");

        for error in [capability_error, certificate_error, key_error] {
            let display = error.to_string();
            let debug = format!("{error:?}");
            for canary in [
                "bootstrap-capability-bytes-canary",
                "certificate-bytes-canary",
                "private-key-bytes-canary",
            ] {
                assert!(!display.contains(canary));
                assert!(!debug.contains(canary));
            }
        }
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
        assert!(
            classification_values.contains(&expected),
            "ACL diagnostic classification mismatch: kind={kind}, expected={expected}, actual={classification_values:?}"
        );
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
            collect_icacls_ace_lines(&format!("{path} ({sid}:"), path)
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
