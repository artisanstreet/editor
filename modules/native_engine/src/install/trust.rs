//! Trust-on-first-download records for engines whose vendor publishes no
//! digest (owner decision 2026-09-26).
//!
//! `toolchain/<engine>/trust.json` keeps one record per exact version and
//! platform: the HTTPS URL it came from, the SHA-256 and size of the exact
//! bytes first downloaded, and when. A later download of the same version and
//! platform (reinstall, repair, reselecting a pruned version) must match the
//! record; a mismatch fails and never replaces the record. A new version gets
//! its own record.

use std::{
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::io as native_files;
use crate::io::NativeFileError;

use super::{
    catalog::{HostPlatform, ManagedEngine},
    state::{
        ManagedStateError, engine_file, is_safe_sha256, map_state_file_error,
        map_state_replace_error,
    },
    version::EngineVersion,
};

const MAX_TRUST_BYTES: usize = 128 * 1024;
/// Maximum records kept per engine; the oldest record is never evicted
/// silently, so a full store refuses new versions instead.
pub const MAX_TRUST_RECORDS: usize = 256;

/// One first-download record.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustRecord {
    pub version: String,
    pub platform: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
    /// Unix milliseconds of the first download.
    pub first_seen_at_ms: u64,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TrustDocument {
    format_version: u32,
    records: Vec<TrustRecord>,
}

/// The outcome of checking a download against the records.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TrustCheck {
    /// The bytes match the recorded first download.
    Matches(TrustRecord),
    /// First download of this version and platform; recorded now.
    Recorded(TrustRecord),
}

/// Reads every record; a missing store is empty.
///
/// # Errors
///
/// Returns [`ManagedStateError`] for an unsafe, oversized, or malformed store.
pub fn read_trust_records(
    engine_root: &Path,
    engine: ManagedEngine,
) -> Result<Vec<TrustRecord>, ManagedStateError> {
    let path = engine_file(engine_root, engine, "trust.json")?;
    let bytes = match native_files::read_bounded(&path, MAX_TRUST_BYTES) {
        Ok(bytes) => bytes,
        Err(NativeFileError::NotFound) => return Ok(Vec::new()),
        Err(error) => return Err(map_state_file_error(error)),
    };
    let document: TrustDocument =
        serde_json::from_slice(&bytes).map_err(|_| ManagedStateError::Malformed)?;
    if document.format_version != 1 {
        return Err(ManagedStateError::UnsupportedVersion);
    }
    if document.records.len() > MAX_TRUST_RECORDS
        || document.records.iter().any(|record| {
            !is_safe_sha256(&record.sha256)
                || EngineVersion::parse(&record.version).is_none()
                || !record.url.starts_with("https://")
        })
    {
        return Err(ManagedStateError::Malformed);
    }
    Ok(document.records)
}

/// Checks a completed download of `version` against the record for this
/// platform, recording it when it is the first.
///
/// # Errors
///
/// Returns [`TrustError::Mismatch`] when the bytes differ from the recorded
/// first download (the record is kept), or [`TrustError::Store`] when the
/// store cannot be read or published.
pub fn check_or_record(
    engine_root: &Path,
    engine: ManagedEngine,
    platform: HostPlatform,
    version: &EngineVersion,
    url: &str,
    sha256_hex: &str,
    size: u64,
) -> Result<TrustCheck, TrustError> {
    let mut records = read_trust_records(engine_root, engine).map_err(TrustError::Store)?;
    if let Some(record) = records
        .iter()
        .find(|record| record.version == version.as_str() && record.platform == platform.label())
    {
        return if record.sha256 == sha256_hex && record.size == size {
            Ok(TrustCheck::Matches(record.clone()))
        } else {
            Err(TrustError::Mismatch)
        };
    }
    if records.len() >= MAX_TRUST_RECORDS || !url.starts_with("https://") {
        return Err(TrustError::Store(ManagedStateError::TooLarge));
    }
    let record = TrustRecord {
        version: version.to_string(),
        platform: platform.label().to_owned(),
        url: url.to_owned(),
        sha256: sha256_hex.to_owned(),
        size,
        first_seen_at_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| {
                u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
            }),
    };
    records.push(record.clone());
    let bytes = serde_json::to_vec(&TrustDocument {
        format_version: 1,
        records,
    })
    .map_err(|_| TrustError::Store(ManagedStateError::Encode))?;
    let path = engine_file(engine_root, engine, "trust.json").map_err(TrustError::Store)?;
    let _outcome = native_files::replace_file(&path, &bytes)
        .map_err(|error| TrustError::Store(map_state_replace_error(error)))?;
    Ok(TrustCheck::Recorded(record))
}

/// A failed trust check.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrustError {
    /// The bytes differ from the recorded first download.
    Mismatch,
    /// The record store is unavailable or invalid.
    Store(ManagedStateError),
}

/// Returns the record of one version on one platform.
#[must_use]
pub fn record_for<'a>(
    records: &'a [TrustRecord],
    version: &str,
    platform: HostPlatform,
) -> Option<&'a TrustRecord> {
    records
        .iter()
        .find(|record| record.version == version && record.platform == platform.label())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const FIRST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const OTHER: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

    fn version(value: &str) -> EngineVersion {
        EngineVersion::parse(value).unwrap()
    }

    #[test]
    fn the_first_download_is_recorded_and_later_ones_must_match() {
        let root = tempfile::tempdir().unwrap();
        let engine_root = root.path().join("toolchain").join("grok");
        fs::create_dir_all(&engine_root).unwrap();
        let check = |sha: &str, size, v: &str| {
            check_or_record(
                &engine_root,
                ManagedEngine::Grok,
                HostPlatform::LinuxX64,
                &version(v),
                "https://x.ai/cli/grok-1.0.41-linux-x86_64",
                sha,
                size,
            )
        };
        assert!(matches!(
            check(FIRST, 10, "1.0.41"),
            Ok(TrustCheck::Recorded(_))
        ));
        assert!(matches!(
            check(FIRST, 10, "1.0.41"),
            Ok(TrustCheck::Matches(_))
        ));
        assert_eq!(check(OTHER, 10, "1.0.41"), Err(TrustError::Mismatch));
        assert_eq!(check(FIRST, 11, "1.0.41"), Err(TrustError::Mismatch));
        let records = read_trust_records(&engine_root, ManagedEngine::Grok).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(
            records[0].sha256, FIRST,
            "a mismatch never replaces the record"
        );
        assert!(matches!(
            check(OTHER, 12, "1.0.42"),
            Ok(TrustCheck::Recorded(_))
        ));
        assert_eq!(
            read_trust_records(&engine_root, ManagedEngine::Grok)
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn malformed_stores_and_plain_http_records_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        let engine_root = root.path().join("toolchain").join("cursor");
        fs::create_dir_all(&engine_root).unwrap();
        fs::write(
            engine_root.join("trust.json"),
            format!(
                r#"{{"format_version":1,"records":[{{"version":"1.0.0","platform":"linux-x64","url":"http://evil","sha256":"{FIRST}","size":1,"first_seen_at_ms":1}}]}}"#
            ),
        )
        .unwrap();
        assert_eq!(
            read_trust_records(&engine_root, ManagedEngine::Cursor),
            Err(ManagedStateError::Malformed)
        );
    }
}
