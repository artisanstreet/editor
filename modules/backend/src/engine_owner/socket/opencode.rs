//! `OpenCode2` [`EngineSocket`] adapter skeleton.
//!
//! Declares the provider-neutral socket adapter for the existing verified
//! `OpenCode2` profile launch capability
//! ([`artisan_native_engine::VerifiedOpenCode2ProfileLaunch`], the same type
//! the owner's `InternalLaunch::Verified` carries). The descriptor mirrors
//! `OpenCode2EngineDescriptor` in `modules/engines/src/opencode2/engine.ts`;
//! `probe` reports the certified launch version without performing I/O;
//! `open` is deliberately unwired and returns
//! [`EngineOpenError::Unimplemented`].
//!
//! The next packet forwards [`EngineOpenInput`] into the existing owner
//! executors
//! [`execute_authorized_configured_turn`](super::super::operation::execute_authorized_configured_turn)
//! with
//! [`create_configured_session`](super::super::operation::create_configured_session)
//! (`modules/backend/src/engine_owner/operation.rs`) and replaces the
//! [`EngineSocket::open`] stub. Nothing calls this adapter yet.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use artisan_domain::{
    EngineCapabilityName, EngineCapabilityState, EngineDescriptor, EngineOpenError,
    EngineOpenInput, EngineOpenResult, EngineProbe, EngineSocket,
};
use artisan_native_engine::VerifiedOpenCode2ProfileLaunch;

use super::super::consts::OPENCODE2_ENGINE_ID;

/// Display name of the `OpenCode2` adapter, mirroring
/// `OpenCode2EngineDescriptor.display_name` in
/// `modules/engines/src/opencode2/engine.ts:100`.
const OPENCODE2_DISPLAY_NAME: &str = "OpenCode";

/// Exact transport spelling of the `OpenCode2` HTTP/SSE wire, mirroring
/// `OpenCode2EngineDescriptor.transport` in
/// `modules/engines/src/opencode2/engine.ts:102`.
///
/// No Rust const exists for it yet, so this skeleton defines the one copy;
/// the wiring packet may lift it into the shared consts without renaming the
/// value.
const OPENCODE2_TRANSPORT: &str = "opencode2-http-sse";

/// `OpenCode2` adapter over one verified profile launch capability.
///
/// Holds the same [`VerifiedOpenCode2ProfileLaunch`] the existing
/// `execute_authorized_configured_turn` executor receives through
/// `InternalLaunch::Verified`; the launch is neither cloned nor re-resolved
/// here. Construct one per admitted `OpenCode2` run once the wiring packet
/// lands.
pub(crate) struct OpenCode2SocketAdapter {
    launch: VerifiedOpenCode2ProfileLaunch,
}

impl OpenCode2SocketAdapter {
    /// Wraps one verified `OpenCode2` profile launch capability for the socket
    /// seam.
    #[must_use]
    pub(crate) fn new(launch: VerifiedOpenCode2ProfileLaunch) -> Self {
        Self { launch }
    }
}

impl EngineSocket for OpenCode2SocketAdapter {
    /// Returns the exact `OpenCode2` descriptor, including every capability
    /// state declared by the TypeScript adapter
    /// (`OpenCode2EngineDescriptor.capabilities`,
    /// `modules/engines/src/opencode2/engine.ts:60-98`).
    fn descriptor(&self) -> EngineDescriptor {
        EngineDescriptor {
            id: OPENCODE2_ENGINE_ID.to_owned(),
            display_name: OPENCODE2_DISPLAY_NAME.to_owned(),
            transport: OPENCODE2_TRANSPORT.to_owned(),
            capabilities: vec![
                (
                    EngineCapabilityName::Approval,
                    EngineCapabilityState::Supported,
                ),
                (
                    EngineCapabilityName::Auth,
                    EngineCapabilityState::Experimental,
                ),
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
                    EngineCapabilityState::Supported,
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
                    EngineCapabilityState::Supported,
                ),
                (
                    EngineCapabilityName::RawFrames,
                    EngineCapabilityState::Supported,
                ),
                (
                    EngineCapabilityName::Resume,
                    EngineCapabilityState::Experimental,
                ),
                (
                    EngineCapabilityName::Start,
                    EngineCapabilityState::Supported,
                ),
                (
                    EngineCapabilityName::Steer,
                    EngineCapabilityState::Supported,
                ),
                // `subagents` — unsupported: native subagents are denied until
                // child permissions are proven not to widen the parent.
                (
                    EngineCapabilityName::Subagents,
                    EngineCapabilityState::Unsupported,
                ),
            ],
        }
    }

    /// Reports readiness from the certified launch version.
    ///
    /// Placeholder until the probe wiring packet lands: no live probe runs
    /// here, so `ready` stays `false` and no terminal state is claimed. The
    /// version is real, read from the certified capability.
    fn probe(&self) -> EngineProbe {
        EngineProbe {
            ready: false,
            version: self.launch.version().to_owned(),
            terminal_state: None,
        }
    }

    /// Fails closed until the open wiring packet forwards to
    /// `execute_authorized_configured_turn` / `create_configured_session`.
    fn open(&self, input: EngineOpenInput) -> EngineOpenResult {
        let _ = input;
        Err(EngineOpenError::Unimplemented)
    }
}
