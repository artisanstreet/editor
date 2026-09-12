//! Provider-neutral engine-socket adapters.
//!
//! This module is the backend half of the `EngineSocket` migration: the
//! domain [`EngineSocket`](artisan_domain::EngineSocket) seam, implemented per
//! provider over the same verified launch capability the owner executors
//! already receive. The live configured-turn dispatch asks [`adapter_for`]
//! for the engine identity, so admitted launches route through this seam
//! instead of a direct `InternalLaunch` match.
//!
//! Adapters borrow the admitted launch: nothing is cloned or re-resolved, and
//! the executor keeps custody. `open` still fails closed with
//! [`EngineOpenError::Unimplemented`](artisan_domain::EngineOpenError::Unimplemented)
//! until the per-engine open/drive split packet lands.

#![forbid(unsafe_code)]

pub(crate) mod claude;
pub(crate) mod codex;
pub(crate) mod cursor;
pub(crate) mod grok;
pub(crate) mod opencode;

use artisan_domain::EngineSocket;

use super::InternalLaunch;

/// Borrows one admitted launch as its provider-neutral socket adapter.
///
/// The mapping is total: the test-only fixture lane gets an adapter that
/// reports the fixture identity and keeps the configured executor fallback
/// in the caller.
#[must_use]
pub(crate) fn adapter_for(launch: &InternalLaunch) -> Box<dyn EngineSocket + '_> {
    match launch {
        InternalLaunch::Verified(verified) => {
            Box::new(opencode::OpenCode2SocketAdapter::new(verified))
        }
        InternalLaunch::Codex(verified) => Box::new(codex::CodexSocketAdapter::new(verified)),
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
    ) -> artisan_domain::EngineOpenResult {
        Err(artisan_domain::EngineOpenError::Unimplemented)
    }
}
