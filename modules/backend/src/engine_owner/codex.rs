//! Finite Codex owner runtime over `codex app-server --stdio`.
//!
//! Spawns the verified Codex CLI as a JSONL stdio app-server, runs the
//! `initialize` handshake with capability flags plus the opt-out
//! notification list, creates one thread from the typed [`CodexSettings`]
//! (or reopens the stored provider thread through the X3 continuation gate),
//! starts exactly one turn, normalizes streaming frames onto the shared S1a
//! observation vocabulary (`TextDelta` / `Terminal` plus cumulative token
//! usage) and the rich activity vocabulary (`Activity` carrying one validated
//! domain observation per reasoning-summary, tool, terminal-activity, file,
//! search, or plan frame, mirroring
//! `modules/engines/src/codex/normalizer.ts`), and supports steer/follow-up,
//! interrupt/cancel, and
//! close with exit classification. An inactivity deadline stalls a silent
//! turn; child-custody teardown reuses the owner `process` contract and
//! quarantines on unobserved reaps.
//!
//! The wire boundary is typed: inbound frames decode into [`CodexEvent`]
//! through bounded `serde_json::Value` extraction. Only validated identities
//! and text cross the boundary: approval and question frames become
//! [`CodexApprovalRequest`] / [`CodexQuestionRequest`] and map onto domain
//! `ApprovalRequest` / `QuestionInput` constructors for the durable A-approve
//! resolve path. Deny lands with no side effect while the turn continues;
//! allow answers through the same durable path.
//!
//! Subagent frames are tracked as discovery only and never adopt the root
//! turn: child-thread activity produces no root `TextDelta`. Terminal mapping
//! preserves interruption (external kill) vs cancel vs failure distinctly.
//!
//! X3 notes: continuation resumes provider-owned state only (never invented
//! checkpoints) through `thread/resume` behind
//! [`check_codex_native_continuation`]; usage is collected best-effort
//! alongside the turn (token frames project to cumulative [`RunUsageReport`]
//! rows, `account/rateLimits/read` windows classify into diagnostics) and
//! never blocks it; teardown terminates the whole process group so no codex
//! grandchild holding a pipe is orphaned; interrupted runs replay the durable
//! prefix on resume without duplicating provider effects.

#![forbid(unsafe_code)]

mod adapter;
mod continuation;
mod protocol;
pub(crate) use adapter::*;
pub(crate) use continuation::*;
pub(crate) use protocol::*;
