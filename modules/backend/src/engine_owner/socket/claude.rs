//! Claude [`EngineSocket`] adapter skeleton.
//!
//! Declares the provider-neutral socket adapter for the existing verified
//! Claude launch capability
//! ([`artisan_native_engine::VerifiedClaudeLaunch`], the same type the owner's
//! `InternalLaunch::Claude` carries). The descriptor mirrors
//! `ClaudeEngineDescriptor` in `modules/engines/src/claude/descriptor.ts`;
//! `probe` reports the verified launch version without performing I/O; `open`
//! is deliberately unwired and returns
//! [`EngineOpenError::Unimplemented`].
//!
//! The next packet forwards [`EngineOpenInput`] into the existing owner
//! executor
//! [`execute_claude_turn`](super::super::operation::execute_claude_turn)
//! (`modules/backend/src/engine_owner/operation.rs`) and replaces the
//! [`EngineSocket::open`] stub. The live configured-turn dispatch constructs this adapter through `socket::adapter_for`; `open` stays unimplemented until the per-engine open/drive split lands.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use artisan_domain::{
    EngineCapabilityName, EngineCapabilityState, EngineDescriptor, EngineOpenError,
    EngineOpenInput, EngineOpenResult, EngineProbe, EngineSocket,
};
use artisan_native_engine::VerifiedClaudeLaunch;

use super::super::consts::CLAUDE_ENGINE_ID;

/// Display name of the Claude adapter, mirroring
/// `ClaudeEngineDescriptor.display_name` in
/// `modules/engines/src/claude/descriptor.ts:58`.
const CLAUDE_DISPLAY_NAME: &str = "Claude";

/// Exact transport spelling of the Claude CLI stream-json wire, mirroring
/// `claude_transport` in `modules/engines/src/claude/descriptor.ts:4`.
///
/// `artisan_native_engine::CLAUDE_TRANSPORT` already carries the same value;
/// this skeleton keeps its own copy self-contained, and the wiring packet may
/// lift it into the shared consts without renaming the value.
const CLAUDE_TRANSPORT: &str = "claude-cli-stream-json";

/// Claude adapter over one verified launch capability.
///
/// Holds the same [`VerifiedClaudeLaunch`] the existing `execute_claude_turn`
/// executor receives through `InternalLaunch::Claude`; the launch is neither
/// cloned nor re-resolved here. Construct one per admitted Claude run once
/// the wiring packet lands.
pub(crate) struct ClaudeSocketAdapter<'a> {
    launch: &'a VerifiedClaudeLaunch,
}

impl<'a> ClaudeSocketAdapter<'a> {
    /// Wraps one verified Claude launch capability for the socket seam.
    #[must_use]
    pub(crate) fn new(launch: &'a VerifiedClaudeLaunch) -> Self {
        Self { launch }
    }
}

impl EngineSocket for ClaudeSocketAdapter<'_> {
    /// Returns the exact Claude descriptor, including every capability state
    /// declared by the TypeScript adapter
    /// (`ClaudeEngineDescriptor.capabilities`,
    /// `modules/engines/src/claude/descriptor.ts:9-56`).
    fn descriptor(&self) -> EngineDescriptor {
        EngineDescriptor {
            id: CLAUDE_ENGINE_ID.to_owned(),
            display_name: CLAUDE_DISPLAY_NAME.to_owned(),
            transport: CLAUDE_TRANSPORT.to_owned(),
            capabilities: vec![
                (
                    EngineCapabilityName::Approval,
                    EngineCapabilityState::Supported,
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
                // `global_guidance` — unsupported: Claude Code reads its own
                // global CLAUDE.md natively; no mirror is wired.
                (
                    EngineCapabilityName::GlobalGuidance,
                    EngineCapabilityState::Unsupported,
                ),
                // `model_catalog` — unsupported: Claude model inventory is
                // supplied by Artisan's curated catalog.
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
                    EngineCapabilityState::Supported,
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
                    EngineCapabilityState::Supported,
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
                    EngineCapabilityState::Experimental,
                ),
                (
                    EngineCapabilityName::Subagents,
                    EngineCapabilityState::Supported,
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
    /// `execute_claude_turn`.
    fn open(&self, input: EngineOpenInput) -> EngineOpenResult {
        let _ = input;
        Err(EngineOpenError::Unimplemented)
    }
}
