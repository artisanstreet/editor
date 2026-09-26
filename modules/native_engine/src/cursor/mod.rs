//! Finite native Cursor executable discovery and non-billable readiness
//! probe.
//!
//! This packet owns ONLY `modules/native_engine/src/cursor/**` plus
//! `tests/native_engine/cursor_*`. It deliberately touches no shared state:
//! no `lib.rs` registration (returned to the controller below), no
//! manifest/`BUILD` edits (none needed: `std`-only, no new dependencies),
//! no credential/runtime/frontend changes.
//!
//! Behavior mirrors `modules/engines/src/cursor/engine.ts` over the shared
//! `MakeAcpEngine` core: the `ARTISAN_CURSOR_EXECUTABLE` override convention
//! (shared with the Codex/Claude/Grok workers), `cursor-agent` then `agent`
//! installation/`PATH` resolution, `--version` reporting parsed with the
//! TypeScript version regex, and the non-billable `status` auth probe whose
//! output is authenticated unless it matches not-authenticated/not-logged-in
//! semantics. Model-resolution inputs (`reasoning_effort`, `speed`,
//! `permission_mode`), the `AE-PROVIDER-206` startup-failure shape, and the
//! `image` image-input mode are recorded as data for the later ACP runtime
//! packet, not executed here.
//!
//! This packet never marks the engine runnable: it reports discovery and
//! readiness facts only. Registration needed from the controller:
//! `pub mod cursor;` in `modules/native_engine/src/lib.rs`, plus
//! `[[test]] cursor_discovery` / `[[test]] cursor_probe` targets pointing at
//! `tests/native_engine/cursor_*.rs` (Cargo `[[test]]` entries in
//! `modules/native_engine/Cargo.toml` and `rust_test` targets in
//! `modules/native_engine/Cargo.toml` following the sibling engine pattern).

pub mod discovery;
pub mod model;
pub mod probe;

pub use discovery::{
    CURSOR_AUTH_PROBE_ARGS, CURSOR_BINARY_NAME, CURSOR_EXECUTABLE_ENV, CURSOR_VERSION_ARGS,
    CursorResolveSource, ResolvedCursorBinary, parse_cursor_version, resolve_live,
};
pub use model::{
    CURSOR_ARTISAN_CODE_UNAVAILABLE_MODEL, CURSOR_ENGINE_ID, CURSOR_IMAGE_INPUT, CursorAcpInputs,
    CursorModelInputs, CursorPermissionMode, CursorSpeed, CursorStartupFailure,
    MAX_UNAVAILABLE_MODEL_CHARS, classify_cursor_startup_failure, cursor_acp_args,
    resolve_cursor_model,
};
pub use probe::{
    BoundedChildOutput, CURSOR_AUTH_METHOD_LOGIN, CursorAuthMethod, CursorAuthState, CursorProbe,
    CursorProbeError, CursorProbeLimits, CursorProbePhase, DEFAULT_AUTH_TIMEOUT,
    DEFAULT_MAX_OUTPUT_BYTES, DEFAULT_VERSION_TIMEOUT, MAX_PROBE_EXCERPT_CHARS,
    classify_auth_result, classify_auth_spawn, classify_version_output, is_authenticated_output,
    probe_cursor_readiness, probe_live_readiness, redact_probe_excerpt, run_bounded_command,
    select_auth_method,
};
