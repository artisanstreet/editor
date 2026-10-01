//! What each live run is thinking right now, held in memory only.
//!
//! A thinking summary is shown only while it is the newest thing its run
//! produced. The run dispatcher therefore keeps the current block here
//! instead of writing it to the observation ledger, and every delivery driver
//! pushes it to the thread's subscribers as it changes. Nothing here survives
//! the run moving on, the run ending, or the Forge stopping.

#![forbid(unsafe_code)]

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard},
};

use artisan_domain::{LiveThinkingBlock, RunId, ThreadId, TurnId, UnixMillis};

/// Most UTF-8 bytes of one block kept and pushed. A longer summary keeps its
/// newest part, which is the part a reader is following.
pub const LIVE_THINKING_MAX_BYTES: usize = 8 * 1024;

/// The current thinking block of every thread with a thinking live run.
#[derive(Clone, Debug, Default)]
pub struct LiveThinkingBoard {
    blocks: Arc<Mutex<HashMap<ThreadId, LiveThinkingBlock>>>,
}

/// Which block of which run a fragment belongs to.
#[derive(Clone, Copy, Debug)]
pub struct ThinkingSource<'a> {
    /// Thread the run belongs to.
    pub thread_id: &'a ThreadId,
    /// Run that is thinking.
    pub run_id: &'a RunId,
    /// Forge turn the run launched from.
    pub turn_id: &'a TurnId,
    /// Provider identity of the thinking block.
    pub item_id: &'a str,
    /// When the fragment arrived.
    pub at: UnixMillis,
}

impl LiveThinkingBoard {
    /// Creates an empty board.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    // A panic while holding the lock cannot leave a block half written in a
    // way that matters: the worst case is a stale summary the next fragment
    // or the run's end replaces.
    fn lock(&self) -> MutexGuard<'_, HashMap<ThreadId, LiveThinkingBlock>> {
        self.blocks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Extends the source's block with one streamed fragment; a fragment of
    /// another block or run starts a new block.
    pub fn append(&self, source: &ThinkingSource<'_>, delta: &str) {
        let mut blocks = self.lock();
        match blocks.get_mut(source.thread_id) {
            Some(block) if block.run_id == *source.run_id && block.item_id == source.item_id => {
                block.text.push_str(delta);
                keep_newest(&mut block.text);
                block.updated_at = source.at;
            }
            _ => {
                blocks.insert(source.thread_id.clone(), new_block(source, delta));
            }
        }
    }

    /// Settles the source's block. `text` is the provider's authoritative
    /// summary when it sent one; otherwise the streamed text stands.
    pub fn complete(&self, source: &ThinkingSource<'_>, text: Option<&str>) {
        let mut blocks = self.lock();
        match blocks.get_mut(source.thread_id) {
            Some(block) if block.run_id == *source.run_id && block.item_id == source.item_id => {
                if let Some(text) = text {
                    text.clone_into(&mut block.text);
                    keep_newest(&mut block.text);
                }
                block.updated_at = source.at;
            }
            _ => {
                if let Some(text) = text.filter(|text| !text.is_empty()) {
                    blocks.insert(source.thread_id.clone(), new_block(source, text));
                }
            }
        }
    }

    /// Drops the thread's block when `run_id` owns it. Returns whether a
    /// block was dropped, so the caller wakes subscribers only then.
    #[must_use]
    pub fn clear(&self, thread_id: &ThreadId, run_id: &RunId) -> bool {
        let mut blocks = self.lock();
        if blocks
            .get(thread_id)
            .is_some_and(|block| block.run_id == *run_id)
        {
            blocks.remove(thread_id);
            return true;
        }
        false
    }

    /// The thread's current block, if its live run is thinking.
    #[must_use]
    pub fn current(&self, thread_id: &ThreadId) -> Option<LiveThinkingBlock> {
        self.lock().get(thread_id).cloned()
    }
}

fn new_block(source: &ThinkingSource<'_>, text: &str) -> LiveThinkingBlock {
    let mut text = text.to_owned();
    keep_newest(&mut text);
    LiveThinkingBlock {
        run_id: source.run_id.clone(),
        turn_id: source.turn_id.clone(),
        item_id: source.item_id.to_owned(),
        text,
        started_at: source.at,
        updated_at: source.at,
    }
}

/// Trims `text` to its newest [`LIVE_THINKING_MAX_BYTES`] at a character
/// boundary.
fn keep_newest(text: &mut String) {
    if text.len() <= LIVE_THINKING_MAX_BYTES {
        return;
    }
    let mut start = text.len() - LIVE_THINKING_MAX_BYTES;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text.drain(..start);
}
