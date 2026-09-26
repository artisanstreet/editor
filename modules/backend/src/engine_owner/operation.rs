//! Private orchestration for the engine owner: the single owner task,
//! generation allocation, per-operation execution, and the quarantine tail.
//!
//! One task owns at most one active engine child at a time — its exact
//! [`tokio::process::Child`], the taken sole stdin lifeline writer, the
//! stderr counting state, the burned generation, and every cleanup decision.
//! Work arrives through a bounded channel and is processed strictly
//! sequentially; there is no per-job task, no parallel owner, and no
//! replacement child before an observed reap.
//!
//! Fixed precedence, re-checked at the top of every scheduling cycle:
//! owner shutdown or terminal state, abandonment or explicit cancellation,
//! the operation deadline, and only then completion sources. Once a cleanup
//! sequence starts it runs to completion regardless of those signals.
//!
//! P3 adds bounded child readiness parsing and a bounded authenticated
//! HTTP/1 health handshake after spawn. Readiness is exactly one
//! newline-terminated `{"url": "..."}` record capped via `cap + 1`; health
//! is one `GET /api/health` with `Basic base64(opencode:<secret>)` over a
//! Hyper `TokioIo<TcpStream>` connection configured with caller-supplied
//! `max_headers` and `max_buf_bytes` and body-bounded via `Limited`.

mod bootstrap;
mod claude;
mod codex;
mod core;
mod cursor;
mod failures;
mod grok;
mod lifecycle;
mod opencode;
mod owner;
mod turn_common;

// Vocabulary and handoff types re-exported unchanged for the rest of the
// engine owner and the `#[path]` suites.
#[allow(unused_imports)]
pub(crate) use self::core::{
    AcceptedCatalog, AcceptedLaunch, AcceptedPreflight, AcceptedTurn, CatalogOperationResult,
    EngineOperationError, EngineTurnResult, Execution, GenerationAllocator, HealthState, Job,
    LaunchAdmissionError, LaunchOutcome, LaunchResult, PreflightReap, PreflightReceipt,
    PreflightResult, PreparedSession, STEER_CHANNEL_CAPACITY, StartRefusal, SteerDelivery,
    SteerError, TurnResult,
};
#[cfg(test)]
pub(crate) use super::process::StartDiagnostic;

// Steer helpers re-exported so the `engine_owner::operation::*` paths keep
// resolving for the `#[path]` engine-owner suites.
#[cfg(test)]
#[allow(unused_imports)]
pub(crate) use self::claude::read_claude_line;
#[allow(unused_imports)]
pub(crate) use self::claude::service_claude_steer_delivery;
#[allow(unused_imports)]
pub(crate) use self::codex::{
    ack_codex_steer_response, service_codex_delivery, service_codex_steer_delivery,
};
// Shared Codex wire helpers live in the codex leaf module; re-exported so the
// `engine_owner::operation::*` paths keep resolving for the `#[path]`
// engine-owner suites.
#[allow(unused_imports)]
pub(crate) use super::codex::{
    codex_response_id_matches, codex_resumed_thread_id, codex_thread_id, codex_turn_id,
    is_codex_result_for,
};
// Owner entry points re-exported for `engine_owner::mod` and the seeded owner
// tests.
#[allow(unused_imports)]
pub(crate) use self::owner::{run_configured_owner, run_owner, run_owner_with_allocator};
