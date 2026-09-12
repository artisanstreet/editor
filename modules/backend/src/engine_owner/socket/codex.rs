//! Codex [`EngineSocket`] adapter skeleton.
//!
//! Declares the provider-neutral socket adapter for the existing verified
//! Codex launch capability
//! ([`artisan_native_engine::VerifiedCodexLaunch`], the same type the owner's
//! `InternalLaunch::Codex` carries). The descriptor mirrors
//! `CodexEngineDescriptor` in `modules/engines/src/codex/engine.ts`; `probe`
//! reports the verified launch version without performing I/O; `open` is
//! deliberately unwired and returns
//! [`EngineOpenError::Unimplemented`].
//!
//! The next packet forwards [`EngineOpenInput`] into the existing owner
//! executor
//! [`execute_codex_turn`](super::super::operation::execute_codex_turn)
//! (`modules/backend/src/engine_owner/operation.rs`) and replaces the
//! [`EngineSocket::open`] stub. The live configured-turn dispatch constructs this adapter through `socket::adapter_for`; `open` stays unimplemented until the per-engine open/drive split lands.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use artisan_domain::{
    EngineCapabilityName, EngineCapabilityState, EngineDescriptor, EngineOpenError,
    EngineOpenInput, EngineOpenResult, EngineProbe, EngineSocket,
};
use artisan_native_engine::VerifiedCodexLaunch;

use super::super::consts::CODEX_ENGINE_ID;

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
/// this skeleton defines the one copy; the wiring packet may lift it into
/// the shared consts without renaming the value.
const CODEX_TRANSPORT: &str = "stdio-jsonl";

/// Codex adapter over one verified launch capability.
///
/// Holds the same [`VerifiedCodexLaunch`] the existing `execute_codex_turn`
/// executor receives through `InternalLaunch::Codex`; the launch is neither
/// cloned nor re-resolved here. Construct one per admitted Codex run once
/// the wiring packet lands.
pub(crate) struct CodexSocketAdapter<'a> {
    launch: &'a VerifiedCodexLaunch,
}

impl<'a> CodexSocketAdapter<'a> {
    /// Wraps one verified Codex launch capability for the socket seam.
    #[must_use]
    pub(crate) fn new(launch: &'a VerifiedCodexLaunch) -> Self {
        Self { launch }
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
    /// Placeholder until the probe wiring packet lands: no live probe runs
    /// here, so `ready` stays `false` and no terminal state is claimed. The
    /// version is real, read from the verified capability.
    fn probe(&self) -> EngineProbe {
        EngineProbe {
            ready: false,
            version: self.launch.version().to_owned(),
            terminal_state: None,
        }
    }

    /// Fails closed until the open wiring packet forwards to
    /// `execute_codex_turn`.
    fn open(&self, input: EngineOpenInput) -> EngineOpenResult {
        let _ = input;
        Err(EngineOpenError::Unimplemented)
    }
}
