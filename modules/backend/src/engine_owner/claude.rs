//! Finite Claude owner runtime over `claude -p --output-format stream-json`.
//!
//! Spawns the verified Claude Code CLI as a stream-JSON stdio process, writes
//! the first user message over stdin, captures the `system/init` session
//! identity behind the bind authorization gate, normalizes streaming frames
//! onto the shared S1a observation vocabulary (`TextDelta` / `Terminal`;
//! usage and title stay later packets), and supports steer/follow-up,
//! cancel, and close with exit classification. An inactivity deadline stalls
//! a silent turn; child-custody teardown reuses the owner `process` contract
//! and quarantines on unobserved reaps.
//!
//! The wire boundary is typed: inbound lines decode into [`ClaudeEvent`]
//! through bounded `serde_json::Value` extraction. Only validated identities
//! and text cross the boundary: `control_request` permission frames become
//! [`ClaudeApprovalRequest`] and map onto domain `ApprovalRequest`
//! constructors for the durable A-approve resolve path, while
//! `AskUserQuestion` permission frames are lifted out of the approval path
//! and canonicalized as [`ClaudeQuestionRequest`] (answering a question *is*
//! the authorization; gating it behind a second approval leaves the CLI
//! blocked on a terminal dialog Artisan never renders, per
//! `modules/engines/src/claude/cli-engine.ts`). Deny lands with no side
//! effect while the turn continues; allow answers through the same durable
//! path.
//!
//! Stream text phases are preserved verbatim on the typed event
//! (`unspecified` for streamed deltas, `commentary` for assistant text that
//! accompanies tool uses, mirroring the TypeScript normalizer); both fold to
//! `TextDelta` on the shared vocabulary. Buffered assistant frames project
//! their ordered content (text and thinking) plus at most one usage sample;
//! their text is authoritative and settles the streamed deltas of the same
//! message through the [`ClaudeTextLedger`], so each block lands exactly once.
//! When the launch requested `--thinking-display summarized`, thinking
//! blocks carry public summary prose that the [`ClaudeThinkingTracker`]
//! projects onto the shared reasoning-summary observations, one item per
//! provider message and content-block index; signatures never become text
//! and unknown display semantics project nothing. Encrypted-thinking
//! estimates (`system/thinking_tokens`) stay tracker plumbing, never root
//! text. Subagent lifecycle frames emit validated [`Observation::Subagent`]
//! rows (state `Discovered`, root plus agent thread identities) and child
//! transcript frames (`parent_tool_use_id`) project into validated
//! [`Observation::SubagentTranscript`] rows carrying renderer-safe message
//! content only; neither ever reaches the root turn. Terminal mapping
//! preserves interruption (external kill / EOF before `result`) vs cancel vs
//! failure distinctly.
//!
//! Rows accumulate in the tracker until drained: the fixture driver proves
//! emission plus sequencing here, and the dispatcher packet wires the drain
//! into the live pump beside the text channel.
//!
//! L3 notes: continuation resumes provider-owned state only (never invented
//! checkpoints) by reopening the stored native session through `--resume`
//! behind [`check_claude_native_continuation`]; usage is collected
//! best-effort alongside the turn (assistant usage gauges project to
//! cumulative [`RunUsageReport`] rows with a replacing context gauge, terminal
//! totals project without one) and never blocks it; the `/usage` CLI buckets
//! map to diagnostics with clamped percents and kind classification without
//! inventing quota; the generated session title is captured best-effort from
//! the transcript at the terminal fence into the terminal `summary_title`;
//! teardown terminates the whole process group so no claude grandchild
//! holding a pipe is orphaned; interrupted runs replay the durable prefix on
//! resume without duplicating provider effects.
//!
//! Later packets own: model inventory, catalog flag flip, frontend selection,
//! and dispatcher persistence of the captured title and quota diagnostics.

#![forbid(unsafe_code)]

mod adapter;
mod content;
mod launch;
mod protocol;
#[cfg(test)]
mod quota;
mod text;
mod thinking;
mod usage;
pub(crate) use adapter::*;
#[cfg(test)]
pub(crate) use content::*;
pub(crate) use launch::*;
pub(crate) use protocol::*;
#[cfg(test)]
pub(crate) use quota::*;
pub(crate) use usage::*;
