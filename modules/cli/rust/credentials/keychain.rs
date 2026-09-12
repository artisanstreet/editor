use std::{
    fs::{self, File},
    num::NonZeroU32,
    path::Path,
    time::Duration,
};

use zeroize::Zeroizing;

use super::ForgeCredentialError;
use super::ForgeCredentialPaths;
use super::storage::{
    FileId, acquire_lock_with_timeout, atomic_replace_private_file, check_ancestors_all,
    cleanup_created, file_id, install_atomic, metadata_is_symlink_or_reparse,
    open_and_read_bounded, private_material_identity_chain_matches, validate_home,
    validate_private_directory, validate_private_material,
};

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

pub(super) const RECONNECT_CAPABILITY_FILENAME: &str = "reconnect-capability.bin";

pub(super) const RECONNECT_LOCK_FILENAME: &str = ".reconnect-capability.lock";

/// Maximum time spent waiting for exclusive reconnect-capability ownership.
pub const RECONNECT_LOCK_TIMEOUT: Duration = Duration::from_millis(250);

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
