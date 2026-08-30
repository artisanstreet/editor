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
    collections::BTreeMap,
    fs::File,
    path::{Component, Path},
};

use crate::{
    error::{InstallerError, Result, io},
    install::hash_file,
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

    use super::{PAYLOAD_MANIFEST_NAME, REQUIRED_PAYLOAD_FILES, write_manifest};

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
}
