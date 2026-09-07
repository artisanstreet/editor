//! Verifies the active version payload against its integrity manifest.
//!
//! `payload-manifest.json` is written by the bootstrap staging step
//! (`modules/bootstrap/rust/payload.rs`) at the root of `versions/<v>` and
//! maps the four required binaries plus optional resource/license files
//! (relative paths, `/` separators) to their lowercase hex SHA-256:
//!
//! ```json
//! { "format_version": 1, "files": { "bin/ae.exe": "<sha256>", "bin/editor.exe": "<sha256>", "bin/forge.exe": "<sha256>", "bin/installer.exe": "<sha256>" } }
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

use crate::{CliError, Result};

pub const PAYLOAD_MANIFEST_NAME: &str = "payload-manifest.json";
const SUPPORTED_FORMAT_VERSION: u64 = 1;
const MAX_REPORTED_ISSUES: usize = 5;
const OPTIONAL_PAYLOAD_DIRECTORIES: [&str; 2] = ["resources", "licenses"];
const FORBIDDEN_LEGACY_NAMES: [&str; 11] = [
    "broker",
    "broker.exe",
    "broker.js",
    "node",
    "node.exe",
    "node.js",
    "node_modules",
    "electron",
    "electron.exe",
    "electron.asar",
    "host.js",
];

#[cfg(windows)]
const REQUIRED_PAYLOAD_FILES: [&str; 4] = [
    "bin/ae.exe",
    "bin/installer.exe",
    "bin/editor.exe",
    "bin/forge.exe",
];

#[cfg(not(windows))]
const REQUIRED_PAYLOAD_FILES: [&str; 4] = ["bin/ae", "bin/installer", "bin/editor", "bin/forge"];

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
        if !is_allowed_manifest_file(relative) {
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
    for required in REQUIRED_PAYLOAD_FILES {
        if !manifest.files.contains_key(required) {
            issues.push(format!("missing manifest entry: {required}"));
        }
    }
    collect_unexpected(version_root, &manifest.files, &mut issues);

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

/// Refuses launch when the active version payload cannot be fully verified.
pub(crate) fn require_verified(version_root: &Path) -> Result<()> {
    match verify(version_root) {
        PayloadHealth::Verified => Ok(()),
        PayloadHealth::Modified(_) | PayloadHealth::Unverifiable => Err(CliError::Installation(
            "active version payload is not verified".to_owned(),
        )),
    }
}

fn is_safe_relative(candidate: &str) -> bool {
    !candidate.is_empty()
        && !candidate
            .bytes()
            .any(|byte| matches!(byte, b'\\' | b':' | 0))
        && Path::new(candidate)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn is_allowed_manifest_file(relative: &str) -> bool {
    !is_forbidden_legacy_member(relative)
        && (REQUIRED_PAYLOAD_FILES.contains(&relative)
            || relative.split_once('/').is_some_and(|(directory, member)| {
                !member.is_empty() && OPTIONAL_PAYLOAD_DIRECTORIES.contains(&directory)
            }))
}

fn is_required_payload_file(relative: &str) -> bool {
    REQUIRED_PAYLOAD_FILES.contains(&relative)
}

fn is_forbidden_legacy_member(relative: &str) -> bool {
    relative.rsplit('/').next().is_some_and(|name| {
        FORBIDDEN_LEGACY_NAMES
            .iter()
            .any(|forbidden| name.eq_ignore_ascii_case(forbidden))
    })
}

#[derive(Clone, Copy)]
enum PayloadDirectory {
    Bin,
    Optional,
}

/// Reports files present in the payload that the manifest does not cover —
/// the signature of a build copied over an installed version.
fn collect_unexpected(
    directory: &Path,
    manifest: &BTreeMap<String, String>,
    issues: &mut Vec<String>,
) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        issues.push("unreadable: .".to_owned());
        return;
    };
    for entry in entries.flatten() {
        let relative = entry.file_name().to_string_lossy().into_owned();
        let Ok(file_type) = entry.file_type() else {
            issues.push(format!("unexpected: {relative}"));
            continue;
        };

        if relative == PAYLOAD_MANIFEST_NAME && file_type.is_file() {
            continue;
        }
        match relative.as_str() {
            "bin" if file_type.is_dir() => collect_directory_members(
                &entry.path(),
                &relative,
                PayloadDirectory::Bin,
                manifest,
                issues,
            ),
            directory
                if OPTIONAL_PAYLOAD_DIRECTORIES.contains(&directory) && file_type.is_dir() =>
            {
                collect_directory_members(
                    &entry.path(),
                    &relative,
                    PayloadDirectory::Optional,
                    manifest,
                    issues,
                );
            }
            _ => {
                issues.push(format!("unexpected: {relative}"));
                if file_type.is_dir() {
                    collect_unexpected_tree(&entry.path(), &relative, issues);
                }
            }
        }
    }
}

fn collect_directory_members(
    directory: &Path,
    prefix: &str,
    kind: PayloadDirectory,
    manifest: &BTreeMap<String, String>,
    issues: &mut Vec<String>,
) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        issues.push(format!("unreadable: {prefix}"));
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let relative = format!("{prefix}/{name}");
        let Ok(file_type) = entry.file_type() else {
            issues.push(format!("unexpected: {relative}"));
            continue;
        };
        if is_forbidden_legacy_member(&relative) {
            issues.push(format!("unexpected: {relative}"));
            continue;
        }
        if matches!(kind, PayloadDirectory::Bin)
            && file_type.is_file()
            && is_required_payload_file(&relative)
        {
            continue;
        }
        if matches!(kind, PayloadDirectory::Optional)
            && file_type.is_file()
            && manifest.contains_key(&relative)
        {
            match is_non_executable_file(&entry.path(), &relative) {
                Ok(true) => continue,
                Ok(false) => {
                    issues.push(format!("unexpected: {relative}"));
                    continue;
                }
                Err(_) => {
                    issues.push(format!("unreadable: {relative}"));
                    continue;
                }
            }
        }
        if matches!(kind, PayloadDirectory::Optional) && file_type.is_dir() {
            collect_directory_members(
                &entry.path(),
                &relative,
                PayloadDirectory::Optional,
                manifest,
                issues,
            );
        } else {
            issues.push(format!("unexpected: {relative}"));
        }
    }
}

fn is_non_executable_file(path: &Path, relative: &str) -> std::io::Result<bool> {
    if relative
        .rsplit('/')
        .next()
        .is_some_and(|name| name.to_ascii_lowercase().ends_with(".exe"))
    {
        return Ok(false);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        Ok(std::fs::metadata(path)?.permissions().mode() & 0o111 == 0)
    }
    #[cfg(not(unix))]
    {
        std::fs::metadata(path)?;
        Ok(true)
    }
}

fn collect_unexpected_tree(directory: &Path, prefix: &str, issues: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        issues.push(format!("unreadable: {prefix}"));
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let relative = format!("{prefix}/{name}");
        let Ok(file_type) = entry.file_type() else {
            issues.push(format!("unexpected: {relative}"));
            continue;
        };
        issues.push(format!("unexpected: {relative}"));
        if file_type.is_dir() {
            collect_unexpected_tree(&entry.path(), &relative, issues);
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
    use std::{collections::BTreeMap, path::Path};

    use crate::CliError;

    use super::{
        PAYLOAD_MANIFEST_NAME, PayloadHealth, REQUIRED_PAYLOAD_FILES, hash_file, require_verified,
        verify,
    };

    fn assert_launch_admission_rejected(root: &Path) {
        let error = require_verified(root).expect_err("unverified payload was admitted");
        assert_eq!(
            error.to_string(),
            "Artisan is not installed correctly: active version payload is not verified"
        );
        assert!(
            matches!(error, CliError::Installation(message) if message == "active version payload is not verified")
        );
    }

    fn write_payload(root: &Path) {
        std::fs::create_dir_all(root.join("bin")).expect("bin directory");
        for (index, relative) in REQUIRED_PAYLOAD_FILES.iter().enumerate() {
            std::fs::write(root.join(*relative), format!("binary-{index}"))
                .expect("required binary");
        }
        write_fixture_manifest(root, &[]);
    }

    fn write_fixture_manifest(root: &Path, optional: &[&str]) {
        let mut files = BTreeMap::new();
        for relative in REQUIRED_PAYLOAD_FILES {
            files.insert(
                relative.to_owned(),
                hash_file(&root.join(relative)).expect("required hash"),
            );
        }
        for relative in optional {
            files.insert(
                (*relative).to_owned(),
                hash_file(&root.join(*relative)).expect("optional hash"),
            );
        }
        let manifest = serde_json::json!({
            "format_version": 1,
            "files": files,
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
        require_verified(root.path()).expect("verified payload was rejected");
    }

    #[test]
    fn every_required_binary_hash_is_checked() {
        for relative in REQUIRED_PAYLOAD_FILES {
            let root = tempfile::tempdir().expect("temp");
            write_payload(root.path());
            std::fs::write(root.path().join(relative), b"modified").expect("overwrite");

            let PayloadHealth::Modified(issues) = verify(root.path()) else {
                panic!("modified binary was accepted: {relative}");
            };
            assert!(issues.contains(&format!("modified: {relative}")));
            assert_launch_admission_rejected(root.path());
        }
    }

    #[test]
    fn each_missing_required_binary_is_reported() {
        for relative in REQUIRED_PAYLOAD_FILES {
            let root = tempfile::tempdir().expect("temp");
            write_payload(root.path());
            std::fs::remove_file(root.path().join(relative)).expect("remove binary");

            let PayloadHealth::Modified(issues) = verify(root.path()) else {
                panic!("missing binary was accepted: {relative}");
            };
            assert!(issues.contains(&format!("missing: {relative}")));
            assert_launch_admission_rejected(root.path());
        }
    }

    #[test]
    fn extra_bin_members_are_reported() {
        let root = tempfile::tempdir().expect("temp");
        write_payload(root.path());
        std::fs::write(root.path().join("bin").join("extra"), b"extra").expect("extra");

        let PayloadHealth::Modified(issues) = verify(root.path()) else {
            panic!("extra binary was accepted");
        };
        assert!(issues.contains(&"unexpected: bin/extra".to_owned()));
        assert_launch_admission_rejected(root.path());

        let root = tempfile::tempdir().expect("temp");
        write_payload(root.path());
        std::fs::create_dir(root.path().join("bin").join("extra-directory"))
            .expect("extra directory");
        let PayloadHealth::Modified(issues) = verify(root.path()) else {
            panic!("extra bin directory was accepted");
        };
        assert!(issues.contains(&"unexpected: bin/extra-directory".to_owned()));
        assert_launch_admission_rejected(root.path());
    }

    #[test]
    fn wrong_platform_binary_extension_is_reported() {
        let root = tempfile::tempdir().expect("temp");
        write_payload(root.path());
        let wrong_extension = if cfg!(windows) {
            "bin/ae"
        } else {
            "bin/ae.exe"
        };
        std::fs::write(root.path().join(wrong_extension), b"wrong").expect("wrong extension");

        let PayloadHealth::Modified(issues) = verify(root.path()) else {
            panic!("wrong platform binary was accepted");
        };
        assert!(issues.contains(&format!("unexpected: {wrong_extension}")));
    }

    #[test]
    fn resources_and_licenses_are_supported_and_hashed() {
        let root = tempfile::tempdir().expect("temp");
        write_payload(root.path());
        std::fs::create_dir_all(root.path().join("resources").join("nested")).expect("resources");
        std::fs::create_dir_all(root.path().join("licenses")).expect("licenses");
        std::fs::write(
            root.path()
                .join("resources")
                .join("nested")
                .join("config.json"),
            b"config",
        )
        .expect("resource");
        std::fs::write(
            root.path().join("licenses").join("third-party.txt"),
            b"license",
        )
        .expect("license");
        write_fixture_manifest(
            root.path(),
            &["resources/nested/config.json", "licenses/third-party.txt"],
        );
        assert_eq!(verify(root.path()), PayloadHealth::Verified);

        std::fs::write(
            root.path()
                .join("resources")
                .join("nested")
                .join("config.json"),
            b"changed",
        )
        .expect("modify resource");
        let PayloadHealth::Modified(issues) = verify(root.path()) else {
            panic!("modified resource was accepted");
        };
        assert!(issues.contains(&"modified: resources/nested/config.json".to_owned()));
    }

    #[test]
    fn legacy_members_and_unknown_top_level_namespaces_are_reported() {
        for relative in [
            "forge/host.js",
            "editor/editor",
            "broker/broker",
            "node/node",
            "electron/electron",
            "host.js",
            "other/file",
            "resources/node",
            "licenses/electron.exe",
            "resources/runtime.exe",
        ] {
            let root = tempfile::tempdir().expect("temp");
            write_payload(root.path());
            let path = root.path().join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("legacy parent");
            }
            std::fs::write(&path, b"legacy").expect("legacy member");

            let PayloadHealth::Modified(issues) = verify(root.path()) else {
                panic!("legacy member was accepted: {relative}");
            };
            let namespace = relative.split('/').next().expect("namespace");
            assert!(issues.iter().any(|issue| {
                issue == &format!("unexpected: {namespace}")
                    || issue == &format!("unexpected: {relative}")
            }));
            assert_launch_admission_rejected(root.path());
        }
    }

    #[test]
    fn issue_reporting_remains_capped() {
        let root = tempfile::tempdir().expect("temp");
        write_payload(root.path());
        for index in 0..8 {
            std::fs::write(root.path().join(format!("unexpected-{index}")), b"extra")
                .expect("extra");
        }

        let PayloadHealth::Modified(issues) = verify(root.path()) else {
            panic!("unexpected files were accepted");
        };
        assert_eq!(issues.len(), 6);
        assert!(issues.last().is_some_and(|issue| issue.starts_with("and ")));
    }

    #[test]
    fn unreadable_and_unexpected_entries_are_not_healthy() {
        let root = tempfile::tempdir().expect("temp");
        write_payload(root.path());
        std::fs::remove_file(root.path().join(REQUIRED_PAYLOAD_FILES[0])).expect("remove");
        std::fs::create_dir(root.path().join(REQUIRED_PAYLOAD_FILES[0])).expect("directory");

        let PayloadHealth::Modified(issues) = verify(root.path()) else {
            panic!("unreadable entry was accepted");
        };
        assert!(issues.iter().any(|issue| {
            issue == &format!("unreadable: {}", REQUIRED_PAYLOAD_FILES[0])
                || issue == &format!("unexpected: {}", REQUIRED_PAYLOAD_FILES[0])
        }));
    }

    #[test]
    fn missing_manifest_is_unverifiable_not_healthy() {
        let root = tempfile::tempdir().expect("temp");
        std::fs::create_dir_all(root.path().join("bin")).expect("bin");
        std::fs::write(root.path().join(REQUIRED_PAYLOAD_FILES[0]), b"artisan").expect("ae");
        assert_eq!(verify(root.path()), PayloadHealth::Unverifiable);
        assert_launch_admission_rejected(root.path());
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
        assert_launch_admission_rejected(root.path());
    }

    #[test]
    fn unsafe_manifest_entries_are_rejected() {
        let root = tempfile::tempdir().expect("temp");
        write_payload(root.path());
        std::fs::write(
            root.path().join(PAYLOAD_MANIFEST_NAME),
            br#"{"format_version":1,"files":{"../ae":"00"}}"#,
        )
        .expect("unsafe manifest");

        let PayloadHealth::Modified(issues) = verify(root.path()) else {
            panic!("unsafe manifest entry was accepted");
        };
        assert!(issues.contains(&"invalid manifest entry: ../ae".to_owned()));
        assert_launch_admission_rejected(root.path());
    }
}
