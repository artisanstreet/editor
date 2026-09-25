//! Finite C1 Cursor runtime on the ACP core: definition plus dispatch arms,
//! not runnable yet.
//!
//! Spawns nothing here. This leaf owns the native Cursor definition row over
//! the shared A1 transport core plus the A2 bridges: typed [`CursorSettings`]
//! derived from the durable [`CursorSelection`](artisan_domain::CursorSelection),
//! the model resolver (effort appended unless suffixed, `-fast` handling,
//! bracket passthrough), the args builder (`--mode ask` on read-only,
//! `--force` mapping, then `acp`), the `AE-PROVIDER-206` startup classifier
//! with model capture, the `cursor/ask_question` and `cursor/create_plan`
//! plan-approval extension mapping, and image-block mode. It extends the ACP
//! core with the cursor definition row and never forks it: spawning,
//! framing, handshake, session, update-loop, and teardown behavior stay in
//! `super::acp`, and permission/elicitation bridging stays in
//! `super::acp_bridges`.
//!
//! One `EngineOwner` task and queue stays the authority for admission,
//! custody, and quarantine; the dispatch arm in `super::operation` and the
//! dispatcher branch in `crate::native_run_dispatch` add only new match arms
//! beside the X1 Codex arm. The actual `cursor-agent` CLI is the only
//! executable this packet names; no fixture binary and no raw JSON cross into
//! the domain.
//!
//! Explicit non-goals for C3: catalog flag flip, frontend selection, the live
//! dashboard usage read and model inventory, the probe/authority launch, and
//! any engine beyond the cursor row.
//!
//! Cursor model rows come only from `crate::model_discovery::cursor`, which
//! lists what the installed CLI reports for this account; no curated list
//! exists. No thinking, speed, cost, or image-input value is inferred when
//! the provider did not report it. Usage stays non-billable and never starts
//! a run.
//!
//! C3 notes: continuation resumes provider-owned state only (never invented
//! checkpoints) through `session/load` behind
//! [`check_cursor_native_continuation`]; usage is collected best-effort
//! alongside the turn (prompt-result and `usage_update` frames project to
//! cumulative [`RunUsageReport`](artisan_domain::RunUsageReport) rows per the TypeScript ACP disclosure,
//! dashboard quota windows classify into diagnostics) and never blocks it;
//! teardown terminates the whole process group so no cursor grandchild
//! holding a pipe is orphaned; interrupted runs replay the durable prefix on
//! resume without duplicating provider effects.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::wildcard_imports)]
// C3 wires the continuation gate into dispatch and the owner fence; the live
// pump, dashboard read, and catalog merge still belong to later packets, so
// the usage projectors await their first live caller. The finite dispatch
// arm fails closed before spawning, and the fixture tests in this module
// plus `tests/backend/engine_owner_cursor.rs` prove the wire shape.
// Every item is covered by those tests.
#![allow(dead_code)]

mod adapter;
mod interaction;
mod usage;

pub(crate) use adapter::*;
#[allow(unused_imports)]
pub(crate) use interaction::*;
#[allow(unused_imports)]
pub(crate) use usage::*;

#[cfg(test)]
use super::consts::CURSOR_ENGINE_ID;

/// Converts cursor's known pre-session model rejection to Artisan's stable
/// model code, mirroring `ClassifyCursorStartupFailure` in
/// `modules/engines/src/cursor/engine.ts`: `Cannot use this model: X` maps to
/// `AE-PROVIDER-206` carrying the captured model name `X`.
#[cfg(test)]
pub(crate) use artisan_native_engine::cursor::classify_cursor_startup_failure;

// ---------------------------------------------------------------------------
// Fixture tests: cursor-shaped ACP args over the shared transport core
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
