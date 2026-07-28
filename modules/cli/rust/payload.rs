//! Verifies the active version payload against its integrity manifest.
//!
//! `payload-manifest.json` is written by the bootstrap staging step
//! (`modules/bootstrap/rust/payload.rs`) at the root of `versions/<v>` and
//! maps every regular payload file (relative path, `/` separators) to its
//! lowercase hex SHA-256:
//!
//! ```json
//! { "format_version": 1, "files": { "bin/ae.exe": "<sha256>" } }
//! ```
//!
//! Doctor stays diagnostic: this module only reports drift — modified,
//! missing, or unexpected files — and never repairs. Versions installed
//! before the manifest existed are reported as unverifiable, not healthy.

use std::{
    collections::BTreeMap,
    fs::File,
    io::Read,
    path::{Component, Path},
};

use serde::Deserialize;
use sha2::{Digest, Sha256};

pub const PAYLOAD_MANIFEST_NAME: &str = "payload-manifest.json";
const SUPPORTED_FORMAT_VERSION: u64 = 1;
const MAX_REPORTED_ISSUES: usize = 5;

#[derive(Debug, Deserialize)]
struct PayloadManifest {
    files: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PayloadHealth {
    /// Every payload file matches the manifest and nothing else is present.
    Verified,
    /// Drift was detected; carries the first few human-readable findings.
    Modified(Vec<String>),
    /// No payload manifest exists (or a newer format), so integrity cannot
    /// be judged honestly either way.
    Unverifiable,
}

impl PayloadHealth {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Verified => "ok",
            Self::Modified(_) => "modified",
            Self::Unverifiable => "unverifiable",
        }
    }
}

/// Checks every file under `version_root` against `payload-manifest.json`.
pub fn verify(version_root: &Path) -> PayloadHealth {
    let manifest_path = version_root.join(PAYLOAD_MANIFEST_NAME);
    if !manifest_path.is_file() {
        return PayloadHealth::Unverifiable;
    }
    let Ok(bytes) = std::fs::read(&manifest_path) else {
        return PayloadHealth::Modified(vec![format!("unreadable: {PAYLOAD_MANIFEST_NAME}")]);
    };
    let Ok(document) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return PayloadHealth::Modified(vec![format!("invalid: {PAYLOAD_MANIFEST_NAME}")]);
    };
    match document
        .get("format_version")
        .and_then(serde_json::Value::as_u64)
    {
        Some(SUPPORTED_FORMAT_VERSION) => {}
        Some(_) => return PayloadHealth::Unverifiable,
        None => return PayloadHealth::Modified(vec![format!("invalid: {PAYLOAD_MANIFEST_NAME}")]),
    }
    let Ok(manifest) = serde_json::from_value::<PayloadManifest>(document) else {
        return PayloadHealth::Modified(vec![format!("invalid: {PAYLOAD_MANIFEST_NAME}")]);
    };

    let mut issues = Vec::new();
    for (relative, expected) in &manifest.files {
        if !is_safe_relative(relative) {
            issues.push(format!("invalid manifest entry: {relative}"));
            continue;
        }
        match hash_file(&version_root.join(relative)) {
            Ok(digest) if digest.eq_ignore_ascii_case(expected) => {}
            Ok(_) => issues.push(format!("modified: {relative}")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                issues.push(format!("missing: {relative}"));
            }
            Err(_) => issues.push(format!("unreadable: {relative}")),
        }
    }
    collect_unexpected(version_root, "", &manifest.files, &mut issues);

    if issues.is_empty() {
        return PayloadHealth::Verified;
    }
    if issues.len() > MAX_REPORTED_ISSUES {
        let remainder = issues.len() - MAX_REPORTED_ISSUES;
        issues.truncate(MAX_REPORTED_ISSUES);
        issues.push(format!("and {remainder} more"));
    }
    PayloadHealth::Modified(issues)
}

fn is_safe_relative(candidate: &str) -> bool {
    !candidate.is_empty()
        && Path::new(candidate)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

/// Reports files present in the payload that the manifest does not cover —
/// the signature of a build copied over an installed version.
fn collect_unexpected(
    directory: &Path,
    prefix: &str,
    manifest: &BTreeMap<String, String>,
    issues: &mut Vec<String>,
) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        issues.push(format!(
            "unreadable: {}",
            if prefix.is_empty() { "." } else { prefix }
        ));
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let relative = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        match entry.file_type() {
            Ok(file_type) if file_type.is_dir() => {
                collect_unexpected(&entry.path(), &relative, manifest, issues);
            }
            Ok(file_type)
                if file_type.is_file()
                    && (relative == PAYLOAD_MANIFEST_NAME || manifest.contains_key(&relative)) => {}
            Ok(_) | Err(_) => issues.push(format!("unexpected: {relative}")),
        }
    }
}

fn hash_file(path: &Path) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex_encode(&hasher.finalize()))
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut text, byte| {
            use std::fmt::Write;
            let _ = write!(text, "{byte:02x}");
            text
        })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{PAYLOAD_MANIFEST_NAME, PayloadHealth, hash_file, verify};

    fn write_payload(root: &Path) {
        std::fs::create_dir_all(root.join("forge")).expect("forge directory");
        std::fs::write(root.join("forge").join("host.js"), b"host").expect("host");
        std::fs::write(root.join("ae.exe"), b"artisan").expect("ae");
        let manifest = serde_json::json!({
            "format_version": 1,
            "files": {
                "ae.exe": hash_file(&root.join("ae.exe")).expect("hash ae"),
                "forge/host.js": hash_file(&root.join("forge").join("host.js"))
                    .expect("hash host"),
            },
        });
        std::fs::write(
            root.join(PAYLOAD_MANIFEST_NAME),
            serde_json::to_vec(&manifest).expect("serialize"),
        )
        .expect("manifest");
    }

    #[test]
    fn clean_payload_verifies() {
        let root = tempfile::tempdir().expect("temp");
        write_payload(root.path());
        assert_eq!(verify(root.path()), PayloadHealth::Verified);
    }

    #[test]
    fn modified_missing_and_unexpected_files_are_reported() {
        let root = tempfile::tempdir().expect("temp");
        write_payload(root.path());
        std::fs::write(root.path().join("forge").join("host.js"), b"dev overlay")
            .expect("overwrite");
        std::fs::write(root.path().join("forge").join("dev-chunk.js"), b"new").expect("extra");
        std::fs::remove_file(root.path().join("ae.exe")).expect("remove");

        let PayloadHealth::Modified(issues) = verify(root.path()) else {
            panic!("drift was not detected");
        };
        assert!(issues.contains(&"modified: forge/host.js".to_owned()));
        assert!(issues.contains(&"missing: ae.exe".to_owned()));
        assert!(issues.contains(&"unexpected: forge/dev-chunk.js".to_owned()));
    }

    #[test]
    fn missing_manifest_is_unverifiable_not_healthy() {
        let root = tempfile::tempdir().expect("temp");
        std::fs::write(root.path().join("ae.exe"), b"artisan").expect("ae");
        assert_eq!(verify(root.path()), PayloadHealth::Unverifiable);
    }

    #[test]
    fn corrupt_manifest_counts_as_drift_and_newer_format_stays_honest() {
        let root = tempfile::tempdir().expect("temp");
        write_payload(root.path());
        std::fs::write(root.path().join(PAYLOAD_MANIFEST_NAME), b"not json").expect("corrupt");
        assert!(matches!(verify(root.path()), PayloadHealth::Modified(_)));

        std::fs::write(
            root.path().join(PAYLOAD_MANIFEST_NAME),
            br#"{"format_version":2,"files":{}}"#,
        )
        .expect("future format");
        assert_eq!(verify(root.path()), PayloadHealth::Unverifiable);
    }
}
