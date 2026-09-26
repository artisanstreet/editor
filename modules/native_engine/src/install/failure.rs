//! The last failed install of an engine, so every surface (the Forge status
//! push, `ae engine list|status`) reports `failed: <reason>` instead of "not
//! installed", and the Forge retries with backoff.
//!
//! `toolchain/<engine>/install-failure.json` holds the classification, the
//! specific detail (for example `too_many_entries: 632 entries, limit 512`),
//! the version being installed, how many consecutive attempts failed, and
//! when. A successful install removes it.

use std::{
    fs, io,
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::io as native_files;
use crate::io::NativeFileError;

use super::{
    catalog::ManagedEngine,
    operations::InstallError,
    state::{ManagedStateError, engine_file, map_state_file_error, map_state_replace_error},
    version::EngineVersion,
};

const FAILURE_FILE: &str = "install-failure.json";
const MAX_FAILURE_BYTES: usize = 4 * 1024;
const MAX_DETAIL_BYTES: usize = 400;
/// First retry delay; doubles per consecutive failure.
const FIRST_RETRY: Duration = Duration::from_mins(1);
/// Longest retry delay (the regular update interval).
const MAX_RETRY: Duration = Duration::from_hours(6);

/// One engine's last failed install.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallFailure {
    format_version: u32,
    /// Stable classification ([`InstallError::code`]).
    pub code: String,
    /// Classification with specifics ([`InstallError::detail`]).
    pub detail: String,
    /// The version being installed, when it was resolved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Consecutive failed attempts.
    pub attempts: u32,
    /// Unix milliseconds of the latest failure.
    pub failed_at_ms: u64,
}

impl InstallFailure {
    /// Unix milliseconds after which the Forge retries: one minute after the
    /// first failure, doubling per attempt, at most six hours.
    #[must_use]
    pub fn retry_at_ms(&self) -> u64 {
        let factor = 1_u32
            .checked_shl(self.attempts.saturating_sub(1))
            .unwrap_or(u32::MAX);
        let delay = FIRST_RETRY.saturating_mul(factor).min(MAX_RETRY);
        self.failed_at_ms
            .saturating_add(u64::try_from(delay.as_millis()).unwrap_or(u64::MAX))
    }

    /// Returns whether the retry time has passed.
    #[must_use]
    pub fn retry_due(&self) -> bool {
        now_ms() >= self.retry_at_ms()
    }
}

/// Reads the last failure; a missing record is `None`.
///
/// # Errors
///
/// Returns [`ManagedStateError`] for an unsafe, oversized, or malformed
/// record.
pub fn read_install_failure(
    engine_root: &Path,
    engine: ManagedEngine,
) -> Result<Option<InstallFailure>, ManagedStateError> {
    let path = engine_file(engine_root, engine, FAILURE_FILE)?;
    let bytes = match native_files::read_bounded(&path, MAX_FAILURE_BYTES) {
        Ok(bytes) => bytes,
        Err(NativeFileError::NotFound) => return Ok(None),
        Err(error) => return Err(map_state_file_error(error)),
    };
    let failure: InstallFailure =
        serde_json::from_slice(&bytes).map_err(|_| ManagedStateError::Malformed)?;
    if failure.format_version != 1 {
        return Err(ManagedStateError::UnsupportedVersion);
    }
    if !is_safe_text(&failure.code, 64)
        || !is_safe_text(&failure.detail, MAX_DETAIL_BYTES)
        || failure
            .version
            .as_deref()
            .is_some_and(|version| EngineVersion::parse(version).is_none())
        || failure.attempts == 0
    {
        return Err(ManagedStateError::Malformed);
    }
    Ok(Some(failure))
}

/// Records a failed attempt, counting consecutive failures.
///
/// # Errors
///
/// Returns [`ManagedStateError`] when the record cannot be published.
pub fn record_install_failure(
    engine_root: &Path,
    engine: ManagedEngine,
    error: &InstallError,
    version: Option<&EngineVersion>,
) -> Result<InstallFailure, ManagedStateError> {
    let attempts = read_install_failure(engine_root, engine)
        .ok()
        .flatten()
        .map_or(1, |previous| previous.attempts.saturating_add(1));
    let failure = InstallFailure {
        format_version: 1,
        code: error.code().to_owned(),
        detail: bounded(&error.detail()),
        version: version.map(ToString::to_string),
        attempts,
        failed_at_ms: now_ms(),
    };
    let bytes = serde_json::to_vec(&failure).map_err(|_| ManagedStateError::Encode)?;
    let path = engine_file(engine_root, engine, FAILURE_FILE)?;
    let _outcome = native_files::replace_file(&path, &bytes).map_err(map_state_replace_error)?;
    Ok(failure)
}

/// Removes the failure record after a successful install.
///
/// # Errors
///
/// Returns [`ManagedStateError`] when the record exists but cannot be
/// removed.
pub fn clear_install_failure(
    engine_root: &Path,
    engine: ManagedEngine,
) -> Result<(), ManagedStateError> {
    let path = engine_file(engine_root, engine, FAILURE_FILE)?;
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(ManagedStateError::Io),
    }
}

fn is_safe_text(text: &str, limit: usize) -> bool {
    !text.trim().is_empty() && text.len() <= limit && !text.chars().any(char::is_control)
}

fn bounded(text: &str) -> String {
    let mut end = text.len().min(MAX_DETAIL_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end]
        .chars()
        .map(|character| {
            if character.is_control() {
                '?'
            } else {
                character
            }
        })
        .collect()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine_core::ArchiveError;

    #[test]
    fn failures_count_attempts_back_off_and_clear() {
        let root = tempfile::tempdir().unwrap();
        let engine_root = root.path().join("toolchain").join("codex");
        fs::create_dir_all(&engine_root).unwrap();
        assert_eq!(
            read_install_failure(&engine_root, ManagedEngine::Codex),
            Ok(None)
        );
        let error = InstallError::Archive(ArchiveError::TooManyEntries {
            entries: 632,
            limit: 512,
        });
        let version = EngineVersion::parse("0.157.1").unwrap();
        let first =
            record_install_failure(&engine_root, ManagedEngine::Codex, &error, Some(&version))
                .unwrap();
        assert_eq!(first.code, "too_many_entries");
        assert_eq!(first.detail, "too_many_entries: 632 entries, limit 512");
        assert_eq!(first.attempts, 1);
        assert_eq!(first.retry_at_ms() - first.failed_at_ms, 60_000);
        let second =
            record_install_failure(&engine_root, ManagedEngine::Codex, &error, None).unwrap();
        assert_eq!(second.attempts, 2);
        assert_eq!(second.retry_at_ms() - second.failed_at_ms, 120_000);
        assert_eq!(
            read_install_failure(&engine_root, ManagedEngine::Codex),
            Ok(Some(second.clone()))
        );
        let many = InstallFailure {
            attempts: 40,
            ..second
        };
        assert_eq!(many.retry_at_ms() - many.failed_at_ms, 6 * 3_600_000);
        clear_install_failure(&engine_root, ManagedEngine::Codex).unwrap();
        clear_install_failure(&engine_root, ManagedEngine::Codex).unwrap();
        assert_eq!(
            read_install_failure(&engine_root, ManagedEngine::Codex),
            Ok(None)
        );
    }

    #[test]
    fn malformed_records_and_foreign_roots_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        let engine_root = root.path().join("toolchain").join("cursor");
        fs::create_dir_all(&engine_root).unwrap();
        fs::write(
            engine_root.join(FAILURE_FILE),
            r#"{"format_version":1,"code":"x","detail":"a\nb","attempts":1,"failed_at_ms":1}"#,
        )
        .unwrap();
        assert_eq!(
            read_install_failure(&engine_root, ManagedEngine::Cursor),
            Err(ManagedStateError::Malformed)
        );
        assert_eq!(
            read_install_failure(&engine_root, ManagedEngine::Codex),
            Err(ManagedStateError::InvalidRoot)
        );
    }
}
