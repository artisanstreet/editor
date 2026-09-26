//! Finite native Codex discovery and readiness probe.
//!
//! Bounded, non-billable surface only (the executable itself is the
//! Forge-managed generation, see `crate::engine_core`): installed version
//! parsing, `codex --version` readiness, and the app-server
//! `initialize` + `account/read` + shutdown probe. The full run adapter
//! (threads, turns, approvals) belongs to a later packet.
//!
//! The controller wires this module with `pub mod codex;` alongside the
//! existing `#[path]` modules in `modules/native_engine/src/lib.rs`. No new
//! third-party dependencies are required.

#[allow(clippy::module_name_repetitions)]
pub mod probe;
pub mod process;
#[allow(clippy::module_name_repetitions)]
pub mod session;
#[allow(clippy::module_name_repetitions)]
pub mod version;

pub use probe::{
    CODEX_ACCOUNT_OUTPUT_BOUND_BYTES, CODEX_VERSION_OUTPUT_BOUND_BYTES, CODEX_VERSION_TIMEOUT,
    CodexAccountRead, CodexAccountType, CodexAuthState, CodexProbeError, CodexReadiness,
    CodexVersion, classify_codex_auth, codex_readiness, parse_codex_account_read,
    run_codex_version, validate_codex_version_output,
};
pub use session::{
    CODEX_APP_SERVER_ARGS, CODEX_MAX_INBOUND_ENVELOPES, CODEX_OPT_OUT_NOTIFICATION_METHODS,
    CODEX_SESSION_OUTPUT_BOUND_BYTES, CODEX_SESSION_TIMEOUT, CodexAccountProbe,
    CodexClientIdentity, CodexServerInfo, CodexSessionProbeInput, ServerEnvelope,
    await_response_id, decode_server_envelope, make_account_read_request, make_initialize_request,
    probe_codex_account, validate_initialize_result,
};
pub use version::{
    CODEX_CONTINUATION_CLI_VERSION, CODEX_INITIALIZE_METHOD, CODEX_MINIMUM_CLI_VERSION,
    CODEX_PROTOCOL_VERSION, CODEX_TRANSPORT, compare_semantic_versions,
    is_continuation_verified_version, meets_minimum_version, parse_codex_continuation_version,
    parse_codex_version,
};
