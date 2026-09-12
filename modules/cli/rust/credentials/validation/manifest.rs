use std::{fs, path::Path};

use serde::{Deserialize, Serialize};

use super::super::certificates::{validate_cert_sans, validate_key_matches_cert};
#[cfg(unix)]
use super::super::storage::check_file_mode;
use super::super::storage::{
    MAX_CAPABILITY_BYTES, MAX_CAPABILITY_READ_BYTES, MAX_CERTIFICATE_BYTES,
    MAX_CERTIFICATE_READ_BYTES, MAX_MANIFEST_BYTES, MAX_MANIFEST_READ_BYTES, MAX_PRIVATE_KEY_BYTES,
    MAX_PRIVATE_KEY_READ_BYTES, acl_diagnostic, check_ancestors_all, is_safe_filename,
    metadata_is_symlink_or_reparse, read_private_material, validate_private_directory,
};
use super::super::{ForgeCredentialError, ForgeCredentialPaths};
#[cfg(all(test, windows))]
use super::dacl::acl_diagnostic;
#[cfg(windows)]
use super::dacl::{resolve_current_identity, verify_windows_dacl};

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct CredentialManifest {
    pub(crate) schema: String,
    pub(crate) version: u64,
    pub(crate) bootstrap_capability: String,
    pub(crate) certificate_chain: Vec<String>,
    pub(crate) private_key: String,
}

fn validate_manifest_bytes(
    bytes: &[u8],
    _paths: &ForgeCredentialPaths,
) -> Result<CredentialManifest, ForgeCredentialError> {
    let manifest: CredentialManifest =
        serde_json::from_slice(bytes).map_err(|_| ForgeCredentialError::ManifestMalformed)?;
    if manifest.schema != "artisan-forge-credentials-v1" {
        return Err(ForgeCredentialError::ManifestSchema);
    }
    if manifest.version != 1 {
        return Err(ForgeCredentialError::ManifestVersion);
    }
    if manifest.bootstrap_capability != "bootstrap-capability.bin" {
        return Err(ForgeCredentialError::ManifestTraversal);
    }
    if manifest.certificate_chain != vec!["localhost-leaf.der".to_string()] {
        return Err(ForgeCredentialError::ManifestTraversal);
    }
    if manifest.private_key != "localhost-key.pkcs8.der" {
        return Err(ForgeCredentialError::ManifestTraversal);
    }
    for name in std::iter::once(&manifest.bootstrap_capability)
        .chain(manifest.certificate_chain.iter())
        .chain(std::iter::once(&manifest.private_key))
    {
        if !is_safe_filename(name) {
            return Err(ForgeCredentialError::ManifestTraversal);
        }
    }
    Ok(manifest)
}

fn classify_manifest_length(length: usize) -> Result<(), ForgeCredentialError> {
    if length <= MAX_MANIFEST_BYTES {
        Ok(())
    } else {
        Err(ForgeCredentialError::ManifestMalformed)
    }
}

pub(crate) fn classify_capability_length(
    length: usize,
    path: &Path,
) -> Result<(), ForgeCredentialError> {
    if length == MAX_CAPABILITY_BYTES {
        Ok(())
    } else {
        Err(ForgeCredentialError::InvalidCapability {
            path: path.to_path_buf(),
        })
    }
}

pub(crate) fn classify_certificate_length(length: usize) -> Result<(), ForgeCredentialError> {
    if (1..=MAX_CERTIFICATE_BYTES).contains(&length) {
        Ok(())
    } else {
        Err(ForgeCredentialError::InvalidCertificate)
    }
}

fn classify_private_key_length(length: usize) -> Result<(), ForgeCredentialError> {
    if (1..=MAX_PRIVATE_KEY_BYTES).contains(&length) {
        Ok(())
    } else {
        Err(ForgeCredentialError::InvalidCertificate)
    }
}

pub(crate) fn validate_existing_bundle(
    paths: &ForgeCredentialPaths,
) -> Result<bool, ForgeCredentialError> {
    let manifest_path = paths.manifest_path();
    let capability_path = paths.capability_path();
    let cert_path = &paths.certificate_paths()[0];
    let key_path = paths.private_key_path();

    let files = [manifest_path, capability_path, cert_path, key_path];
    let mut exists = Vec::new();
    let mut missing = Vec::new();
    for file in &files {
        match fs::symlink_metadata(file) {
            Ok(meta) if metadata_is_symlink_or_reparse(&meta) => {
                return Err(ForgeCredentialError::UnsafePath((*file).to_path_buf()));
            }
            Ok(meta) if meta.is_dir() => {
                return Err(ForgeCredentialError::UnsafePath((*file).to_path_buf()));
            }
            Ok(meta) if meta.is_file() => {
                check_ancestors_all(file, true)?;
                exists.push(*file);
            }
            Ok(_) => return Err(ForgeCredentialError::UnsafePath((*file).to_path_buf())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                missing.push(*file);
            }
            Err(_) => {
                return Err(ForgeCredentialError::Io {
                    context: "inspect bundle file",
                    path: (*file).to_path_buf(),
                });
            }
        }
    }
    if missing.len() == files.len() {
        return Ok(false);
    }
    if !missing.is_empty() {
        return Err(ForgeCredentialError::PartialBundle);
    }
    for file in &exists {
        #[cfg(unix)]
        check_file_mode(file)?;
        #[cfg(windows)]
        {
            acl_diagnostic!(acl_diagnostic::stage("BundleAclVerification"));
            let identity = resolve_current_identity()?;
            verify_windows_dacl(file, &identity.sid)?;
        }
        check_ancestors_all(file, true)?;
    }
    let manifest_bytes = read_private_material(
        paths,
        manifest_path,
        MAX_MANIFEST_READ_BYTES,
        ForgeCredentialError::ManifestMalformed,
    )?;
    classify_manifest_length(manifest_bytes.len())?;
    validate_manifest_bytes(&manifest_bytes, paths)?;
    let cap_bytes = read_private_material(
        paths,
        capability_path,
        MAX_CAPABILITY_READ_BYTES,
        ForgeCredentialError::InvalidCapability {
            path: capability_path.to_path_buf(),
        },
    )?;
    classify_capability_length(cap_bytes.len(), capability_path)?;
    let cert_der = read_private_material(
        paths,
        cert_path,
        MAX_CERTIFICATE_READ_BYTES,
        ForgeCredentialError::InvalidCertificate,
    )?;
    classify_certificate_length(cert_der.len())?;
    let key_bytes = read_private_material(
        paths,
        key_path,
        MAX_PRIVATE_KEY_READ_BYTES,
        ForgeCredentialError::InvalidCertificate,
    )?;
    classify_private_key_length(key_bytes.len())?;
    validate_cert_sans(&cert_der)?;
    validate_key_matches_cert(&key_bytes, &cert_der)?;
    let _ = rustls::crypto::ring::default_provider();
    Ok(true)
}

pub(crate) fn validate_existing_identity_bundle(
    paths: &ForgeCredentialPaths,
) -> Result<bool, ForgeCredentialError> {
    let directory = paths.credentials_dir();
    check_ancestors_all(&directory, false)?;
    match fs::symlink_metadata(&directory) {
        Ok(_) => validate_private_directory(&directory)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(_) => {
            return Err(ForgeCredentialError::Io {
                context: "inspect credentials directory",
                path: directory,
            });
        }
    }
    let manifest_path = paths.manifest_path();
    let cert_path = &paths.certificate_paths()[0];
    let key_path = paths.private_key_path();
    let files = [manifest_path, cert_path.as_path(), key_path];
    let mut exists = Vec::new();
    let mut missing = Vec::new();
    for file in &files {
        match fs::symlink_metadata(file) {
            Ok(meta) if metadata_is_symlink_or_reparse(&meta) => {
                return Err(ForgeCredentialError::UnsafePath((*file).to_path_buf()));
            }
            Ok(meta) if meta.is_dir() => {
                return Err(ForgeCredentialError::UnsafePath((*file).to_path_buf()));
            }
            Ok(meta) if meta.is_file() => {
                check_ancestors_all(file, true)?;
                exists.push(*file);
            }
            Ok(_) => return Err(ForgeCredentialError::UnsafePath((*file).to_path_buf())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                missing.push(*file);
            }
            Err(_) => {
                return Err(ForgeCredentialError::Io {
                    context: "inspect identity file",
                    path: (*file).to_path_buf(),
                });
            }
        }
    }
    if missing.len() == files.len() {
        return Ok(false);
    }
    if !missing.is_empty() {
        return Err(ForgeCredentialError::PartialBundle);
    }
    for file in &exists {
        #[cfg(unix)]
        check_file_mode(file)?;
        #[cfg(windows)]
        {
            acl_diagnostic!(acl_diagnostic::stage("BundleAclVerification"));
            let identity = resolve_current_identity()?;
            verify_windows_dacl(file, &identity.sid)?;
        }
        check_ancestors_all(file, true)?;
    }
    let manifest_bytes = read_private_material(
        paths,
        manifest_path,
        MAX_MANIFEST_READ_BYTES,
        ForgeCredentialError::ManifestMalformed,
    )?;
    classify_manifest_length(manifest_bytes.len())?;
    validate_manifest_bytes(&manifest_bytes, paths)?;
    let cert_der = read_private_material(
        paths,
        cert_path,
        MAX_CERTIFICATE_READ_BYTES,
        ForgeCredentialError::InvalidCertificate,
    )?;
    classify_certificate_length(cert_der.len())?;
    let key_bytes = read_private_material(
        paths,
        key_path,
        MAX_PRIVATE_KEY_READ_BYTES,
        ForgeCredentialError::InvalidCertificate,
    )?;
    classify_private_key_length(key_bytes.len())?;
    validate_cert_sans(&cert_der)?;
    validate_key_matches_cert(&key_bytes, &cert_der)?;
    let _ = rustls::crypto::ring::default_provider();
    Ok(true)
}
