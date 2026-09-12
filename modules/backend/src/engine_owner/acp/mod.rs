//! Finite ACP shared-transport core A1: stdio lifecycle plus framing.
//!
//! This leaf owns the native JSON-RPC-over-stdio transport spoken directly on
//! the wire: piped-`stdio` spawn with no shell, a bounded `initialize`
//! handshake with timeout, `session/new` and `session/load`, the streaming
//! update-loop framing with byte and line bounds, `cancel`/`close` teardown
//! with kill-then-reap and quarantine on failure, exit classification
//! (`interruption` vs `failure` vs `cancel`), and an inactivity deadline
//! measured after the last received frame. Every bound is caller-supplied
//! through [`AcpBounds`]; nothing here hardcodes a transport tuning value.
//!
//! The core is called from per-engine dispatch arms and adds no tasks: one
//! `EngineOwner` task and queue stays the authority for admission, custody,
//! and quarantine. Spawning mirrors the `process.rs` custody contract
//! (piped stdio, `kill_on_drop`, whole-job termination on Windows, close
//! lifeline first, bounded wait, `start_kill`, observed reap or retained
//! custody) without forking its OpenCode-specific recipe or types.
//!
//! Envelopes are typed at the boundary ([`AcpEnvelope`]); provider error text
//! is never retained (only the numeric code), update payloads cross as the
//! explicitly redacted [`WireUpdate`] boundary type for the A2 bridges to
//! interpret, and image payloads pass through as typed [`PromptPart`] blocks
//! per the row's [`ImageMode`], never as file paths. The TypeScript
//! `@agentclientprotocol/sdk` is behavior evidence only; this transport
//! speaks the wire directly and takes no SDK dependency.
//!
//! Explicit non-goals for A1: permission/elicitation bridges (A2 packet),
//! probe harness (A3), dispatcher admission, catalog, frontend, and any
//! engine beyond the grok and cursor rows ([`GROK_ACP`], [`CURSOR_ACP`]).
//!
//! `GrokSettings`/`CursorSettings` do not exist yet, so the row arg builders
//! take the explicit [`LaunchArgs`] params mirroring `GrokAcpArgs` and
//! `CursorAcpArgs` from the TypeScript evidence.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::wildcard_imports)]
// A1 has no in-crate callers yet: per-engine dispatch arms land in a later
// packet. Every item below is covered by the inline fixture-agent tests.
#![allow(dead_code)]

mod definition;
mod runtime;
mod wire;

pub(crate) use definition::*;
pub(crate) use runtime::*;
pub(crate) use wire::*;

/// Wire protocol version this core accepts, mirroring the TypeScript
/// `Acp.PROTOCOL_VERSION` evidence. The A3 probe harness re-validates this
/// against the pinned SDK before any dispatcher trusts it.
pub(crate) const ACP_PROTOCOL_VERSION: u32 = 1;

/// Client identity sent in `initialize` params, mirroring the TypeScript
/// evidence. A later packet should source the version from crate metadata.
pub(crate) const ACP_CLIENT_NAME: &str = "Artisan Editor";
pub(crate) const ACP_CLIENT_VERSION: &str = "0.2.2";

pub(crate) const METHOD_INITIALIZE: &str = "initialize";
pub(crate) const METHOD_AUTHENTICATE: &str = "authenticate";
pub(crate) const METHOD_SESSION_NEW: &str = "session/new";
pub(crate) const METHOD_SESSION_LOAD: &str = "session/load";
pub(crate) const METHOD_SESSION_PROMPT: &str = "session/prompt";
pub(crate) const METHOD_SESSION_CANCEL: &str = "session/cancel";
pub(crate) const METHOD_SESSION_UPDATE: &str = "session/update";

// ---------------------------------------------------------------------------
// Fixture-agent tests: a fake agent speaking JSON-RPC over stdio
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
