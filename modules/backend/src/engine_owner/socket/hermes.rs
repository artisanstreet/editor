//! Hermes [`EngineSocket`] adapter skeleton.
//!
//! Declares the provider-neutral socket adapter for the existing verified
//! Hermes service launch capability
//! ([`super::super::hermes::VerifiedHermesLaunch`], the same type the owner's
//! `InternalLaunch::Hermes` carries). The descriptor mirrors
//! `HermesEngineDescriptor` in `modules/engines/src/hermes/engine.ts`;
//! `probe` reports the probed launch version without performing I/O; `open`
//! is deliberately unwired and returns
//! [`EngineOpenError::Unimplemented`].
//!
//! The next packet forwards [`EngineOpenInput`] into the existing owner
//! executor
//! [`execute_hermes_turn`](super::super::operation::execute_hermes_turn)
//! (`modules/backend/src/engine_owner/operation.rs`) and replaces the
//! [`EngineSocket::open`] stub. Nothing calls this adapter yet.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use artisan_domain::{
    EngineCapabilityName, EngineCapabilityState, EngineDescriptor, EngineOpenError,
    EngineOpenInput, EngineOpenResult, EngineProbe, EngineSocket,
};

use super::super::consts::HERMES_ENGINE_ID;
use super::super::hermes::VerifiedHermesLaunch;

/// Display name of the Hermes adapter, mirroring
/// `HermesEngineDescriptor.display_name` in
/// `modules/engines/src/hermes/engine.ts:95`.
const HERMES_DISPLAY_NAME: &str = "Hermes";

/// Exact transport spelling of the Hermes JSON-RPC WebSocket wire, mirroring
/// `HermesEngineDescriptor.transport` in
/// `modules/engines/src/hermes/engine.ts:97`.
///
/// No Rust const exists for it yet, so this skeleton defines the one copy;
/// the wiring packet may lift it into the shared consts without renaming the
/// value.
const HERMES_TRANSPORT: &str = "hermes-jsonrpc-websocket";

/// Hermes adapter over one verified service launch capability.
///
/// Holds the same [`VerifiedHermesLaunch`] the existing `execute_hermes_turn`
/// executor receives through `InternalLaunch::Hermes`; the launch is neither
/// cloned nor re-resolved here. Construct one per admitted Hermes run once
/// the wiring packet lands.
pub(crate) struct HermesSocketAdapter {
    launch: VerifiedHermesLaunch,
}

impl HermesSocketAdapter {
    /// Wraps one verified Hermes service launch capability for the socket
    /// seam.
    #[must_use]
    pub(crate) fn new(launch: VerifiedHermesLaunch) -> Self {
        Self { launch }
    }
}

impl EngineSocket for HermesSocketAdapter {
    /// Returns the exact Hermes descriptor, including every capability state
    /// declared by the TypeScript adapter
    /// (`HermesEngineDescriptor.capabilities`,
    /// `modules/engines/src/hermes/engine.ts:52-93`).
    fn descriptor(&self) -> EngineDescriptor {
        EngineDescriptor {
            id: HERMES_ENGINE_ID.to_owned(),
            display_name: HERMES_DISPLAY_NAME.to_owned(),
            transport: HERMES_TRANSPORT.to_owned(),
            capabilities: vec![
                (
                    EngineCapabilityName::Approval,
                    EngineCapabilityState::Supported,
                ),
                // `auth` — unsupported: provider setup remains owned by the
                // installed Hermes profile.
                (
                    EngineCapabilityName::Auth,
                    EngineCapabilityState::Unsupported,
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
                    EngineCapabilityState::Experimental,
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
                    EngineCapabilityState::Supported,
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
                (
                    EngineCapabilityName::Subagents,
                    EngineCapabilityState::Experimental,
                ),
            ],
        }
    }

    /// Reports readiness from the probe-certified launch version.
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
    /// `execute_hermes_turn`.
    fn open(&self, input: EngineOpenInput) -> EngineOpenResult {
        let _ = input;
        Err(EngineOpenError::Unimplemented)
    }
}
