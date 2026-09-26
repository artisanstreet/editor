//! Bounded install-state codec, validation, and error classification.
//!
//! `state.json` records the active generation, up to
//! [`MAX_PREVIOUS_GENERATIONS`] previously active generations (most recent
//! first, the rollback targets), and an optional pending generation that is
//! installed and verified but waits for the engine to become idle. Every
//! generation records the executable path, size, and SHA-256 measured at
//! install time after the download matched the vendor digest; resolution
//! verifies against those values.
//!
//! Format 1 (the original single-engine `OpenCode2` record) is still read and
//! is rewritten as format 2 on the next publication.

use std::{
    fmt,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Deserializer, Serialize};

use crate::io::NativeFileError;

use super::{
    catalog::{Distribution, HostPlatform, ManagedEngine},
    version::EngineVersion,
};

pub(crate) const MAX_STATE_BYTES: usize = 16 * 1024;
pub(crate) const MAX_GENERATION_ID_BYTES: usize = 128;
pub(crate) const MAX_BINARY_PATH_BYTES: usize = 256;
/// Previously active generations kept for rollback.
pub const MAX_PREVIOUS_GENERATIONS: usize = 3;
const CURRENT_FORMAT: u32 = 2;

/// Bounded, path-free failures from managed engine inspection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagedEngineError {
    UnsupportedPlatform,
    StateMissing,
    StateTooLarge,
    StateMalformed,
    StateUnsupportedVersion,
    ActiveGenerationUntrusted,
    UnsafePath,
    ExecutableUnavailable,
    ExecutableChanged,
    ExecutableSizeMismatch,
    ExecutableHashMismatch,
    Io,
}

impl ManagedEngineError {
    /// Returns the stable CLI classification for this failure.
    #[must_use]
    pub const fn cli_reason(self) -> &'static str {
        match self {
            Self::UnsupportedPlatform => "unsupported_platform",
            Self::StateMissing => "not_installed",
            Self::StateTooLarge | Self::StateMalformed | Self::StateUnsupportedVersion => {
                "state_invalid"
            }
            Self::ActiveGenerationUntrusted => "generation_untrusted",
            Self::UnsafePath => "unsafe_path",
            Self::ExecutableUnavailable => "executable_unavailable",
            Self::ExecutableChanged => "executable_changed",
            Self::ExecutableSizeMismatch => "executable_size_mismatch",
            Self::ExecutableHashMismatch => "executable_hash_mismatch",
            Self::Io => "io",
        }
    }
}

impl fmt::Display for ManagedEngineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::UnsupportedPlatform => "managed engine is unsupported on this platform",
            Self::StateMissing => "managed engine is not installed",
            Self::StateTooLarge => "managed engine state exceeds its bound",
            Self::StateMalformed => "managed engine state is malformed",
            Self::StateUnsupportedVersion => "managed engine state version is unsupported",
            Self::ActiveGenerationUntrusted => "managed engine active generation is untrusted",
            Self::UnsafePath => "managed engine path is unsafe",
            Self::ExecutableUnavailable => "managed engine executable is unavailable",
            Self::ExecutableChanged => "managed engine executable changed during verification",
            Self::ExecutableSizeMismatch => "managed engine executable size does not match",
            Self::ExecutableHashMismatch => "managed engine executable hash does not match",
            Self::Io => "managed engine authority I/O failed",
        })
    }
}

impl std::error::Error for ManagedEngineError {}

/// Bounded, path-free failures from install-state codec operations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagedStateError {
    InvalidRoot,
    TooLarge,
    Malformed,
    UnsupportedVersion,
    ActiveGenerationUntrusted,
    UnsafePath,
    Io,
    Encode,
    AtomicPublishFailed,
}

impl fmt::Display for ManagedStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidRoot => "managed engine state root is invalid",
            Self::TooLarge => "managed engine state exceeds its bound",
            Self::Malformed => "managed engine state is malformed",
            Self::UnsupportedVersion => "managed engine state version is unsupported",
            Self::ActiveGenerationUntrusted => "managed engine state has an untrusted generation",
            Self::UnsafePath => "managed engine state path is unsafe",
            Self::Io => "managed engine state I/O failed",
            Self::Encode => "managed engine state encoding failed",
            Self::AtomicPublishFailed => "managed engine state publication failed",
        })
    }
}

impl std::error::Error for ManagedStateError {}

/// One installed generation as recorded in `state.json`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedGeneration {
    /// Executable path relative to the generation directory.
    pub binary: String,
    /// Generation directory name below `versions/`.
    pub directory: String,
    /// Lowercase hexadecimal SHA-256 of the executable.
    pub sha256: String,
    /// Executable size in bytes (absent in format 1 records).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// Engine release version.
    pub version: String,
}

impl ManagedGeneration {
    /// Returns the parsed release version.
    #[must_use]
    pub fn parsed_version(&self) -> Option<EngineVersion> {
        EngineVersion::parse(&self.version)
    }
}

/// The decoded install state.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ManagedToolchainState {
    pub active: ManagedGeneration,
    pub format_version: u32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub previous: Vec<ManagedGeneration>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending: Option<ManagedGeneration>,
}

impl ManagedToolchainState {
    /// A state whose only generation is `active`.
    #[must_use]
    pub fn new(active: ManagedGeneration) -> Self {
        Self {
            active,
            format_version: CURRENT_FORMAT,
            previous: Vec::new(),
            pending: None,
        }
    }

    /// Returns the state after activating `generation`: the old active
    /// generation becomes the most recent rollback target, duplicates are
    /// dropped, and at most [`MAX_PREVIOUS_GENERATIONS`] remain.
    #[must_use]
    pub fn activated(&self, generation: ManagedGeneration) -> Self {
        let previous = std::iter::once(self.active.clone())
            .chain(self.previous.iter().cloned())
            .filter(|candidate| candidate.directory != generation.directory)
            .take(MAX_PREVIOUS_GENERATIONS)
            .collect();
        let pending = self
            .pending
            .clone()
            .filter(|pending| pending.directory != generation.directory);
        Self {
            active: generation,
            format_version: CURRENT_FORMAT,
            previous,
            pending,
        }
    }

    /// Returns the state with `generation` waiting for activation.
    #[must_use]
    pub fn with_pending(&self, generation: ManagedGeneration) -> Self {
        Self {
            pending: Some(generation),
            format_version: CURRENT_FORMAT,
            ..self.clone()
        }
    }

    /// Returns every generation directory the state references.
    pub fn directories(&self) -> impl Iterator<Item = &str> {
        std::iter::once(&self.active)
            .chain(&self.previous)
            .chain(&self.pending)
            .map(|generation| generation.directory.as_str())
    }

    /// Returns the retained generation for an exact version, preferring the
    /// active one, then pending, then the most recent previous.
    #[must_use]
    pub fn generation_for(&self, version: &str) -> Option<&ManagedGeneration> {
        std::iter::once(&self.active)
            .chain(&self.pending)
            .chain(&self.previous)
            .find(|generation| generation.version == version)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StateV2 {
    active: ManagedGeneration,
    format_version: u32,
    #[serde(default)]
    previous: Vec<ManagedGeneration>,
    #[serde(default, deserialize_with = "present_object")]
    pending: Option<ManagedGeneration>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StateV1 {
    active: ManagedGeneration,
    format_version: u32,
    #[serde(default, deserialize_with = "present_object")]
    previous: Option<ManagedGeneration>,
}

/// `null` is rejected: an optional generation is either absent or an object.
fn present_object<'de, D>(deserializer: D) -> Result<Option<ManagedGeneration>, D::Error>
where
    D: Deserializer<'de>,
{
    ManagedGeneration::deserialize(deserializer).map(Some)
}

pub(crate) fn state_path_for_root(
    engine_root: &Path,
    engine: ManagedEngine,
) -> Result<PathBuf, ManagedStateError> {
    engine_file(engine_root, engine, "state.json")
}

pub(crate) fn engine_file(
    engine_root: &Path,
    engine: ManagedEngine,
    name: &str,
) -> Result<PathBuf, ManagedStateError> {
    let expected_suffix = Path::new("toolchain").join(engine.id());
    if !engine_root.is_absolute()
        || engine_root
            .components()
            .any(|component| matches!(component, Component::ParentDir))
        || !engine_root.ends_with(expected_suffix)
    {
        return Err(ManagedStateError::InvalidRoot);
    }
    Ok(engine_root.join(name))
}

/// Strictly decodes format 1 or 2, rejecting unknown, duplicate, and
/// trailing content.
pub(crate) fn decode_state(bytes: &[u8]) -> Result<ManagedToolchainState, ManagedEngineError> {
    if bytes.len() > MAX_STATE_BYTES {
        return Err(ManagedEngineError::StateTooLarge);
    }
    if let Ok(state) = serde_json::from_slice::<StateV2>(bytes)
        && state.format_version != 1
    {
        return Ok(ManagedToolchainState {
            active: state.active,
            format_version: state.format_version,
            previous: state.previous,
            pending: state.pending,
        });
    }
    let state =
        serde_json::from_slice::<StateV1>(bytes).map_err(|_| ManagedEngineError::StateMalformed)?;
    if state.format_version != 1 {
        return Err(ManagedEngineError::StateMalformed);
    }
    Ok(ManagedToolchainState {
        active: state.active,
        format_version: state.format_version,
        previous: state.previous.into_iter().collect(),
        pending: None,
    })
}

pub(crate) fn validate_install_state(
    state: &ManagedToolchainState,
    engine: ManagedEngine,
    platform: HostPlatform,
) -> Result<(), ManagedStateError> {
    validate_state_version(state).map_err(map_state_validation_error)?;
    validate_generation(&state.active, true, engine, platform)
        .map_err(map_state_validation_error)?;
    if let Some(pending) = &state.pending {
        validate_generation(pending, true, engine, platform).map_err(map_state_validation_error)?;
    }
    if state.previous.len() > MAX_PREVIOUS_GENERATIONS {
        return Err(ManagedStateError::Malformed);
    }
    for previous in &state.previous {
        validate_generation(previous, false, engine, platform)
            .map_err(map_state_validation_error)?;
    }
    let mut directories = state.directories().collect::<Vec<_>>();
    let referenced = directories.len();
    directories.sort_unstable();
    directories.dedup();
    if directories.len() != referenced {
        return Err(ManagedStateError::Malformed);
    }
    Ok(())
}

pub(crate) fn validate_state_version(
    state: &ManagedToolchainState,
) -> Result<(), ManagedEngineError> {
    if !matches!(state.format_version, 1 | CURRENT_FORMAT) {
        return Err(ManagedEngineError::StateUnsupportedVersion);
    }
    Ok(())
}

/// Validates one generation record. Launchable generations (active or
/// pending) must also meet the engine floor.
pub(crate) fn validate_generation(
    generation: &ManagedGeneration,
    launchable: bool,
    engine: ManagedEngine,
    platform: HostPlatform,
) -> Result<(), ManagedEngineError> {
    if !is_safe_basename(&generation.directory, MAX_GENERATION_ID_BYTES)
        || !is_safe_relative_path(&generation.binary, MAX_BINARY_PATH_BYTES)
    {
        return Err(ManagedEngineError::UnsafePath);
    }
    let Some(version) = generation.parsed_version() else {
        return Err(ManagedEngineError::ActiveGenerationUntrusted);
    };
    if !is_safe_sha256(&generation.sha256) || generation.size == Some(0) {
        return Err(ManagedEngineError::ActiveGenerationUntrusted);
    }
    let Distribution::Supported(plan) = engine.distribution(platform) else {
        return Err(ManagedEngineError::UnsupportedPlatform);
    };
    if generation.binary != plan.layout.entry() {
        return Err(ManagedEngineError::ActiveGenerationUntrusted);
    }
    if launchable && !engine.meets_floor(&version) {
        return Err(ManagedEngineError::ActiveGenerationUntrusted);
    }
    Ok(())
}

pub(crate) fn is_safe_basename(value: &str, maximum_bytes: usize) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= maximum_bytes
        && bytes[0].is_ascii_alphanumeric()
        && bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'.' | b'_' | b'-'))
}

pub(crate) fn is_safe_relative_path(value: &str, maximum_bytes: usize) -> bool {
    let bytes = value.as_bytes();
    if bytes.is_empty()
        || bytes.len() > maximum_bytes
        || !bytes[0].is_ascii_alphanumeric()
        || bytes.iter().any(|byte| {
            !byte.is_ascii_alphanumeric() && !matches!(*byte, b'.' | b'_' | b'-' | b'/' | b'\\')
        })
    {
        return false;
    }
    let mut segment_start = 0;
    for (index, byte) in bytes.iter().enumerate() {
        if matches!(*byte, b'/' | b'\\') {
            if index == segment_start || &bytes[segment_start..index] == b".." {
                return false;
            }
            segment_start = index + 1;
        }
    }
    segment_start < bytes.len() && &bytes[segment_start..] != b".."
}

pub(crate) fn is_safe_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

pub(crate) fn map_state_file_error(error: NativeFileError) -> ManagedStateError {
    match error {
        NativeFileError::TooLarge => ManagedStateError::TooLarge,
        NativeFileError::UnsafePath | NativeFileError::PrivatePermissions => {
            ManagedStateError::UnsafePath
        }
        NativeFileError::NotFound
        | NativeFileError::FileChanged
        | NativeFileError::FileSizeMismatch
        | NativeFileError::FileHashMismatch
        | NativeFileError::Io => ManagedStateError::Io,
    }
}

pub(crate) fn map_state_decoder_error(error: ManagedEngineError) -> ManagedStateError {
    match error {
        ManagedEngineError::StateTooLarge => ManagedStateError::TooLarge,
        ManagedEngineError::StateMalformed => ManagedStateError::Malformed,
        other => map_state_validation_error(other),
    }
}

pub(crate) fn map_state_validation_error(error: ManagedEngineError) -> ManagedStateError {
    match error {
        ManagedEngineError::StateUnsupportedVersion => ManagedStateError::UnsupportedVersion,
        ManagedEngineError::UnsafePath => ManagedStateError::UnsafePath,
        ManagedEngineError::ActiveGenerationUntrusted | ManagedEngineError::UnsupportedPlatform => {
            ManagedStateError::ActiveGenerationUntrusted
        }
        ManagedEngineError::StateTooLarge => ManagedStateError::TooLarge,
        ManagedEngineError::StateMalformed
        | ManagedEngineError::StateMissing
        | ManagedEngineError::ExecutableUnavailable
        | ManagedEngineError::ExecutableChanged
        | ManagedEngineError::ExecutableSizeMismatch
        | ManagedEngineError::ExecutableHashMismatch
        | ManagedEngineError::Io => ManagedStateError::Malformed,
    }
}

pub(crate) fn map_state_seam_error(error: ManagedStateError) -> ManagedEngineError {
    match error {
        ManagedStateError::InvalidRoot | ManagedStateError::UnsafePath => {
            ManagedEngineError::UnsafePath
        }
        ManagedStateError::TooLarge => ManagedEngineError::StateTooLarge,
        ManagedStateError::Malformed => ManagedEngineError::StateMalformed,
        ManagedStateError::UnsupportedVersion => ManagedEngineError::StateUnsupportedVersion,
        ManagedStateError::ActiveGenerationUntrusted => {
            ManagedEngineError::ActiveGenerationUntrusted
        }
        ManagedStateError::Io
        | ManagedStateError::Encode
        | ManagedStateError::AtomicPublishFailed => ManagedEngineError::Io,
    }
}

pub(crate) fn map_state_replace_error(error: NativeFileError) -> ManagedStateError {
    match error {
        NativeFileError::UnsafePath | NativeFileError::PrivatePermissions => {
            ManagedStateError::UnsafePath
        }
        NativeFileError::NotFound
        | NativeFileError::TooLarge
        | NativeFileError::FileChanged
        | NativeFileError::FileSizeMismatch
        | NativeFileError::FileHashMismatch
        | NativeFileError::Io => ManagedStateError::AtomicPublishFailed,
    }
}
