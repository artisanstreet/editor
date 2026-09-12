//! Provider-neutral engine-socket adapters.
//!
//! This module is the backend half of the `EngineSocket` migration: the
//! domain [`EngineSocket`](artisan_domain::EngineSocket) seam, implemented per
//! provider over the same verified launch capability the owner executors
//! already receive, plus the immutable per-attempt [`SocketTurnContext`] the
//! open phase applies. The live configured-turn dispatch asks [`adapter_for`]
//! for the engine identity, and the Codex configured turn calls the same
//! adapter's `open` so the spawn and preflight handshake run through the seam
//! instead of through a direct `InternalLaunch` match.
//!
//! Adapters borrow the admitted launch and the turn context: nothing is
//! cloned or re-resolved, and the executor keeps custody. `open` is a real
//! object-safe async operation for Codex; the engines whose wiring packets
//! have not landed keep total adapters that fail closed with
//! [`EngineOpenError::Unimplemented`](artisan_domain::EngineOpenError::Unimplemented)
//! and perform no I/O.

#![forbid(unsafe_code)]

pub(crate) mod claude;
pub(crate) mod codex;
pub(crate) mod codex_session;
pub(crate) mod cursor;
pub(crate) mod grok;
pub(crate) mod opencode;

use std::sync::Arc;

use artisan_database::ThreadEngineSettings;
use artisan_domain::EngineSocket;
use artisan_transport::CancelHandle;
use tokio::time::Instant;

use super::InternalLaunch;
use super::{EngineBounds, EngineLimits};

/// Immutable per-attempt context handed to a socket adapter.
///
/// One adapter is constructed per admitted configured turn, so its open phase
/// applies exactly the persisted runtime budgets, the whole-attempt deadline,
/// and the caller's cancellation signals the drive phase would have applied.
pub(crate) struct SocketTurnContext<'a> {
    /// Immutable settings snapshot for the admitted turn.
    pub(crate) settings: &'a ThreadEngineSettings,
    /// Persisted phase budgets.
    pub(crate) limits: EngineLimits,
    /// Persisted byte/count bounds.
    pub(crate) bounds: EngineBounds,
    /// Whole-attempt deadline shared with the drive phase.
    pub(crate) attempt_deadline: Instant,
    /// Owner-wide shutdown signal.
    pub(crate) shutdown: &'a Arc<CancelHandle>,
    /// Per-turn cancellation signal.
    pub(crate) control: &'a Arc<CancelHandle>,
}

/// Borrows one admitted launch as its provider-neutral socket adapter.
///
/// The mapping is total: the test-only fixture lane gets an adapter that
/// reports the fixture identity and keeps the configured executor fallback
/// in the caller. `context` is consumed only by the Codex adapter, whose open
/// phase applies the persisted budgets and cancellation signals.
#[must_use]
pub(crate) fn adapter_for<'a>(
    launch: &'a InternalLaunch,
    context: SocketTurnContext<'a>,
) -> Box<dyn EngineSocket + 'a> {
    match launch {
        InternalLaunch::Verified(verified) => {
            Box::new(opencode::OpenCode2SocketAdapter::new(verified))
        }
        InternalLaunch::Codex(verified) => {
            Box::new(codex::CodexSocketAdapter::new(verified, context))
        }
        InternalLaunch::Claude(verified) => Box::new(claude::ClaudeSocketAdapter::new(verified)),
        InternalLaunch::Grok(launch) => Box::new(grok::GrokSocketAdapter::new(launch)),
        InternalLaunch::Cursor(launch) => Box::new(cursor::CursorSocketAdapter::new(launch)),
        #[cfg(test)]
        InternalLaunch::Fixture(_) => Box::new(FixtureSocketAdapter),
    }
}

/// Identity-only adapter for the test-only fixture launch.
///
/// The fixture executor is the configured `OpenCode2` lane; the adapter
/// exists so [`adapter_for`] is total and the live dispatch never needs an
/// `Option`.
#[cfg(test)]
pub(crate) struct FixtureSocketAdapter;

#[cfg(test)]
impl EngineSocket for FixtureSocketAdapter {
    fn descriptor(&self) -> artisan_domain::EngineDescriptor {
        artisan_domain::EngineDescriptor {
            id: "fixture".to_owned(),
            display_name: "Fixture".to_owned(),
            transport: "in-process".to_owned(),
            capabilities: Vec::new(),
        }
    }

    fn probe(&self) -> artisan_domain::EngineProbe {
        artisan_domain::EngineProbe {
            ready: false,
            version: "fixture".to_owned(),
            terminal_state: None,
        }
    }

    fn open(
        &self,
        _input: artisan_domain::EngineOpenInput,
    ) -> artisan_domain::EngineOpenFuture<'_> {
        Box::pin(std::future::ready(
            artisan_domain::EngineOpenOutcome::failed(
                artisan_domain::EngineOpenError::Unimplemented,
            ),
        ))
    }
}

// ---------------------------------------------------------------------------
// Private cfg(test) open witnesses.
// ---------------------------------------------------------------------------

#[cfg(test)]
thread_local! {
    static SOCKET_OPEN_COUNT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Records one wired adapter `open` entry (test-only).
#[cfg(test)]
pub(crate) fn record_socket_open_for_tests() {
    SOCKET_OPEN_COUNT.with(|count| count.set(count.get() + 1));
}

/// Resets the wired adapter `open` witness (test-only).
#[cfg(test)]
pub(crate) fn reset_socket_open_count_for_tests() {
    SOCKET_OPEN_COUNT.with(|count| count.set(0));
}

/// Reads the wired adapter `open` witness (test-only).
#[cfg(test)]
pub(crate) fn socket_open_count_for_tests() -> u64 {
    SOCKET_OPEN_COUNT.with(std::cell::Cell::get)
}
