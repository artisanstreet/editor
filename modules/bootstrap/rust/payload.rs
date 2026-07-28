//! Per-version payload integrity manifest.
//!
//! `payload-manifest.json` sits at the root of `versions/<v>` and maps every
//! regular payload file (relative path, `/` separators) to its lowercase hex
//! SHA-256:
//!
//! ```json
//! { "format_version": 1, "files": { "bin/ae.exe": "<sha256>" } }
//! ```
//!
//! The staging step writes it once the extracted tree is final, so `ae doctor`
//! can detect post-install drift (for example a development build copied over
//! an installed payload). The verifying reader lives in
//! `modules/cli/rust/payload.rs`; both sides must stay format-compatible.

use std::{collections::BTreeMap, fs::File, path::Path};

use crate::{
    error::{BootstrapError, Result, io},
    install::hash_file,
};

pub const PAYLOAD_MANIFEST_NAME: &str = "payload-manifest.json";
pub const PAYLOAD_MANIFEST_FORMAT_VERSION: u8 = 1;

/// Writes `payload-manifest.json` at the payload root, covering every regular
/// file already present. Must run after the tree is final and before it is
/// activated as `versions/<v>`.
pub fn write_manifest(root: &Path) -> Result<()> {
    let mut files = BTreeMap::new();
    collect(root, "", &mut files)?;
    let path = root.join(PAYLOAD_MANIFEST_NAME);
    let mut file = File::create(&path).map_err(io(&path))?;
    serde_json::to_writer(
        &mut file,
        &serde_json::json!({
            "format_version": PAYLOAD_MANIFEST_FORMAT_VERSION,
            "files": files,
        }),
    )
    .map_err(BootstrapError::InvalidPayload)?;
    file.sync_all().map_err(io(&path))?;
    Ok(())
}

fn collect(directory: &Path, prefix: &str, files: &mut BTreeMap<String, String>) -> Result<()> {
    for entry in std::fs::read_dir(directory).map_err(io(directory))? {
        let entry = entry.map_err(io(directory))?;
        let path = entry.path();
        let name = entry.file_name().into_string().map_err(|name| {
            BootstrapError::Archive(format!("payload name is not Unicode: {name:?}"))
        })?;
        let relative = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        let file_type = entry.file_type().map_err(io(&path))?;
        if file_type.is_dir() {
            collect(&path, &relative, files)?;
        } else if file_type.is_file() {
            if relative != PAYLOAD_MANIFEST_NAME {
                files.insert(relative, hash_file(&path)?);
            }
        } else {
            // The archive extractor admits only regular files and directories.
            return Err(BootstrapError::Archive(format!(
                "payload entry is neither file nor directory: {relative}"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde::Deserialize;
    use tempfile::tempdir;

    use super::{PAYLOAD_MANIFEST_NAME, write_manifest};

    #[derive(Deserialize)]
    struct Manifest {
        format_version: u8,
        files: BTreeMap<String, String>,
    }

    #[test]
    fn manifest_covers_the_tree_with_relative_slash_paths_and_excludes_itself() {
        let root = tempdir().expect("temp");
        std::fs::create_dir_all(root.path().join("bin")).expect("bin");
        std::fs::write(root.path().join("bin").join("ae.exe"), b"artisan").expect("ae");
        std::fs::write(root.path().join("host.js"), b"host").expect("host");

        write_manifest(root.path()).expect("write manifest");
        // A second write must not list the manifest itself.
        write_manifest(root.path()).expect("rewrite manifest");

        let manifest: Manifest = serde_json::from_slice(
            &std::fs::read(root.path().join(PAYLOAD_MANIFEST_NAME)).expect("read"),
        )
        .expect("parse");
        assert_eq!(manifest.format_version, 1);
        assert_eq!(
            manifest.files.keys().collect::<Vec<_>>(),
            ["bin/ae.exe", "host.js"]
        );
        // Digest of b"artisan", matching the release contract representation.
        assert_eq!(
            manifest.files["bin/ae.exe"],
            "0b74ed7ff22b86fd0838fd29a78940a8d54377951e968867948a57b3e53646fc"
        );
        assert!(!manifest.files.contains_key(PAYLOAD_MANIFEST_NAME));
    }
}
