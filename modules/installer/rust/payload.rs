//! Per-version payload integrity manifest.
//!
//! `payload-manifest.json` sits at the root of `versions/<v>` and maps every
//! regular payload file (relative path, `/` separators) to its lowercase hex
//! SHA-256:
//!
//! ```json
//! { "format_version": 1, "files": { "bin/ae.exe": "<sha256>", "bin/editor.exe": "<sha256>", "bin/forge.exe": "<sha256>", "bin/installer.exe": "<sha256>" } }
//! ```
//!
//! The staging step writes it once the extracted tree is final, so `ae doctor`
//! can detect post-install drift (for example a development build copied over
//! an installed payload). The verifying reader lives in
//! `modules/cli/rust/payload.rs`; both sides must stay format-compatible.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    path::{Component, Path},
};

use serde::Deserialize;

use crate::{
    error::{InstallerError, Result, io},
    install::hash_file,
    manifest::Artifact,
};

pub const PAYLOAD_MANIFEST_NAME: &str = "payload-manifest.json";
pub const PAYLOAD_MANIFEST_FORMAT_VERSION: u8 = 1;
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

/// Writes `payload-manifest.json` at the payload root, covering the four
/// required binaries and any non-executable resources or licenses. Must run
/// after the tree is final and before it is activated as `versions/<v>`.
pub fn write_manifest(root: &Path) -> Result<()> {
    let mut files = BTreeMap::new();
    collect(root, &mut files)?;
    for required in REQUIRED_PAYLOAD_FILES {
        if !files.contains_key(required) {
            return Err(invalid_layout(required));
        }
    }
    let path = root.join(PAYLOAD_MANIFEST_NAME);
    let mut file = File::create(&path).map_err(io(&path))?;
    serde_json::to_writer(
        &mut file,
        &serde_json::json!({
            "format_version": PAYLOAD_MANIFEST_FORMAT_VERSION,
            "files": files,
        }),
    )
    .map_err(InstallerError::InvalidPayload)?;
    file.sync_all().map_err(io(&path))?;
    Ok(())
}

/// The parsed form of a payload manifest written by [`write_manifest`].
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PayloadManifestDocument {
    format_version: u8,
    files: BTreeMap<String, String>,
}

fn read_manifest(root: &Path) -> Result<BTreeMap<String, String>> {
    let path = root.join(PAYLOAD_MANIFEST_NAME);
    let bytes = std::fs::read(&path).map_err(io(&path))?;
    let document: PayloadManifestDocument =
        serde_json::from_slice(&bytes).map_err(InstallerError::InvalidPayload)?;
    if document.format_version != PAYLOAD_MANIFEST_FORMAT_VERSION {
        return Err(InstallerError::Archive(format!(
            "unsupported payload manifest format {}",
            document.format_version
        )));
    }
    for relative in document.files.keys() {
        if !is_safe_relative(relative) || !is_payload_member(relative) {
            return Err(invalid_layout(relative));
        }
    }
    Ok(document.files)
}

/// Proves an existing `versions/<v>` tree is the tree the signed release
/// artifact produces.
///
/// A version directory on disk carries no signature of its own, so it is only
/// trusted after the signed artifact has been downloaded, its archive checksum
/// verified, and its extracted stage compared file by file with the existing
/// tree. The tree's own `payload-manifest.json` must cover exactly the signed
/// archive entries; any extra, missing, or modified file is refused.
pub(crate) fn verify_existing_against_stage(
    existing: &Path,
    verified_stage: &Path,
    artifact: &Artifact,
    version: &str,
) -> Result<()> {
    if !artifact
        .archive_entries
        .iter()
        .any(|entry| entry == PAYLOAD_MANIFEST_NAME)
    {
        return Err(unverified(
            version,
            "the signed artifact does not declare a payload manifest",
        ));
    }
    let expected: BTreeSet<&str> = artifact
        .archive_entries
        .iter()
        .map(String::as_str)
        .filter(|entry| *entry != PAYLOAD_MANIFEST_NAME)
        .collect();
    if expected.is_empty() {
        return Err(unverified(
            version,
            "the signed artifact declares no payload files",
        ));
    }
    let recorded =
        read_manifest(existing).map_err(|error| unverified(version, error.to_string()))?;
    if recorded.len() != expected.len()
        || !recorded.keys().all(|key| expected.contains(key.as_str()))
    {
        return Err(tampered(version));
    }
    for relative in &expected {
        let Some(recorded_digest) = recorded.get(*relative) else {
            return Err(tampered(version));
        };
        let actual = hash_file(&existing.join(relative)).map_err(|_| tampered(version))?;
        if !actual.eq_ignore_ascii_case(recorded_digest) {
            return Err(tampered(version));
        }
        let staged = hash_file(&verified_stage.join(relative))?;
        if actual != staged {
            return Err(tampered(version));
        }
    }
    if tree_has_unexpected_files(existing, &expected)
        .map_err(|error| unverified(version, error.to_string()))?
    {
        return Err(tampered(version));
    }
    Ok(())
}

fn unverified(version: &str, reason: impl Into<String>) -> InstallerError {
    InstallerError::UnverifiedRelease {
        version: version.to_owned(),
        reason: reason.into(),
    }
}

fn tampered(version: &str) -> InstallerError {
    InstallerError::TamperedRelease {
        version: version.to_owned(),
    }
}

fn is_payload_member(relative: &str) -> bool {
    REQUIRED_PAYLOAD_FILES.contains(&relative)
        || relative.split_once('/').is_some_and(|(directory, member)| {
            !member.is_empty() && OPTIONAL_PAYLOAD_DIRECTORIES.contains(&directory)
        })
}

/// Reports whether the tree contains anything that is neither the payload
/// manifest nor one of the signed archive entries. Directories are allowed
/// only when they can contain a signed entry, so an injected namespace or
/// symlink is refused.
fn tree_has_unexpected_files(existing: &Path, expected: &BTreeSet<&str>) -> Result<bool> {
    fn walk(directory: &Path, prefix: &str, expected: &BTreeSet<&str>) -> Result<bool> {
        for entry in std::fs::read_dir(directory).map_err(io(directory))? {
            let entry = entry.map_err(io(directory))?;
            let Ok(name) = entry.file_name().into_string() else {
                return Ok(true);
            };
            let relative = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            let file_type = entry.file_type().map_err(io(&entry.path()))?;
            if file_type.is_symlink() {
                return Ok(true);
            }
            if file_type.is_dir() {
                let nested_prefix = format!("{relative}/");
                if !expected
                    .iter()
                    .any(|candidate| candidate.starts_with(&nested_prefix))
                    || walk(&entry.path(), &relative, expected)?
                {
                    return Ok(true);
                }
            } else if !file_type.is_file()
                || (relative != PAYLOAD_MANIFEST_NAME && !expected.contains(relative.as_str()))
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
    walk(existing, "", expected)
}

fn collect(root: &Path, files: &mut BTreeMap<String, String>) -> Result<()> {
    for entry in std::fs::read_dir(root).map_err(io(root))? {
        let entry = entry.map_err(io(root))?;
        let path = entry.path();
        let name = entry.file_name().into_string().map_err(|name| {
            InstallerError::Archive(format!("payload name is not Unicode: {}", name.display()))
        })?;
        let file_type = entry.file_type().map_err(io(&path))?;
        match name.as_str() {
            PAYLOAD_MANIFEST_NAME if file_type.is_file() => {}
            "bin" if file_type.is_dir() => collect_bin(&path, files)?,
            directory
                if OPTIONAL_PAYLOAD_DIRECTORIES.contains(&directory) && file_type.is_dir() =>
            {
                collect_optional(&path, &name, files)?;
            }
            _ => return Err(invalid_layout(&name)),
        }
    }
    Ok(())
}

fn collect_bin(directory: &Path, files: &mut BTreeMap<String, String>) -> Result<()> {
    for entry in std::fs::read_dir(directory).map_err(io(directory))? {
        let entry = entry.map_err(io(directory))?;
        let path = entry.path();
        let name = entry.file_name().into_string().map_err(|name| {
            InstallerError::Archive(format!("payload name is not Unicode: {}", name.display()))
        })?;
        let relative = format!("bin/{name}");
        let file_type = entry.file_type().map_err(io(&path))?;
        if !file_type.is_file() || !REQUIRED_PAYLOAD_FILES.contains(&relative.as_str()) {
            return Err(invalid_layout(&relative));
        }
        files.insert(relative, hash_file(&path)?);
    }
    Ok(())
}

fn collect_optional(
    directory: &Path,
    prefix: &str,
    files: &mut BTreeMap<String, String>,
) -> Result<()> {
    for entry in std::fs::read_dir(directory).map_err(io(directory))? {
        let entry = entry.map_err(io(directory))?;
        let path = entry.path();
        let name = entry.file_name().into_string().map_err(|name| {
            InstallerError::Archive(format!("payload name is not Unicode: {}", name.display()))
        })?;
        let relative = format!("{prefix}/{name}");
        if !is_safe_relative(&relative) || is_forbidden_legacy_member(&relative) {
            return Err(invalid_layout(&relative));
        }
        let file_type = entry.file_type().map_err(io(&path))?;
        if file_type.is_dir() {
            collect_optional(&path, &relative, files)?;
        } else if file_type.is_file() {
            if !is_non_executable_file(&path, &relative).map_err(io(&path))? {
                return Err(invalid_layout(&relative));
            }
            files.insert(relative, hash_file(&path)?);
        } else {
            return Err(invalid_layout(&relative));
        }
    }
    Ok(())
}

fn invalid_layout(relative: &str) -> InstallerError {
    InstallerError::Archive(format!("invalid payload layout: {relative}"))
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

fn is_forbidden_legacy_member(relative: &str) -> bool {
    relative.rsplit('/').next().is_some_and(|name| {
        FORBIDDEN_LEGACY_NAMES
            .iter()
            .any(|forbidden| name.eq_ignore_ascii_case(forbidden))
    })
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

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, path::Path};

    use serde::Deserialize;
    use tempfile::tempdir;

    use super::{
        PAYLOAD_MANIFEST_NAME, REQUIRED_PAYLOAD_FILES, verify_existing_against_stage,
        write_manifest,
    };
    use crate::{
        error::InstallerError,
        manifest::{ArchiveFormat, Artifact},
    };

    #[derive(Deserialize)]
    struct Manifest {
        format_version: u8,
        files: BTreeMap<String, String>,
    }

    fn write_valid_payload(root: &Path) {
        std::fs::create_dir_all(root.join("bin")).expect("bin");
        for (index, relative) in REQUIRED_PAYLOAD_FILES.iter().enumerate() {
            std::fs::write(root.join(*relative), format!("binary-{index}"))
                .expect("required binary");
        }
    }

    fn add_member(root: &Path, relative: &str, bytes: &[u8]) {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("member parent");
        }
        std::fs::write(path, bytes).expect("member");
    }

    #[test]
    fn manifest_covers_four_binaries_and_optional_tree_deterministically() {
        let root = tempdir().expect("temp");
        write_valid_payload(root.path());
        add_member(root.path(), "resources/nested/config.json", b"config");
        add_member(root.path(), "licenses/third-party.txt", b"license");

        write_manifest(root.path()).expect("write manifest");
        let first = std::fs::read(root.path().join(PAYLOAD_MANIFEST_NAME)).expect("first read");
        write_manifest(root.path()).expect("rewrite manifest");
        let second = std::fs::read(root.path().join(PAYLOAD_MANIFEST_NAME)).expect("second read");
        assert_eq!(first, second);

        let manifest: Manifest = serde_json::from_slice(&second).expect("parse");
        assert_eq!(manifest.format_version, 1);
        assert_eq!(
            manifest
                .files
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            [
                REQUIRED_PAYLOAD_FILES[0],
                REQUIRED_PAYLOAD_FILES[2],
                REQUIRED_PAYLOAD_FILES[3],
                REQUIRED_PAYLOAD_FILES[1],
                "licenses/third-party.txt",
                "resources/nested/config.json",
            ]
            .into_iter()
            .collect::<Vec<_>>()
        );
        assert!(!manifest.files.contains_key(PAYLOAD_MANIFEST_NAME));
        for required in REQUIRED_PAYLOAD_FILES {
            assert!(manifest.files.contains_key(required));
        }
    }

    #[test]
    fn writer_rejects_each_missing_required_binary() {
        for missing in REQUIRED_PAYLOAD_FILES {
            let root = tempdir().expect("temp");
            write_valid_payload(root.path());
            std::fs::remove_file(root.path().join(missing)).expect("remove binary");

            assert!(write_manifest(root.path()).is_err(), "accepted {missing}");
            assert!(!root.path().join(PAYLOAD_MANIFEST_NAME).exists());
        }
    }

    #[test]
    fn writer_rejects_extra_bin_members_and_wrong_extension() {
        let root = tempdir().expect("temp");
        write_valid_payload(root.path());
        add_member(root.path(), "bin/extra", b"extra");
        assert!(write_manifest(root.path()).is_err());

        let root = tempdir().expect("temp");
        write_valid_payload(root.path());
        std::fs::create_dir(root.path().join("bin").join("nested")).expect("nested bin directory");
        assert!(write_manifest(root.path()).is_err());

        let root = tempdir().expect("temp");
        write_valid_payload(root.path());
        let wrong_extension = if cfg!(windows) {
            "bin/ae"
        } else {
            "bin/ae.exe"
        };
        add_member(root.path(), wrong_extension, b"wrong");
        assert!(write_manifest(root.path()).is_err());
    }

    #[test]
    fn writer_rejects_legacy_members_and_unknown_top_level_namespaces() {
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
            let root = tempdir().expect("temp");
            write_valid_payload(root.path());
            add_member(root.path(), relative, b"legacy");
            assert!(write_manifest(root.path()).is_err(), "accepted {relative}");
        }

        let root = tempdir().expect("temp");
        write_valid_payload(root.path());
        std::fs::create_dir(root.path().join("forge")).expect("empty legacy directory");
        assert!(write_manifest(root.path()).is_err());
    }

    fn signed_artifact(extra_entries: &[&str]) -> Artifact {
        let mut archive_entries: Vec<String> = REQUIRED_PAYLOAD_FILES
            .iter()
            .map(|entry| (*entry).to_owned())
            .collect();
        archive_entries.extend(extra_entries.iter().map(|entry| (*entry).to_owned()));
        archive_entries.push(PAYLOAD_MANIFEST_NAME.to_owned());
        Artifact {
            id: "windows-x64".to_owned(),
            platform: "windows".to_owned(),
            architecture: "x64".to_owned(),
            libc: None,
            format: ArchiveFormat::Zip,
            file_name: "artisan-editor-versioned-payload.zip".to_owned(),
            size: 1,
            sha256: "0".repeat(64),
            archive_entries,
        }
    }

    fn write_verified_pair(root: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
        let existing = root.join("existing");
        let stage = root.join("stage");
        write_valid_payload(&existing);
        write_valid_payload(&stage);
        write_manifest(&existing).expect("existing manifest");
        write_manifest(&stage).expect("stage manifest");
        (existing, stage)
    }

    #[test]
    fn existing_tree_matching_the_signed_stage_is_accepted() {
        let directory = tempdir().expect("temp");
        let (existing, stage) = write_verified_pair(directory.path());
        verify_existing_against_stage(&existing, &stage, &signed_artifact(&[]), "1.2.3")
            .expect("verified existing release");
    }

    #[test]
    fn optional_payload_members_are_compared_too() {
        let directory = tempdir().expect("temp");
        let (existing, stage) = write_verified_pair(directory.path());
        for root in [&existing, &stage] {
            add_member(root, "resources/nested/config.json", b"config");
            write_manifest(root).expect("manifest with resource");
        }
        verify_existing_against_stage(
            &existing,
            &stage,
            &signed_artifact(&["resources/nested/config.json"]),
            "1.2.3",
        )
        .expect("verified optional member");
    }

    #[test]
    fn tampered_existing_tree_is_refused_even_with_a_refreshed_manifest() {
        let directory = tempdir().expect("temp");
        let (existing, stage) = write_verified_pair(directory.path());
        std::fs::write(existing.join(REQUIRED_PAYLOAD_FILES[0]), b"tampered")
            .expect("tamper binary");
        // A dev-staged or hostile tree can refresh its own manifest. It still
        // cannot match the tree extracted from the signed artifact.
        write_manifest(&existing).expect("refreshed manifest");

        let error =
            verify_existing_against_stage(&existing, &stage, &signed_artifact(&[]), "1.2.3")
                .expect_err("tampered tree");
        assert!(matches!(
            error,
            InstallerError::TamperedRelease { version } if version == "1.2.3"
        ));
    }

    #[test]
    fn added_file_in_existing_tree_is_refused() {
        let directory = tempdir().expect("temp");
        let (existing, stage) = write_verified_pair(directory.path());
        add_member(&existing, "bin/injected.dll", b"payload");

        let error =
            verify_existing_against_stage(&existing, &stage, &signed_artifact(&[]), "1.2.3")
                .expect_err("extra file");
        assert!(matches!(error, InstallerError::TamperedRelease { .. }));
    }

    #[test]
    fn missing_payload_manifest_is_unverifiable() {
        let directory = tempdir().expect("temp");
        let (existing, stage) = write_verified_pair(directory.path());
        std::fs::remove_file(existing.join(PAYLOAD_MANIFEST_NAME)).expect("remove manifest");

        let error =
            verify_existing_against_stage(&existing, &stage, &signed_artifact(&[]), "1.2.3")
                .expect_err("missing manifest");
        assert!(matches!(
            error,
            InstallerError::UnverifiedRelease { version, .. } if version == "1.2.3"
        ));
    }

    #[test]
    fn payload_manifest_must_cover_exactly_the_signed_entries() {
        let directory = tempdir().expect("temp");
        let (existing, stage) = write_verified_pair(directory.path());

        let error = verify_existing_against_stage(
            &existing,
            &stage,
            &signed_artifact(&["resources/not-installed.json"]),
            "1.2.3",
        )
        .expect_err("manifest that misses a signed entry");
        assert!(matches!(error, InstallerError::TamperedRelease { .. }));
    }

    #[test]
    fn unsafe_payload_manifest_entries_are_unverifiable() {
        let directory = tempdir().expect("temp");
        let (existing, stage) = write_verified_pair(directory.path());
        std::fs::write(
            existing.join(PAYLOAD_MANIFEST_NAME),
            br#"{"format_version":1,"files":{"../ae":"00"}}"#,
        )
        .expect("unsafe manifest");

        let error =
            verify_existing_against_stage(&existing, &stage, &signed_artifact(&[]), "1.2.3")
                .expect_err("unsafe manifest");
        assert!(matches!(error, InstallerError::UnverifiedRelease { .. }));
    }
}
