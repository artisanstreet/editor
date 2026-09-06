//! Finite native Codex discovery and readiness probe.
//!
//! This directory owns the bounded, non-billable Codex surface only:
//! executable discovery, installed-version parsing, and CLI/account
//! readiness classification. The full app-server adapter (handshake,
//! `account/read` session, runs) belongs to a later packet and must not be
//! added here.
//!
//! The controller wires this module with:
//!
//! ```rust,ignore
//! pub mod codex;
//! ```
//!
//! placed alongside the existing `#[path]` modules in
//! `modules/native_engine/src/lib.rs`, plus a public re-export such as:
//!
//! ```rust,ignore
//! pub use codex::{CodexProbeError, CodexReadiness, resolve_codex_executable};
//! ```
//!
//! No new third-party dependencies are required; this module uses only
//! `std` and the crate's existing `serde_json`.

pub mod discovery;
pub mod probe;
pub mod version;

pub use discovery::{
    CODEX_EXECUTABLE_OVERRIDE_ENV, CODEX_FALLBACK_COMMAND, CODEX_WINGET_PACKAGE_DIR,
    CodexDiscoveryInput, codex_fallback_executable, codex_local_root, codex_winget_arch,
    codex_winget_executable, compare_codex_directory_names, is_windows_apps_path,
    resolve_codex_executable, resolve_codex_home, sort_codex_directory_names,
};
pub use probe::{
    CODEX_ACCOUNT_OUTPUT_BOUND_BYTES, CODEX_VERSION_OUTPUT_BOUND_BYTES, CODEX_VERSION_TIMEOUT,
    CodexAccountRead, CodexAccountType, CodexAuthState, CodexProbeError, CodexReadiness,
    CodexVersion, CodexVersionFixture, classify_codex_auth, classify_version_fixture,
    codex_readiness, parse_codex_account_read, run_codex_version, validate_codex_version_output,
};
pub use version::{
    CODEX_CONTINUATION_CLI_VERSION, CODEX_INITIALIZE_METHOD, CODEX_MINIMUM_CLI_VERSION,
    CODEX_PROTOCOL_VERSION, CODEX_TRANSPORT, compare_semantic_versions,
    is_continuation_verified_version, meets_minimum_version, parse_codex_continuation_version,
    parse_codex_version,
};
