//! Cursor [`EngineSocket`] adapter skeleton.
//!
//! Declares the provider-neutral socket adapter for the finite C1 cursor
//! launch capability
//! ([`super::super::cursor::CursorLaunch`], the same type the owner's
//! `InternalLaunch::Cursor` carries). The descriptor mirrors
//! `CursorEngineDescriptor` in `modules/engines/src/cursor/engine.ts`;
//! `probe` reports the launch version (the C1 unprobed sentinel) without
//! performing I/O; `open` is deliberately unwired and returns a failed
//! [`EngineOpenOutcome`] with [`EngineOpenError::Unimplemented`].
//!
//! A later packet forwards [`EngineOpenInput`] into the existing owner
//! executor
//! [`execute_cursor_turn`](super::super::operation::execute_cursor_turn)
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

use super::super::consts::CURSOR_ENGINE_ID;
use super::super::cursor::CursorLaunch;

/// Display name of the Cursor adapter, mirroring
/// `CursorEngineDescriptor.display_name` in
/// `modules/engines/src/cursor/engine.ts:58`.
const CURSOR_DISPLAY_NAME: &str = "Cursor";

/// Exact transport spelling of the Cursor ACP stdio wire, mirroring
/// `cursor_transport` in `modules/engines/src/cursor/engine.ts:12`.
///
/// No Rust const exists for it yet, so this skeleton defines the one copy;
/// the wiring packet may lift it into the shared consts without renaming the
/// value.
const CURSOR_TRANSPORT: &str = "cursor-acp-stdio";

/// Cursor adapter over one finite C1 launch capability.
///
/// Holds the same [`CursorLaunch`] the existing `execute_cursor_turn`
/// executor receives through `InternalLaunch::Cursor`; the launch is neither
/// cloned nor re-resolved here. Construct one per admitted Cursor run once
/// the wiring packet lands.
pub(crate) struct CursorSocketAdapter<'a> {
    launch: &'a CursorLaunch,
}

impl<'a> CursorSocketAdapter<'a> {
    /// Wraps one finite C1 cursor launch capability for the socket seam.
    #[must_use]
    pub(crate) fn new(launch: &'a CursorLaunch) -> Self {
        Self { launch }
    }
}

impl EngineSocket for CursorSocketAdapter<'_> {
    /// Returns the exact Cursor descriptor, including every capability state
    /// declared by the TypeScript adapter
    /// (`CursorEngineDescriptor.capabilities`,
    /// `modules/engines/src/cursor/engine.ts:15-56`).
    fn descriptor(&self) -> EngineDescriptor {
        EngineDescriptor {
            id: CURSOR_ENGINE_ID.to_owned(),
            display_name: CURSOR_DISPLAY_NAME.to_owned(),
            transport: CURSOR_TRANSPORT.to_owned(),
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
                // `global_guidance` — unsupported: Cursor discovers
                // AGENTS.md, CLAUDE.md, and .cursor/rules natively.
                (
                    EngineCapabilityName::GlobalGuidance,
                    EngineCapabilityState::Unsupported,
                ),
                // `model_catalog` — unsupported: Cursor models are supplied by
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

    /// Reports readiness from the launch version.
    ///
    /// Placeholder until the probe wiring packet lands: no live probe runs
    /// here, so `ready` stays `false` and no terminal state is claimed. C1
    /// has no probe-certified version yet, so the reported version is the
    /// unprobed sentinel carried by the launch.
    fn probe(&self) -> EngineProbe {
        EngineProbe {
            ready: false,
            version: self.launch.version().to_owned(),
            terminal_state: None,
        }
    }

    /// Fails closed until the open wiring packet forwards to
    /// `execute_cursor_turn`.
    fn open(&self, _input: EngineOpenInput) -> EngineOpenFuture<'_> {
        Box::pin(std::future::ready(EngineOpenOutcome::failed(
            EngineOpenError::Unimplemented,
        )))
    }
}
