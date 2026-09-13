//! Finite native Grok Build (`grok`) executable discovery and non-billable
//! readiness probe.
//!
//! This packet owns ONLY `modules/native_engine/src/grok/**` plus
//! `tests/native_engine/grok_*`. It deliberately touches no shared state:
//! no `lib.rs` registration (returned to the controller below), no
//! manifest/`BUILD` edits (none needed: `std`-only, no new dependencies),
//! no credential/runtime/frontend changes.
//!
//! Behavior mirrors `modules/engines/src/grok/engine.ts` over the shared
//! `MakeAcpEngine` core: default executable `"grok"`, `--version` version
//! reporting parsed with the TypeScript version regex, and the non-billable
//! `grok --no-auto-update models` auth probe whose output is authenticated
//! unless it matches not-authenticated/not-logged-in semantics. The
//! auth-method selection (`xai.api_key` when `XAI_API_KEY` is present, else
//! `cached_token`) is recorded as data for the later ACP runtime packet, not
//! executed here.
//!
//! The explicit `ARTISAN_GROK_EXECUTABLE` override is a native convention
//! mirroring the sibling Codex/Claude workers (`ARTISAN_CODEX_EXECUTABLE` in
//! `modules/engines/src/codex/executable.ts`).
//!
//! This packet never marks the engine runnable: it reports discovery and
//! readiness facts only. Registration needed from the controller:
//! `pub mod grok;` in `modules/native_engine/src/lib.rs`, plus
//! `[[test]] grok_discovery` / `[[test]] grok_probe` targets pointing at
//! `tests/native_engine/grok_*.rs` (Cargo `[[test]]` entries in
//! `modules/native_engine/Cargo.toml` and `rust_test` targets in a new
//! `modules/native_engine/Cargo.toml` following the `tests/cli` pattern).

pub mod discovery;
pub mod probe;

pub use discovery::{
    GROK_AUTH_PROBE_ARGS, GROK_BINARY_NAME, GROK_EXECUTABLE_ENV, GROK_VERSION_ARGS,
    GrokResolveSource, ResolvedGrokBinary, explicit_override, find_grok_on_path,
    parse_grok_version, resolve_grok_binary, resolve_live,
};
pub use probe::{
    BoundedChildOutput, DEFAULT_AUTH_TIMEOUT, DEFAULT_MAX_OUTPUT_BYTES, DEFAULT_VERSION_TIMEOUT,
    GROK_AUTH_METHOD_API_KEY, GROK_AUTH_METHOD_CACHED_TOKEN, GrokAuthMethod, GrokAuthState,
    GrokProbe, GrokProbeError, GrokProbeLimits, GrokProbePhase, MAX_PROBE_EXCERPT_CHARS,
    XAI_API_KEY_ENV, classify_auth_result, classify_auth_spawn, classify_version_output,
    is_authenticated_output, preferred_auth_method, probe_grok_readiness, probe_live_readiness,
    redact_probe_excerpt, run_bounded_command, select_auth_method,
};
