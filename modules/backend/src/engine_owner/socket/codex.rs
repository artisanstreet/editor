//! Codex [`EngineSocket`] adapter.
//!
//! Declares the provider-neutral socket adapter for the existing verified
//! Codex launch capability
//! ([`artisan_native_engine::VerifiedCodexLaunch`], the same type the owner's
//! `InternalLaunch::Codex` carries). The descriptor mirrors
//! `CodexEngineDescriptor` in `modules/engines/src/codex/engine.ts`; `probe`
//! reports the verified launch version without performing I/O.
//!
//! `open` is the real Codex open phase: it spawns `codex app-server --stdio`
//! under the owner custody contract, performs the `initialize` /
//! `initialized` / `thread/start` (or gated `thread/resume`) handshake with
//! the persisted budgets and cancellation signals from [`SocketTurnContext`],
//! and returns the opened session that the configured drive path
//! (`super::super::operation::execute_codex_turn`) consumes without spawning
//! a second child.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use artisan_domain::{
    EngineCapabilityName, EngineCapabilityState, EngineDescriptor, EngineOpenFuture,
    EngineOpenInput, EngineProbe, EngineSocket,
};
use artisan_native_engine::VerifiedCodexLaunch;

use super::super::consts::CODEX_ENGINE_ID;
use super::SocketTurnContext;
use super::codex_session::open_codex_session;

/// Display name of the Codex adapter, mirroring
/// `CodexEngineDescriptor.display_name` in
/// `modules/engines/src/codex/engine.ts`.
const CODEX_DISPLAY_NAME: &str = "Codex";

/// Exact transport spelling of the Codex app-server wire, mirroring
/// `CodexTransportMetadata.transport` in
/// `modules/engines/src/codex/protocol.ts`.
///
/// The owner spawns exactly `codex app-server --stdio`
/// (see `super::super::process::spawn_codex_engine`), which is the JSONL
/// stdio transport this spelling names. No Rust const exists for it yet, so
/// this adapter defines the one copy; a later packet may lift it into the
/// shared consts without renaming the value.
const CODEX_TRANSPORT: &str = "stdio-jsonl";

/// Codex adapter over one verified launch capability and turn context.
///
/// Holds the same [`VerifiedCodexLaunch`] the existing `execute_codex_turn`
/// executor receives through `InternalLaunch::Codex`; the launch is neither
/// cloned nor re-resolved here. The [`SocketTurnContext`] carries the
/// persisted budgets, attempt deadline, and cancellation signals the open
/// phase applies.
pub(crate) struct CodexSocketAdapter<'a> {
    launch: &'a VerifiedCodexLaunch,
    context: SocketTurnContext<'a>,
}

impl<'a> CodexSocketAdapter<'a> {
    /// Wraps one verified Codex launch capability for the socket seam.
    #[must_use]
    pub(crate) fn new(launch: &'a VerifiedCodexLaunch, context: SocketTurnContext<'a>) -> Self {
        Self { launch, context }
    }
}

impl EngineSocket for CodexSocketAdapter<'_> {
    /// Returns the exact Codex descriptor, including every capability state
    /// declared by the TypeScript adapter.
    fn descriptor(&self) -> EngineDescriptor {
        EngineDescriptor {
            id: CODEX_ENGINE_ID.to_owned(),
            display_name: CODEX_DISPLAY_NAME.to_owned(),
            transport: CODEX_TRANSPORT.to_owned(),
            capabilities: vec![
                (
                    EngineCapabilityName::Approval,
                    EngineCapabilityState::Experimental,
                ),
                (EngineCapabilityName::Auth, EngineCapabilityState::Supported),
                (
                    EngineCapabilityName::Cancel,
                    EngineCapabilityState::Supported,
                ),
                (
                    EngineCapabilityName::Close,
                    EngineCapabilityState::Supported,
                ),
                (
                    EngineCapabilityName::Events,
                    EngineCapabilityState::Supported,
                ),
                (
                    EngineCapabilityName::GlobalGuidance,
                    EngineCapabilityState::Supported,
                ),
                (
                    EngineCapabilityName::ModelCatalog,
                    EngineCapabilityState::Unsupported,
                ),
                (
                    EngineCapabilityName::ModelSelection,
                    EngineCapabilityState::Supported,
                ),
                (
                    EngineCapabilityName::NativeContinuation,
                    EngineCapabilityState::Experimental,
                ),
                (
                    EngineCapabilityName::NativeTools,
                    EngineCapabilityState::Experimental,
                ),
                (
                    EngineCapabilityName::Probe,
                    EngineCapabilityState::Supported,
                ),
                (
                    EngineCapabilityName::Question,
                    EngineCapabilityState::Experimental,
                ),
                (
                    EngineCapabilityName::RawFrames,
                    EngineCapabilityState::Supported,
                ),
                (
                    EngineCapabilityName::Resume,
                    EngineCapabilityState::Supported,
                ),
                (
                    EngineCapabilityName::Start,
                    EngineCapabilityState::Supported,
                ),
                (
                    EngineCapabilityName::Steer,
                    EngineCapabilityState::Supported,
                ),
                (
                    EngineCapabilityName::Subagents,
                    EngineCapabilityState::Experimental,
                ),
            ],
        }
    }

    /// Reports readiness from the verified launch version.
    ///
    /// No live probe runs here, so `ready` stays `false` and no terminal
    /// state is claimed. The version is real, read from the verified
    /// capability.
    fn probe(&self) -> EngineProbe {
        EngineProbe {
            ready: false,
            version: self.launch.version().to_owned(),
            terminal_state: None,
        }
    }

    /// Runs the real Codex open phase on the boxed object-safe future.
    fn open(&self, input: EngineOpenInput) -> EngineOpenFuture<'_> {
        #[cfg(test)]
        super::record_socket_open_for_tests();
        Box::pin(open_codex_session(self.launch, &self.context, input))
    }
}
