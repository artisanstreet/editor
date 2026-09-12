//! Grok [`EngineSocket`] adapter skeleton.
//!
//! Declares the provider-neutral socket adapter for the probe-certified Grok
//! launch capability
//! ([`super::super::grok::GrokLaunch`], the same type the owner's
//! `InternalLaunch::Grok` carries). The descriptor mirrors
//! `GrokEngineDescriptor` in `modules/engines/src/grok/engine.ts`; `probe`
//! reports the probed launch version without performing I/O; `open` is
//! deliberately unwired and returns a failed [`EngineOpenOutcome`] with
//! [`EngineOpenError::Unimplemented`].
//!
//! A later packet forwards [`EngineOpenInput`] into the existing owner
//! executor
//! [`execute_grok_turn`](super::super::operation::execute_grok_turn)
//! (`modules/backend/src/engine_owner/operation.rs`) and replaces this stub.
//! The live configured-turn dispatch constructs this adapter through
//! `socket::adapter_for`; Codex is the first engine wired through the
//! open/drive split.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use artisan_domain::{
    EngineCapabilityName, EngineCapabilityState, EngineDescriptor, EngineOpenError,
    EngineOpenFuture, EngineOpenInput, EngineOpenOutcome, EngineProbe, EngineSocket,
};

use super::super::consts::GROK_ENGINE_ID;
use super::super::grok::GrokLaunch;

/// Display name of the Grok adapter, mirroring
/// `GrokEngineDescriptor.display_name` in
/// `modules/engines/src/grok/engine.ts:52`.
const GROK_DISPLAY_NAME: &str = "Grok Build";

/// Exact transport spelling of the Grok ACP stdio wire, mirroring
/// `grok_transport` in `modules/engines/src/grok/engine.ts:6`.
///
/// No Rust const exists for it yet, so this skeleton defines the one copy;
/// the wiring packet may lift it into the shared consts without renaming the
/// value.
const GROK_TRANSPORT: &str = "grok-acp-stdio";

/// Grok adapter over one probe-certified launch capability.
///
/// Holds the same [`GrokLaunch`] the existing `execute_grok_turn` executor
/// receives through `InternalLaunch::Grok`; the launch is neither cloned nor
/// re-resolved here. Construct one per admitted Grok run once the wiring
/// packet lands.
pub(crate) struct GrokSocketAdapter<'a> {
    launch: &'a GrokLaunch,
}

impl<'a> GrokSocketAdapter<'a> {
    /// Wraps one probe-certified Grok launch capability for the socket seam.
    #[must_use]
    pub(crate) fn new(launch: &'a GrokLaunch) -> Self {
        Self { launch }
    }
}

impl EngineSocket for GrokSocketAdapter<'_> {
    /// Returns the exact Grok descriptor, including every capability state
    /// declared by the TypeScript adapter
    /// (`GrokEngineDescriptor.capabilities`,
    /// `modules/engines/src/grok/engine.ts:9-50`).
    fn descriptor(&self) -> EngineDescriptor {
        EngineDescriptor {
            id: GROK_ENGINE_ID.to_owned(),
            display_name: GROK_DISPLAY_NAME.to_owned(),
            transport: GROK_TRANSPORT.to_owned(),
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
                // `global_guidance` — unsupported: Grok Build discovers
                // AGENTS.md and its own global instructions natively.
                (
                    EngineCapabilityName::GlobalGuidance,
                    EngineCapabilityState::Unsupported,
                ),
                // `model_catalog` — unsupported: Grok models are supplied by
                // Artisan's curated catalog.
                (
                    EngineCapabilityName::ModelCatalog,
                    EngineCapabilityState::Unsupported,
                ),
                (
                    EngineCapabilityName::ModelSelection,
                    EngineCapabilityState::Supported,
                ),
                // `native_continuation` — unsupported: ACP does not guarantee
                // that a loaded session may change model identity.
                (
                    EngineCapabilityName::NativeContinuation,
                    EngineCapabilityState::Unsupported,
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
                    EngineCapabilityState::Experimental,
                ),
            ],
        }
    }

    /// Reports readiness from the probe-certified launch version.
    ///
    /// Placeholder until the probe wiring packet lands: no live probe runs
    /// here, so `ready` stays `false` and no terminal state is claimed. The
    /// version is real, read from the probe-certified capability.
    fn probe(&self) -> EngineProbe {
        EngineProbe {
            ready: false,
            version: self.launch.version().to_owned(),
            terminal_state: None,
        }
    }

    /// Fails closed until the open wiring packet forwards to
    /// `execute_grok_turn`.
    fn open(&self, _input: EngineOpenInput) -> EngineOpenFuture<'_> {
        Box::pin(std::future::ready(EngineOpenOutcome::failed(
            EngineOpenError::Unimplemented,
        )))
    }
}
