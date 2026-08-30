use std::{
    fmt,
    path::{Component, Path, PathBuf},
};

use serde::{
    Deserialize,
    de::{self, Deserializer, MapAccess, Visitor},
};
#[cfg(test)]
use sha2::{Digest, Sha256};

use crate::instance::{self, NativeFileId, NativeInstanceConfig, NativeInstanceError};

pub(crate) const CERTIFIED_ENGINE_ID: &str = "opencode2";
pub(crate) const CERTIFIED_VERSION: &str = "0.0.0-beta-17778";
pub(crate) const CERTIFIED_UPSTREAM_COMMIT: &str = "0d2684b67308380fc47540fe55deb55306a08e3f";
pub(crate) const CERTIFIED_PLATFORM: &str = "win32";
pub(crate) const CERTIFIED_ARCHITECTURE: &str = "x64";
pub(crate) const CERTIFIED_ARTIFACT_KIND: &str = "npm-tarball";
pub(crate) const CERTIFIED_ARCHIVE_MEMBER: &str = "package/bin/opencode2.exe";
pub(crate) const CERTIFIED_BINARY: &str = "opencode2.exe";
pub(crate) const CERTIFIED_NPM_INTEGRITY_SHA512: &str =
    "Z0oMvTBUhxmz1IYuQSMOZTpI2HoWjeIjdxJ39SoGrhDwvJZK7OI0rgIMYtDGavOucOQT8oxrazUiO4j+2hVMpw==";
pub(crate) const CERTIFIED_DOWNLOAD_BOUND_BYTES: u64 = 268_435_456;
pub(crate) const CERTIFIED_EXECUTABLE_SIZE_BYTES: u64 = 144_313_344;
pub(crate) const CERTIFIED_EXECUTABLE_SHA256_HEX: &str =
    "452794a764e1033e629c4cd40bde6433c10c6bd32433fb3be279bf03969a6edf";
pub(crate) const CERTIFIED_EXECUTABLE_SHA256: [u8; 32] = [
    0x45, 0x27, 0x94, 0xa7, 0x64, 0xe1, 0x03, 0x3e, 0x62, 0x9c, 0x4c, 0xd4, 0x0b, 0xde, 0x64, 0x33,
    0xc1, 0x0c, 0x6b, 0xd3, 0x24, 0x33, 0xfb, 0x3b, 0xe2, 0x79, 0xbf, 0x03, 0x96, 0x9a, 0x6e, 0xdf,
];
pub(crate) const CERTIFIED_NPM_URL: &str = "https://registry.npmjs.org/@opencode-ai/cli-windows-x64/-/cli-windows-x64-0.0.0-beta-17778.tgz";

const MAX_STATE_BYTES: usize = 16 * 1024;
const MAX_GENERATION_ID_BYTES: usize = 128;
const MAX_BINARY_PATH_BYTES: usize = 256;
const MAX_VERSION_BYTES: usize = 128;

const STATE_FIELDS: &[&str] = &["active", "format_version", "previous"];
const GENERATION_FIELDS: &[&str] = &["binary", "directory", "sha256", "version"];

#[derive(Clone)]
struct ManagedToolchainStateV1 {
    active: ManagedGenerationV1,
    format_version: u32,
    previous: Option<ManagedGenerationV1>,
}

#[derive(Clone)]
struct ManagedGenerationV1 {
    binary: String,
    directory: String,
    sha256: String,
    version: String,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NativeOpenCode2Error {
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
    pub(crate) const fn cli_reason(self) -> &'static str {
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
        let message = match self {
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
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for NativeOpenCode2Error {}

#[derive(Debug)]
pub(crate) enum OpenCode2Inspection {
    UnsupportedPlatform,
    NotInstalled,
    Ready(ResolvedOpenCode2Generation),
}

pub(crate) struct ResolvedOpenCode2Generation {
    executable: PathBuf,
    generation_id: String,
    version: &'static str,
    upstream_commit: &'static str,
    executable_size_bytes: u64,
    executable_sha256: [u8; 32],
    verified_file_id: NativeFileId,
}

impl fmt::Debug for ResolvedOpenCode2Generation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let _ = self.verified_file_id;
        formatter
            .debug_struct("ResolvedOpenCode2Generation")
            .finish_non_exhaustive()
    }
}

impl ResolvedOpenCode2Generation {
    pub(crate) fn executable_path(&self) -> &Path {
        &self.executable
    }

    pub(crate) fn generation_id(&self) -> &str {
        &self.generation_id
    }

    pub(crate) fn version(&self) -> &'static str {
        self.version
    }

    pub(crate) fn upstream_commit(&self) -> &'static str {
        self.upstream_commit
    }

    pub(crate) fn executable_size_bytes(&self) -> u64 {
        self.executable_size_bytes
    }

    pub(crate) fn executable_sha256(&self) -> &[u8; 32] {
        &self.executable_sha256
    }
}

pub(crate) struct NativeOpenCode2Authority;

impl NativeOpenCode2Authority {
    pub(crate) const fn new() -> Self {
        let _ = (
            CERTIFIED_PLATFORM,
            CERTIFIED_ARCHITECTURE,
            CERTIFIED_ARTIFACT_KIND,
            CERTIFIED_ARCHIVE_MEMBER,
            CERTIFIED_NPM_INTEGRITY_SHA512,
            CERTIFIED_DOWNLOAD_BOUND_BYTES,
            CERTIFIED_NPM_URL,
        );
        Self
    }

    pub(crate) fn inspect(
        &self,
        instance: &NativeInstanceConfig,
    ) -> Result<OpenCode2Inspection, NativeOpenCode2Error> {
        if !is_supported_platform() {
            return Ok(OpenCode2Inspection::UnsupportedPlatform);
        }
        match Self::resolve_active(instance) {
            Ok(generation) => Ok(OpenCode2Inspection::Ready(generation)),
            Err(NativeOpenCode2Error::StateMissing) => Ok(OpenCode2Inspection::NotInstalled),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn resolve_active(
        instance: &NativeInstanceConfig,
    ) -> Result<ResolvedOpenCode2Generation, NativeOpenCode2Error> {
        if !is_supported_platform() {
            return Err(NativeOpenCode2Error::UnsupportedPlatform);
        }
        let paths = managed_paths(instance)?;
        let state = read_managed_state(&paths)?;
        validate_generation(&state.active, true)?;
        if let Some(previous) = state.previous.as_ref() {
            validate_generation(previous, false)?;
        }

        let executable = paths
            .versions_root
            .join(&state.active.directory)
            .join(&state.active.binary);
        let verified_file_id = instance::verify_native_file(
            &executable,
            CERTIFIED_EXECUTABLE_SIZE_BYTES,
            &CERTIFIED_EXECUTABLE_SHA256,
        )
        .map_err(|error| map_executable_error(&error))?;
        Ok(ResolvedOpenCode2Generation {
            executable,
            generation_id: state.active.directory,
            version: CERTIFIED_VERSION,
            upstream_commit: CERTIFIED_UPSTREAM_COMMIT,
            executable_size_bytes: CERTIFIED_EXECUTABLE_SIZE_BYTES,
            executable_sha256: CERTIFIED_EXECUTABLE_SHA256,
            verified_file_id,
        })
    }
}

struct ManagedPaths {
    state_path: PathBuf,
    versions_root: PathBuf,
}

fn is_supported_platform() -> bool {
    cfg!(all(target_os = "windows", target_arch = "x86_64"))
}

fn managed_paths(instance: &NativeInstanceConfig) -> Result<ManagedPaths, NativeOpenCode2Error> {
    let database_path = instance.database_path();
    if !database_path.is_absolute()
        || database_path
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(NativeOpenCode2Error::UnsafePath);
    }
    let database_parent = database_path
        .parent()
        .ok_or(NativeOpenCode2Error::UnsafePath)?;
    let toolchain_root = database_parent.join("toolchain");
    let engine_root = toolchain_root.join(CERTIFIED_ENGINE_ID);
    Ok(ManagedPaths {
        state_path: engine_root.join("state.json"),
        versions_root: engine_root.join("versions"),
    })
}

fn read_managed_state(
    paths: &ManagedPaths,
) -> Result<ManagedToolchainStateV1, NativeOpenCode2Error> {
    let bytes = instance::read_bounded_native_file(&paths.state_path, MAX_STATE_BYTES)
        .map_err(|error| map_state_error(&error))?;
    let state = decode_state(&bytes)?;
    validate_state_version(&state)?;
    Ok(state)
}

fn decode_state(bytes: &[u8]) -> Result<ManagedToolchainStateV1, NativeOpenCode2Error> {
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

fn validate_state_version(state: &ManagedToolchainStateV1) -> Result<(), NativeOpenCode2Error> {
    if state.format_version != 1 {
        return Err(NativeOpenCode2Error::StateUnsupportedVersion);
    }
    Ok(())
}

fn validate_generation(
    generation: &ManagedGenerationV1,
    active: bool,
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
        && (generation.binary != CERTIFIED_BINARY
            || generation.version != CERTIFIED_VERSION
            || generation.sha256 != CERTIFIED_EXECUTABLE_SHA256_HEX)
    {
        return Err(NativeOpenCode2Error::ActiveGenerationUntrusted);
    }
    Ok(())
}

fn is_safe_basename(value: &str, maximum_bytes: usize) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= maximum_bytes
        && bytes[0].is_ascii_alphanumeric()
        && bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'.' | b'_' | b'-'))
}

fn is_safe_relative_path(value: &str, maximum_bytes: usize) -> bool {
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

fn is_safe_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn is_safe_version(value: &str) -> bool {
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

fn map_state_error(error: &NativeInstanceError) -> NativeOpenCode2Error {
    match error {
        NativeInstanceError::NotFound => NativeOpenCode2Error::StateMissing,
        NativeInstanceError::TooLarge => NativeOpenCode2Error::StateTooLarge,
        NativeInstanceError::UnsafePath(_) => NativeOpenCode2Error::UnsafePath,
        NativeInstanceError::InvalidManifest => NativeOpenCode2Error::StateMalformed,
        NativeInstanceError::FileChanged
        | NativeInstanceError::FileSizeMismatch
        | NativeInstanceError::FileHashMismatch
        | NativeInstanceError::InvalidPath(_)
        | NativeInstanceError::Io { .. } => NativeOpenCode2Error::Io,
    }
}

fn map_executable_error(error: &NativeInstanceError) -> NativeOpenCode2Error {
    match error {
        NativeInstanceError::NotFound => NativeOpenCode2Error::ExecutableUnavailable,
        NativeInstanceError::TooLarge | NativeInstanceError::FileSizeMismatch => {
            NativeOpenCode2Error::ExecutableSizeMismatch
        }
        NativeInstanceError::FileChanged => NativeOpenCode2Error::ExecutableChanged,
        NativeInstanceError::FileHashMismatch => NativeOpenCode2Error::ExecutableHashMismatch,
        NativeInstanceError::UnsafePath(_) => NativeOpenCode2Error::UnsafePath,
        NativeInstanceError::InvalidPath(_) | NativeInstanceError::InvalidManifest => {
            NativeOpenCode2Error::Io
        }
        NativeInstanceError::Io { .. } => NativeOpenCode2Error::Io,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn certified_metadata_is_immutable() {
        assert_eq!(CERTIFIED_ENGINE_ID, "opencode2");
        assert_eq!(CERTIFIED_VERSION, "0.0.0-beta-17778");
        assert_eq!(
            CERTIFIED_UPSTREAM_COMMIT,
            "0d2684b67308380fc47540fe55deb55306a08e3f"
        );
        assert_eq!(CERTIFIED_PLATFORM, "win32");
        assert_eq!(CERTIFIED_ARCHITECTURE, "x64");
        assert_eq!(CERTIFIED_ARTIFACT_KIND, "npm-tarball");
        assert_eq!(CERTIFIED_ARCHIVE_MEMBER, "package/bin/opencode2.exe");
        assert_eq!(CERTIFIED_BINARY, "opencode2.exe");
        assert_eq!(CERTIFIED_DOWNLOAD_BOUND_BYTES, 268_435_456);
        assert_eq!(CERTIFIED_EXECUTABLE_SIZE_BYTES, 144_313_344);
        assert_eq!(
            CERTIFIED_EXECUTABLE_SHA256_HEX,
            "452794a764e1033e629c4cd40bde6433c10c6bd32433fb3be279bf03969a6edf"
        );
        assert_eq!(
            CERTIFIED_NPM_INTEGRITY_SHA512,
            "Z0oMvTBUhxmz1IYuQSMOZTpI2HoWjeIjdxJ39SoGrhDwvJZK7OI0rgIMYtDGavOucOQT8oxrazUiO4j+2hVMpw=="
        );
        assert_eq!(
            CERTIFIED_NPM_URL,
            "https://registry.npmjs.org/@opencode-ai/cli-windows-x64/-/cli-windows-x64-0.0.0-beta-17778.tgz"
        );
        assert_eq!(
            CERTIFIED_EXECUTABLE_SHA256,
            [
                0x45, 0x27, 0x94, 0xa7, 0x64, 0xe1, 0x03, 0x3e, 0x62, 0x9c, 0x4c, 0xd4, 0x0b, 0xde,
                0x64, 0x33, 0xc1, 0x0c, 0x6b, 0xd3, 0x24, 0x33, 0xfb, 0x3b, 0xe2, 0x79, 0xbf, 0x03,
                0x96, 0x9a, 0x6e, 0xdf,
            ]
        );
    }

    #[test]
    fn state_decoder_accepts_active_with_or_without_previous() {
        let active = r#"{"binary":"opencode2.exe","directory":"generation-a","sha256":"452794a764e1033e629c4cd40bde6433c10c6bd32433fb3be279bf03969a6edf","version":"0.0.0-beta-17778"}"#;
        let without_previous = format!(r#"{{"active":{active},"format_version":1}}"#);
        let state = decode_state(without_previous.as_bytes()).unwrap();
        assert_eq!(state.format_version, 1);
        assert!(state.previous.is_none());
        assert_eq!(state.active.directory, "generation-a");

        let previous = r#"{"binary":"old.exe","directory":"generation-old","sha256":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","version":"1.2.3"}"#;
        let with_previous =
            format!(r#"{{"active":{active},"format_version":1,"previous":{previous}}}"#);
        let state = decode_state(with_previous.as_bytes()).unwrap();
        assert_eq!(state.previous.as_ref().unwrap().directory, "generation-old");
        validate_generation(&state.active, true).unwrap();
        validate_generation(state.previous.as_ref().unwrap(), false).unwrap();
    }

    #[test]
    fn state_decoder_rejects_malformed_duplicate_unknown_trailing_and_null_previous() {
        let valid = r#"{"active":{"binary":"opencode2.exe","directory":"generation-a","sha256":"452794a764e1033e629c4cd40bde6433c10c6bd32433fb3be279bf03969a6edf","version":"0.0.0-beta-17778"},"format_version":1}"#;
        for malformed in [
            r#"{"active":{}}"#,
            r#"{"active":{"binary":"opencode2.exe","directory":"generation-a","sha256":"452794a764e1033e629c4cd40bde6433c10c6bd32433fb3be279bf03969a6edf","version":"0.0.0-beta-17778"},"format_version":1,"extra":true}"#,
            r#"{"active":{"binary":"opencode2.exe","directory":"generation-a","sha256":"452794a764e1033e629c4cd40bde6433c10c6bd32433fb3be279bf03969a6edf","version":"0.0.0-beta-17778"},"format_version":1,"format_version":1}"#,
            r#"{"active":{"binary":"opencode2.exe","directory":"generation-a","sha256":"452794a764e1033e629c4cd40bde6433c10c6bd32433fb3be279bf03969a6edf","version":"0.0.0-beta-17778"},"format_version":1} trailing"#,
            r#"{"active":{"binary":"opencode2.exe","directory":"generation-a","sha256":"452794a764e1033e629c4cd40bde6433c10c6bd32433fb3be279bf03969a6edf","version":"0.0.0-beta-17778"},"format_version":1,"previous":null}"#,
        ] {
            assert!(matches!(
                decode_state(malformed.as_bytes()),
                Err(NativeOpenCode2Error::StateMalformed)
            ));
        }
        assert!(decode_state(valid.as_bytes()).is_ok());
        assert!(matches!(
            decode_state(&[0xff, 0xfe]),
            Err(NativeOpenCode2Error::StateMalformed)
        ));
    }

    #[test]
    fn state_decoder_rejects_unsupported_format_and_oversized_bytes() {
        let valid = r#"{"active":{"binary":"opencode2.exe","directory":"generation-a","sha256":"452794a764e1033e629c4cd40bde6433c10c6bd32433fb3be279bf03969a6edf","version":"0.0.0-beta-17778"},"format_version":2}"#;
        let state = decode_state(valid.as_bytes()).unwrap();
        assert_eq!(state.format_version, 2);
        assert_eq!(
            validate_state_version(&state),
            Err(NativeOpenCode2Error::StateUnsupportedVersion)
        );
        assert!(matches!(
            decode_state(&vec![b' '; MAX_STATE_BYTES + 1]),
            Err(NativeOpenCode2Error::StateTooLarge)
        ));
    }

    #[test]
    fn unsafe_generation_binary_version_and_digest_values_fail_closed() {
        assert!(!is_safe_basename("../generation", MAX_GENERATION_ID_BYTES));
        assert!(!is_safe_relative_path(
            "../opencode2.exe",
            MAX_BINARY_PATH_BYTES
        ));
        assert!(!is_safe_relative_path(
            "C:\\opencode2.exe",
            MAX_BINARY_PATH_BYTES
        ));
        assert!(!is_safe_relative_path(
            "nested//opencode2.exe",
            MAX_BINARY_PATH_BYTES
        ));
        assert!(!is_safe_sha256(&"A".repeat(64)));
        assert!(!is_safe_version("0.0"));
        let mut active = certified_generation("generation-a");
        active.binary = "nested/opencode2.exe".into();
        assert_eq!(
            validate_generation(&active, true),
            Err(NativeOpenCode2Error::ActiveGenerationUntrusted)
        );
        active = certified_generation("generation-a");
        active.directory = "../generation-a".into();
        assert_eq!(
            validate_generation(&active, true),
            Err(NativeOpenCode2Error::UnsafePath)
        );
        active = certified_generation("generation-a");
        active.version = "1.2.3".into();
        assert_eq!(
            validate_generation(&active, true),
            Err(NativeOpenCode2Error::ActiveGenerationUntrusted)
        );
        active = certified_generation("generation-a");
        active.sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".into();
        assert_eq!(
            validate_generation(&active, true),
            Err(NativeOpenCode2Error::ActiveGenerationUntrusted)
        );
    }

    #[test]
    fn exact_database_parent_toolchain_root_is_used() {
        let root = std::env::temp_dir().join("authority-root");
        let instance = sample_instance(&root);
        let paths = managed_paths(&instance).unwrap();
        assert_eq!(
            paths.state_path,
            root.join("data")
                .join("toolchain")
                .join("opencode2")
                .join("state.json")
        );
        assert_eq!(
            paths.versions_root,
            root.join("data")
                .join("toolchain")
                .join("opencode2")
                .join("versions")
        );
        let traversal = NativeInstanceConfig::new(
            root.join("data").join("..").join("other").join("db.sqlite"),
            root.join("custody").join("lock"),
            root.join("readiness").join("ready"),
            root.join("credentials").join("manifest.json"),
            sample_listener(),
        )
        .unwrap();
        assert!(matches!(
            managed_paths(&traversal),
            Err(NativeOpenCode2Error::UnsafePath)
        ));
    }

    #[test]
    fn missing_state_is_not_a_fallback_to_previous() {
        let state = ManagedToolchainStateV1 {
            active: certified_generation("active-generation"),
            format_version: 1,
            previous: Some(certified_generation("previous-generation")),
        };
        assert_eq!(state.active.directory, "active-generation");
        assert_eq!(
            state.previous.as_ref().unwrap().directory,
            "previous-generation"
        );
        let root = tempfile::tempdir().unwrap();
        assert!(
            !is_supported_platform()
                || NativeOpenCode2Authority::resolve_active(&sample_instance(root.path())).is_err()
        );
    }

    #[test]
    fn active_generation_is_the_only_generation_path_candidate() {
        let root = tempfile::tempdir().unwrap();
        let instance = sample_instance(root.path());
        let paths = managed_paths(&instance).unwrap();
        let state = ManagedToolchainStateV1 {
            active: certified_generation("active-generation"),
            format_version: 1,
            previous: Some(certified_generation("previous-generation")),
        };
        let active_path = paths
            .versions_root
            .join(&state.active.directory)
            .join(&state.active.binary);
        assert!(active_path.ends_with("active-generation/opencode2.exe"));
        assert!(!active_path.ends_with("previous-generation/opencode2.exe"));
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn replaced_executable_identity_cannot_reuse_the_old_trust() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("opencode2.exe");
        let original = b"original binary";
        let replacement = b"replaced binary";
        assert_eq!(original.len(), replacement.len());
        std::fs::write(&path, original).unwrap();
        let original_hash = digest_array(original);
        let original_id =
            instance::verify_native_file(&path, original.len() as u64, &original_hash).unwrap();

        let backup = root.path().join("opencode2.old");
        std::fs::rename(&path, &backup).unwrap();
        std::fs::write(&path, replacement).unwrap();
        let replacement_hash = digest_array(replacement);
        let replacement_id =
            instance::verify_native_file(&path, replacement.len() as u64, &replacement_hash)
                .unwrap();
        assert_ne!(original_id, replacement_id);
        assert_eq!(
            instance::verify_native_file(&path, original.len() as u64, &original_hash),
            Err(NativeInstanceError::FileHashMismatch)
        );
    }

    #[test]
    fn unsupported_platform_does_not_read_managed_state() {
        let inspection = NativeOpenCode2Authority::new().inspect(&sample_instance(
            &std::env::temp_dir().join("authority-unsupported"),
        ));
        if !is_supported_platform() {
            assert!(matches!(
                inspection,
                Ok(OpenCode2Inspection::UnsupportedPlatform)
            ));
        }
    }

    #[cfg(unix)]
    #[test]
    fn state_and_executable_symlinks_are_rejected() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let state_directory = root.path().join("data").join("toolchain").join("opencode2");
        std::fs::create_dir_all(&state_directory).unwrap();
        let real_state = root.path().join("real-state.json");
        std::fs::write(&real_state, b"{}").unwrap();
        symlink(&real_state, state_directory.join("state.json")).unwrap();
        let instance = sample_instance(root.path());
        assert!(matches!(
            read_managed_state(&managed_paths(&instance).unwrap()),
            Err(NativeOpenCode2Error::UnsafePath)
        ));

        let executable = root.path().join("executable.exe");
        std::fs::write(&executable, b"native").unwrap();
        let link = root.path().join("executable-link.exe");
        symlink(&executable, &link).unwrap();
        let expected = digest_array(b"native");
        assert!(matches!(
            instance::verify_native_file(&link, 6, &expected),
            Err(NativeInstanceError::UnsafePath(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn ancestor_symlinks_are_rejected_before_state_read() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let real_data = root.path().join("real-data");
        std::fs::create_dir_all(real_data.join("toolchain").join("opencode2")).unwrap();
        symlink(&real_data, root.path().join("data")).unwrap();
        let instance = sample_instance(root.path());
        let paths = managed_paths(&instance).unwrap();
        let state = br#"{"active":{"binary":"opencode2.exe","directory":"generation-a","sha256":"452794a764e1033e629c4cd40bde6433c10c6bd32433fb3be279bf03969a6edf","version":"0.0.0-beta-17778"},"format_version":1}"#;
        std::fs::write(&paths.state_path, state).unwrap();
        assert!(matches!(
            read_managed_state(&paths),
            Err(NativeOpenCode2Error::UnsafePath)
        ));
    }

    #[test]
    fn bounded_state_reader_never_returns_bytes_above_its_limit() {
        let root = tempfile::tempdir().unwrap();
        let instance = sample_instance(root.path());
        let paths = managed_paths(&instance).unwrap();
        std::fs::create_dir_all(paths.state_path.parent().unwrap()).unwrap();
        std::fs::write(&paths.state_path, vec![b'x'; MAX_STATE_BYTES + 1]).unwrap();
        assert_eq!(
            instance::read_bounded_native_file(&paths.state_path, MAX_STATE_BYTES),
            Err(NativeInstanceError::TooLarge)
        );
    }

    #[test]
    fn bounded_stream_verification_checks_size_and_hash() {
        let bytes = b"a synthetic bounded fixture spanning a reader call";
        let expected = digest_array(bytes);
        let mut reader = std::io::Cursor::new(bytes.as_slice());
        assert!(instance::stream_and_verify(&mut reader, bytes.len() as u64, &expected).is_ok());

        let mut short = std::io::Cursor::new(&bytes[..bytes.len() - 1]);
        assert_eq!(
            instance::stream_and_verify(&mut short, bytes.len() as u64, &expected),
            Err(NativeInstanceError::FileSizeMismatch)
        );
        let mut wrong_hash = std::io::Cursor::new(bytes.as_slice());
        assert_eq!(
            instance::stream_and_verify(&mut wrong_hash, bytes.len() as u64, &[0; 32]),
            Err(NativeInstanceError::FileHashMismatch)
        );
        let mut oversized = std::io::Cursor::new([bytes.as_slice(), b"!"].concat());
        assert_eq!(
            instance::stream_and_verify(&mut oversized, bytes.len() as u64, &expected),
            Err(NativeInstanceError::FileSizeMismatch)
        );
    }

    #[test]
    fn native_file_verification_rejects_size_and_hash_mismatch() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("opencode2.exe");
        let bytes = b"native executable fixture";
        std::fs::write(&path, bytes).unwrap();
        let expected = digest_array(bytes);
        assert!(instance::verify_native_file(&path, (bytes.len() + 1) as u64, &expected).is_err());
        assert_eq!(
            instance::verify_native_file(&path, bytes.len() as u64, &[0; 32]),
            Err(NativeInstanceError::FileHashMismatch)
        );
        assert!(instance::verify_native_file(&path, bytes.len() as u64, &expected).is_ok());
    }

    #[test]
    fn authority_errors_and_resolved_debug_are_redacted() {
        let error = NativeOpenCode2Error::ExecutableHashMismatch;
        assert_eq!(
            error.to_string(),
            "OpenCode2 executable hash does not match"
        );
        assert!(!format!("{error:?}").contains("452794"));
        assert!(!format!("{error}").contains("C:\\secret"));

        let generation = ResolvedOpenCode2Generation {
            executable: PathBuf::from("C:\\secret\\opencode2.exe"),
            generation_id: "generation-a".into(),
            version: CERTIFIED_VERSION,
            upstream_commit: CERTIFIED_UPSTREAM_COMMIT,
            executable_size_bytes: CERTIFIED_EXECUTABLE_SIZE_BYTES,
            executable_sha256: CERTIFIED_EXECUTABLE_SHA256,
            verified_file_id: verified_fixture_file_id(),
        };
        let debug = format!("{generation:?}");
        assert!(!debug.contains("secret"));
        assert!(!debug.contains(CERTIFIED_EXECUTABLE_SHA256_HEX));
        assert!(!debug.contains("volume"));
        assert!(!debug.contains("dev"));
    }

    #[test]
    fn state_bytes_remain_unchanged_after_bounded_read() {
        let root = tempfile::tempdir().unwrap();
        let instance = sample_instance(root.path());
        let paths = managed_paths(&instance).unwrap();
        std::fs::create_dir_all(paths.state_path.parent().unwrap()).unwrap();
        let bytes = br#"{"active":{"binary":"opencode2.exe","directory":"generation-a","sha256":"452794a764e1033e629c4cd40bde6433c10c6bd32433fb3be279bf03969a6edf","version":"0.0.0-beta-17778"},"format_version":1}"#;
        std::fs::write(&paths.state_path, bytes).unwrap();
        let before = std::fs::read(&paths.state_path).unwrap();
        let _ = read_managed_state(&paths).unwrap();
        let after = std::fs::read(&paths.state_path).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn a_different_engine_root_is_never_considered() {
        let root = tempfile::tempdir().unwrap();
        let instance = sample_instance(root.path());
        let other_state = root
            .path()
            .join("data")
            .join("toolchain")
            .join("other-engine")
            .join("state.json");
        std::fs::create_dir_all(other_state.parent().unwrap()).unwrap();
        std::fs::write(&other_state, b"not OpenCode2 state").unwrap();
        let paths = managed_paths(&instance).unwrap();
        assert!(matches!(
            read_managed_state(&paths),
            Err(NativeOpenCode2Error::StateMissing)
        ));
    }

    fn certified_generation(directory: &str) -> ManagedGenerationV1 {
        ManagedGenerationV1 {
            binary: CERTIFIED_BINARY.into(),
            directory: directory.into(),
            sha256: CERTIFIED_EXECUTABLE_SHA256_HEX.into(),
            version: CERTIFIED_VERSION.into(),
        }
    }

    fn sample_instance(root: &Path) -> NativeInstanceConfig {
        NativeInstanceConfig::new(
            root.join("data").join("artisan.sqlite"),
            root.join("custody").join("lock"),
            root.join("readiness").join("ready"),
            root.join("credentials").join("manifest.json"),
            sample_listener(),
        )
        .unwrap()
    }

    fn sample_listener() -> crate::instance::NativeListenerConfig {
        use std::num::NonZeroU32;

        crate::instance::NativeListenerConfig::new(
            1,
            2,
            3,
            4,
            NonZeroU32::new(1).unwrap(),
            NonZeroU32::new(1).unwrap(),
        )
    }

    #[cfg(any(unix, windows))]
    fn verified_fixture_file_id() -> NativeFileId {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("debug-fixture.bin");
        let bytes = b"debug fixture";
        std::fs::write(&path, bytes).unwrap();
        let digest = digest_array(bytes);
        instance::verify_native_file(&path, bytes.len() as u64, &digest).unwrap()
    }

    #[cfg(not(any(unix, windows)))]
    fn verified_fixture_file_id() -> NativeFileId {
        NativeFileId
    }

    fn digest_array(bytes: &[u8]) -> [u8; 32] {
        let digest = Sha256::digest(bytes);
        let mut result = [0_u8; 32];
        result.copy_from_slice(&digest);
        result
    }
}
