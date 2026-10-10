//! Codex context-compaction pairing.
//!
//! Codex reports one compaction twice: as a `contextCompaction` item
//! (`item/started` then `item/completed`, each stamped with
//! `startedAtMs` / `completedAtMs`) and through the deprecated
//! `thread/compacted` notification (`{threadId, turnId}` only). The item pair
//! is authoritative; `thread/compacted` closes a compaction only when no item
//! completion did, so one compaction never settles twice.

use std::collections::{HashMap, HashSet};

/// One compaction signal decoded from the wire.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CodexCompactionSignal {
    /// `item/started` on a `contextCompaction` item.
    ItemStarted {
        item_id: String,
        started_at_ms: Option<u64>,
    },
    /// `item/completed` on a `contextCompaction` item.
    ItemCompleted {
        item_id: String,
        completed_at_ms: Option<u64>,
    },
    /// The deprecated `thread/compacted` notification.
    ThreadCompacted,
}

/// What one compaction signal should emit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum CodexCompactionStep {
    /// A compaction began on this item.
    Started { item_id: String },
    /// A compaction ended. `item_id` is the item it pairs with, or [`None`]
    /// when only `thread/compacted` reported it.
    Completed {
        item_id: Option<String>,
        duration_ms: Option<u64>,
    },
    /// Already reported: emit nothing.
    Duplicate,
}

#[derive(Debug)]
struct OpenCompaction {
    turn_id: String,
    item_id: String,
    started_at_ms: Option<u64>,
}

/// Pairs compaction signals for one live turn.
#[derive(Debug, Default)]
pub(crate) struct CodexCompactionTracker {
    /// Compactions started and not yet completed, oldest first.
    open: Vec<OpenCompaction>,
    /// Items whose completion was already emitted.
    settled: HashSet<String>,
    /// Item completions per turn that no `thread/compacted` has echoed yet.
    unechoed: HashMap<String, u32>,
}

impl CodexCompactionTracker {
    /// Records one authorized compaction signal and returns what to emit.
    pub(crate) fn observe(
        &mut self,
        turn_id: &str,
        signal: CodexCompactionSignal,
    ) -> CodexCompactionStep {
        match signal {
            CodexCompactionSignal::ItemStarted {
                item_id,
                started_at_ms,
            } => {
                if self.settled.contains(&item_id)
                    || self.open.iter().any(|open| open.item_id == item_id)
                {
                    return CodexCompactionStep::Duplicate;
                }
                self.open.push(OpenCompaction {
                    turn_id: turn_id.to_owned(),
                    item_id: item_id.clone(),
                    started_at_ms,
                });
                CodexCompactionStep::Started { item_id }
            }
            CodexCompactionSignal::ItemCompleted {
                item_id,
                completed_at_ms,
            } => {
                // `thread/compacted` may already have closed this item.
                if !self.settled.insert(item_id.clone()) {
                    return CodexCompactionStep::Duplicate;
                }
                let started_at_ms = self
                    .open
                    .iter()
                    .position(|open| open.item_id == item_id)
                    .and_then(|index| self.open.remove(index).started_at_ms);
                *self.unechoed.entry(turn_id.to_owned()).or_default() += 1;
                CodexCompactionStep::Completed {
                    item_id: Some(item_id),
                    duration_ms: started_at_ms
                        .zip(completed_at_ms)
                        .and_then(|(started, completed)| completed.checked_sub(started)),
                }
            }
            CodexCompactionSignal::ThreadCompacted => {
                if let Some(count) = self.unechoed.get_mut(turn_id)
                    && *count > 0
                {
                    *count -= 1;
                    return CodexCompactionStep::Duplicate;
                }
                if let Some(index) = self.open.iter().position(|open| open.turn_id == turn_id) {
                    let open = self.open.remove(index);
                    self.settled.insert(open.item_id.clone());
                    return CodexCompactionStep::Completed {
                        item_id: Some(open.item_id),
                        duration_ms: None,
                    };
                }
                CodexCompactionStep::Completed {
                    item_id: None,
                    duration_ms: None,
                }
            }
        }
    }
}
