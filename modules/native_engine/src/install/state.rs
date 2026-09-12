//! Bounded install-state codec, validation, and atomic publication.
//!
//! Owns the installed-generation document, its strict decoder, every certified
//! safety check applied to decoded values, the state-file location rule, and
//! the atomic replacement seam used to publish new state. Failures map into
//! path-free classifications shared with the authority façade.

use std::{
    fmt,
    path::{Component, Path, PathBuf},
};

use serde::{
    Deserialize, Serialize,
    de::{self, Deserializer, MapAccess, Visitor},
};

use crate::io::NativeFileError;

use super::spec::NativeOpenCode2InstallSpec;

pub(crate) const MAX_STATE_BYTES: usize = 16 * 1024;
pub(crate) const MAX_GENERATION_ID_BYTES: usize = 128;
pub(crate) const MAX_BINARY_PATH_BYTES: usize = 256;
const MAX_VERSION_BYTES: usize = 128;

/// Bounded, path-free failures from certified `OpenCode2` inspection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeOpenCode2Error {
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

impl NativeOpenCode2Error {
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

impl fmt::Display for NativeOpenCode2Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::UnsupportedPlatform => "OpenCode2 is unsupported on this platform",
            Self::StateMissing => "OpenCode2 managed state is missing",
            Self::StateTooLarge => "OpenCode2 managed state exceeds its bound",
            Self::StateMalformed => "OpenCode2 managed state is malformed",
            Self::StateUnsupportedVersion => "OpenCode2 managed state version is unsupported",
            Self::ActiveGenerationUntrusted => "OpenCode2 active generation is untrusted",
            Self::UnsafePath => "OpenCode2 managed path is unsafe",
            Self::ExecutableUnavailable => "OpenCode2 executable is unavailable",
            Self::ExecutableChanged => "OpenCode2 executable changed during verification",
            Self::ExecutableSizeMismatch => "OpenCode2 executable size does not match",
            Self::ExecutableHashMismatch => "OpenCode2 executable hash does not match",
            Self::Io => "OpenCode2 authority I/O failed",
        })
    }
}

impl std::error::Error for NativeOpenCode2Error {}

/// Bounded, path-free failures from certified install-state codec operations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeOpenCode2StateError {
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

impl fmt::Display for NativeOpenCode2StateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidRoot => "OpenCode2 install state root is invalid",
            Self::TooLarge => "OpenCode2 install state exceeds its bound",
            Self::Malformed => "OpenCode2 install state is malformed",
            Self::UnsupportedVersion => "OpenCode2 install state version is unsupported",
            Self::ActiveGenerationUntrusted => {
                "OpenCode2 install state has an untrusted active generation"
            }
            Self::UnsafePath => "OpenCode2 install state path is unsafe",
            Self::Io => "OpenCode2 install state I/O failed",
            Self::Encode => "OpenCode2 install state encoding failed",
            Self::AtomicPublishFailed => "OpenCode2 install state publication failed",
        })
    }
}

impl std::error::Error for NativeOpenCode2StateError {}

/// A validated, non-serializable view of the managed `OpenCode2` install state.
#[must_use = "retain validated install state for the operation it authorizes"]
pub struct NativeOpenCode2State {
    pub(crate) inner: ManagedToolchainStateV1,
}

impl fmt::Debug for NativeOpenCode2State {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NativeOpenCode2State")
            .finish_non_exhaustive()
    }
}

const STATE_FIELDS: &[&str] = &["active", "format_version", "previous"];
const GENERATION_FIELDS: &[&str] = &["binary", "directory", "sha256", "version"];

#[derive(Clone, Serialize)]
pub(crate) struct ManagedToolchainStateV1 {
    pub(crate) active: ManagedGenerationV1,
    pub(crate) format_version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) previous: Option<ManagedGenerationV1>,
}

#[derive(Clone, Serialize)]
pub(crate) struct ManagedGenerationV1 {
    pub(crate) binary: String,
    pub(crate) directory: String,
    pub(crate) sha256: String,
    pub(crate) version: String,
}

impl<'de> Deserialize<'de> for ManagedGenerationV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct GenerationVisitor;

        impl<'de> Visitor<'de> for GenerationVisitor {
            type Value = ManagedGenerationV1;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an OpenCode2 generation object")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut binary = None;
                let mut directory = None;
                let mut sha256 = None;
                let mut version = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "binary" => {
                            if binary.is_some() {
                                return Err(de::Error::duplicate_field("binary"));
                            }
                            binary = Some(map.next_value()?);
                        }
                        "directory" => {
                            if directory.is_some() {
                                return Err(de::Error::duplicate_field("directory"));
                            }
                            directory = Some(map.next_value()?);
                        }
                        "sha256" => {
                            if sha256.is_some() {
                                return Err(de::Error::duplicate_field("sha256"));
                            }
                            sha256 = Some(map.next_value()?);
                        }
                        "version" => {
                            if version.is_some() {
                                return Err(de::Error::duplicate_field("version"));
                            }
                            version = Some(map.next_value()?);
                        }
                        _ => return Err(de::Error::unknown_field(&key, GENERATION_FIELDS)),
                    }
                }
                Ok(ManagedGenerationV1 {
                    binary: binary.ok_or_else(|| de::Error::missing_field("binary"))?,
                    directory: directory.ok_or_else(|| de::Error::missing_field("directory"))?,
                    sha256: sha256.ok_or_else(|| de::Error::missing_field("sha256"))?,
                    version: version.ok_or_else(|| de::Error::missing_field("version"))?,
                })
            }
        }
        deserializer.deserialize_map(GenerationVisitor)
    }
}

impl<'de> Deserialize<'de> for ManagedToolchainStateV1 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct StateVisitor;

        impl<'de> Visitor<'de> for StateVisitor {
            type Value = ManagedToolchainStateV1;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an OpenCode2 managed state object")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut active = None;
                let mut format_version = None;
                let mut previous = None;
                let mut previous_seen = false;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "active" => {
                            if active.is_some() {
                                return Err(de::Error::duplicate_field("active"));
                            }
                            active = Some(map.next_value()?);
                        }
                        "format_version" => {
                            if format_version.is_some() {
                                return Err(de::Error::duplicate_field("format_version"));
                            }
                            format_version = Some(map.next_value()?);
                        }
                        "previous" => {
                            if previous_seen {
                                return Err(de::Error::duplicate_field("previous"));
                            }
                            previous_seen = true;
                            let value: Option<ManagedGenerationV1> = map.next_value()?;
                            previous =
                                Some(value.ok_or_else(|| {
                                    de::Error::custom("previous must be an object")
                                })?);
                        }
                        _ => return Err(de::Error::unknown_field(&key, STATE_FIELDS)),
                    }
                }
                Ok(ManagedToolchainStateV1 {
                    active: active.ok_or_else(|| de::Error::missing_field("active"))?,
                    format_version: format_version
                        .ok_or_else(|| de::Error::missing_field("format_version"))?,
                    previous,
                })
            }
        }
        deserializer.deserialize_map(StateVisitor)
    }
}

pub(crate) fn state_path_for_root(
    engine_root: &Path,
    engine_id: &str,
) -> Result<PathBuf, NativeOpenCode2StateError> {
    let expected_suffix = Path::new("toolchain").join(engine_id);
    if !engine_root.is_absolute()
        || engine_root
            .components()
            .any(|component| matches!(component, Component::ParentDir))
        || !engine_root.ends_with(expected_suffix)
    {
        return Err(NativeOpenCode2StateError::InvalidRoot);
    }
    Ok(engine_root.join("state.json"))
}

pub(crate) fn decode_state(bytes: &[u8]) -> Result<ManagedToolchainStateV1, NativeOpenCode2Error> {
    if bytes.len() > MAX_STATE_BYTES {
        return Err(NativeOpenCode2Error::StateTooLarge);
    }
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let state = ManagedToolchainStateV1::deserialize(&mut deserializer)
        .map_err(|_| NativeOpenCode2Error::StateMalformed)?;
    deserializer
        .end()
        .map_err(|_| NativeOpenCode2Error::StateMalformed)?;
    Ok(state)
}

pub(crate) fn validate_install_state(
    state: &ManagedToolchainStateV1,
    spec: &NativeOpenCode2InstallSpec,
) -> Result<(), NativeOpenCode2StateError> {
    validate_state_version(state).map_err(map_state_validation_error)?;
    validate_generation(&state.active, true, spec).map_err(map_state_validation_error)?;
    if let Some(previous) = state.previous.as_ref() {
        validate_generation(previous, false, spec).map_err(map_state_validation_error)?;
    }
    Ok(())
}

pub(crate) fn validate_state_version(
    state: &ManagedToolchainStateV1,
) -> Result<(), NativeOpenCode2Error> {
    if state.format_version != 1 {
        return Err(NativeOpenCode2Error::StateUnsupportedVersion);
    }
    Ok(())
}

pub(crate) fn validate_generation(
    generation: &ManagedGenerationV1,
    active: bool,
    spec: &NativeOpenCode2InstallSpec,
) -> Result<(), NativeOpenCode2Error> {
    if !is_safe_basename(&generation.directory, MAX_GENERATION_ID_BYTES)
        || !is_safe_relative_path(&generation.binary, MAX_BINARY_PATH_BYTES)
    {
        return Err(NativeOpenCode2Error::UnsafePath);
    }
    if !is_safe_sha256(&generation.sha256) || !is_safe_version(&generation.version) {
        return Err(NativeOpenCode2Error::ActiveGenerationUntrusted);
    }
    if active
        && (generation.binary != spec.binary()
            || generation.version != spec.version()
            || generation.sha256 != spec.executable_sha256_hex())
    {
        return Err(NativeOpenCode2Error::ActiveGenerationUntrusted);
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

pub(crate) fn is_safe_version(value: &str) -> bool {
    if value.is_empty() || value.len() > MAX_VERSION_BYTES || !value.is_ascii() {
        return false;
    }
    let (release, prerelease) = value
        .split_once('-')
        .map_or((value, None), |(release, pre)| (release, Some(pre)));
    let mut release_parts = release.split('.');
    for _ in 0..3 {
        let Some(part) = release_parts.next() else {
            return false;
        };
        if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return false;
        }
    }
    if release_parts.next().is_some() {
        return false;
    }
    prerelease.is_none_or(|pre| {
        !pre.is_empty()
            && pre
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
    })
}

pub(crate) fn map_state_file_error(error: NativeFileError) -> NativeOpenCode2StateError {
    match error {
        NativeFileError::TooLarge => NativeOpenCode2StateError::TooLarge,
        NativeFileError::UnsafePath | NativeFileError::PrivatePermissions => {
            NativeOpenCode2StateError::UnsafePath
        }
        NativeFileError::NotFound
        | NativeFileError::FileChanged
        | NativeFileError::FileSizeMismatch
        | NativeFileError::FileHashMismatch
        | NativeFileError::Io => NativeOpenCode2StateError::Io,
    }
}

pub(crate) fn map_state_decoder_error(error: NativeOpenCode2Error) -> NativeOpenCode2StateError {
    match error {
        NativeOpenCode2Error::StateTooLarge => NativeOpenCode2StateError::TooLarge,
        NativeOpenCode2Error::StateMalformed => NativeOpenCode2StateError::Malformed,
        other => map_state_validation_error(other),
    }
}

pub(crate) fn map_state_validation_error(error: NativeOpenCode2Error) -> NativeOpenCode2StateError {
    match error {
        NativeOpenCode2Error::StateUnsupportedVersion => {
            NativeOpenCode2StateError::UnsupportedVersion
        }
        NativeOpenCode2Error::UnsafePath => NativeOpenCode2StateError::UnsafePath,
        NativeOpenCode2Error::ActiveGenerationUntrusted => {
            NativeOpenCode2StateError::ActiveGenerationUntrusted
        }
        NativeOpenCode2Error::StateTooLarge => NativeOpenCode2StateError::TooLarge,
        NativeOpenCode2Error::StateMalformed
        | NativeOpenCode2Error::UnsupportedPlatform
        | NativeOpenCode2Error::StateMissing
        | NativeOpenCode2Error::ExecutableUnavailable
        | NativeOpenCode2Error::ExecutableChanged
        | NativeOpenCode2Error::ExecutableSizeMismatch
        | NativeOpenCode2Error::ExecutableHashMismatch
        | NativeOpenCode2Error::Io => NativeOpenCode2StateError::Malformed,
    }
}

pub(crate) fn map_state_seam_error(error: NativeOpenCode2StateError) -> NativeOpenCode2Error {
    match error {
        NativeOpenCode2StateError::InvalidRoot | NativeOpenCode2StateError::UnsafePath => {
            NativeOpenCode2Error::UnsafePath
        }
        NativeOpenCode2StateError::TooLarge => NativeOpenCode2Error::StateTooLarge,
        NativeOpenCode2StateError::Malformed => NativeOpenCode2Error::StateMalformed,
        NativeOpenCode2StateError::UnsupportedVersion => {
            NativeOpenCode2Error::StateUnsupportedVersion
        }
        NativeOpenCode2StateError::ActiveGenerationUntrusted => {
            NativeOpenCode2Error::ActiveGenerationUntrusted
        }
        NativeOpenCode2StateError::Io
        | NativeOpenCode2StateError::Encode
        | NativeOpenCode2StateError::AtomicPublishFailed => NativeOpenCode2Error::Io,
    }
}

pub(crate) fn map_state_replace_error(error: NativeFileError) -> NativeOpenCode2StateError {
    match error {
        NativeFileError::UnsafePath | NativeFileError::PrivatePermissions => {
            NativeOpenCode2StateError::UnsafePath
        }
        NativeFileError::NotFound
        | NativeFileError::TooLarge
        | NativeFileError::FileChanged
        | NativeFileError::FileSizeMismatch
        | NativeFileError::FileHashMismatch
        | NativeFileError::Io => NativeOpenCode2StateError::AtomicPublishFailed,
    }
}
