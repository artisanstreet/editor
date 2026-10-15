//! What decoders keep of settled items.
//!
//! A decoder keeps its own copy of each item it may still change: a tool's
//! late status, an agent's next report, an answer to a question. Once an
//! item has settled, Forge has stored its complete event, so the copy keeps
//! its large texts and collections only as the descriptors the store gave
//! them (`forge_content::compact_settled`). Settled work therefore costs a
//! decoder a few hundred bytes per item however long the run, and what is
//! still open is the engine's own working set, each frame of which
//! `process::MAX_LINE_BYTES` caps. That is the whole memory bound: engines
//! are local processes Forge launched, so there is no separate budget.
//!
//! [`settle`] wraps every decoder's output, so no decoder compacts by hand.

use crate::EngineEvent;
use artisan_api::ItemKind;

/// A decoder that keeps copies of the items it emits.
pub(crate) trait Retains {
    /// The decoder's copy of the item it emits as `key`, if it keeps one.
    fn retained(&mut self, key: &str) -> Option<&mut ItemKind>;
}

/// Passes a decoder's events on, compacting its copy of every item they
/// settle. The events still carry the complete item: Forge stores it before
/// any later event names the descriptors.
pub(crate) fn settle(decoder: &mut impl Retains, events: Vec<EngineEvent>) -> Vec<EngineEvent> {
    for event in &events {
        let (EngineEvent::Item { key, status, .. } | EngineEvent::Status { key, status }) = event
        else {
            continue;
        };
        if !status.is_terminal() {
            continue;
        }
        if let Some(kind) = decoder.retained(key)
            && let Err(error) = forge_content::compact_settled(kind)
        {
            // Only hashing a collection's records can fail, which plain data
            // never does; the copy then stays as it was.
            tracing::warn!(key, %error, "keeping a settled item uncompacted");
        }
    }
    events
}
