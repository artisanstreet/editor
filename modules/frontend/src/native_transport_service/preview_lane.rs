//! The link-preview lane: rich-link resolves wait here, apart from the
//! command queue, and run only while that queue is idle.
//!
//! A thread full of links asks for one page title per link, and the Forge
//! fetches each page from the web (hundreds of milliseconds, seconds at
//! worst). Queued with ordinary commands, a burst of them filled the command
//! queue and kept navigation (`Unsubscribe`, `ReadActiveRun`, the next
//! thread's snapshot) waiting behind every fetch until the thread switch
//! timed out.
//!
//! The lane cannot fetch in parallel: the Forge admits one authenticated
//! connection at a time (a second one waits until the first ends) and serves
//! that connection's requests strictly in order, so every preview has to go
//! over the service's one session. What the lane guarantees instead is
//! priority. The command loop takes a preview only when no command and no
//! delivery is ready, one at a time, so a command never waits for more than
//! the one preview already on the wire, and never for the queued ones. Each
//! result is published the moment it arrives.
//!
//! Previews belong to the surface that asked for them. When the application
//! mounts another conversation it discards the queued ones
//! ([`PreviewLane::discard_queued`]): the new surface has a fresh title table
//! and asks for its own links.

#![forbid(unsafe_code)]

use std::collections::VecDeque;

use super::{CommandSendError, Mutex, PoisonError};

/// Maximum number of link previews waiting for the service thread. Twice a
/// surface's title-table bound, so one surface's whole burst plus a late
/// tail from the previous one fit; a full lane answers `Busy` and the
/// application retries its unsubmitted tail, exactly like the command queue.
pub const PREVIEW_CAPACITY: usize = 128;

/// One link preview waiting in the lane.
pub(super) struct QueuedPreview {
    #[cfg(feature = "flight-recorder")]
    pub(super) trace: artisan_tracing::Span,
    /// Canonical absolute URL selected by the rich-link URL policy.
    pub(super) url: String,
}

/// The bounded queue of link previews shared by the application handle and
/// the service loop.
#[derive(Default)]
pub struct PreviewLane {
    queued: Mutex<VecDeque<QueuedPreview>>,
    /// Wakes the service loop when a preview arrives. `notify_one` keeps a
    /// permit while nobody waits, so a push between the loop's empty check
    /// and its wait is never lost.
    arrived: tokio::sync::Notify,
}

impl PreviewLane {
    /// Admits one preview without waiting.
    ///
    /// # Errors
    ///
    /// Returns [`CommandSendError::Busy`] when the lane is full.
    pub(super) fn try_push(&self, url: String) -> Result<(), CommandSendError> {
        let mut queued = self.queued.lock().unwrap_or_else(PoisonError::into_inner);
        if queued.len() >= PREVIEW_CAPACITY {
            #[cfg(feature = "flight-recorder")]
            artisan_tracing::instant!("transport.preview", "preview.rejected", "reason" => "Busy");
            return Err(CommandSendError::Busy);
        }
        queued.push_back(QueuedPreview {
            #[cfg(feature = "flight-recorder")]
            trace: artisan_tracing::span!("transport.preview", "preview.wait"),
            url,
        });
        #[cfg(feature = "flight-recorder")]
        artisan_tracing::counter!("transport.preview", "preview.depth", "queued" => queued.len());
        drop(queued);
        self.arrived.notify_one();
        Ok(())
    }

    /// Drops every preview that has not started; the one on the wire, if
    /// any, still publishes its result. Returns how many were dropped.
    pub(super) fn discard_queued(&self) -> usize {
        let discarded =
            std::mem::take(&mut *self.queued.lock().unwrap_or_else(PoisonError::into_inner));
        #[cfg(feature = "flight-recorder")]
        if !discarded.is_empty() {
            artisan_tracing::instant!("transport.preview", "preview.discarded", "count" => discarded.len());
        }
        discarded.len()
    }

    /// The oldest queued preview, waiting for one when the lane is empty.
    ///
    /// Cancel-safe: a preview leaves the queue only in the poll that returns
    /// it, so the command loop may drop this future whenever a command or a
    /// delivery wins its select.
    pub(super) async fn next(&self) -> QueuedPreview {
        loop {
            let popped = self
                .queued
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .pop_front();
            if let Some(preview) = popped {
                return preview;
            }
            self.arrived.notified().await;
        }
    }

    /// Previews waiting to start, for tests.
    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.queued
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }
}
