//! Credential manifest and bundle validation, split by concern: `manifest`
//! validates the manifest schema and bundles, and `dacl` owns Windows DACL
//! parsing, verification, and restriction.

mod dacl;
mod manifest;

#[cfg(all(test, windows))]
pub(super) use dacl::acl_diagnostic;
#[cfg(windows)]
pub(super) use dacl::{
    hidden_output, resolve_current_identity, restrict_directory_windows, restrict_file_windows,
    verify_windows_dacl,
};

pub(super) use manifest::{
    CredentialManifest, classify_capability_length, classify_certificate_length,
    validate_existing_bundle, validate_existing_identity_bundle,
};
