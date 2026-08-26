//! Durable thread unread and attention presentation state.
//!
//! The native slice of the legacy root thread-list model: from the
//! Forge-owned run authority and the durable reader cursors of one thread it
//! derives the two observations a thread row needs — whether the thread is
//! unread, and whether it needs reader attention. Both observations are
//! computed here from typed inputs and exposed read-only, so a caller can
//! never supply contradictory booleans.
//!
//! Semantics carried over from the legacy presentation (`ThreadSettled`,
//! `ThreadUnread`, `ThreadNeedsAttention`, `ThreadHasActiveWork`, and
//! `ThreadReaderActivityAt` in
//! `modules/frontend/src/lib/root/thread-navigation.ts`, fed by the root
//! tracking in `thread-read-tracker.ts`):
//!
//! - Read/unread is durable cursor equality against the root-visible reader
//!   activity cursor. An absent or stale acknowledgement means unread; exact
//!   equality with the current cursor means read. Ordering and wall-clock
//!   time are never compared, so drift in either direction is equally
//!   unread.
//! - Hidden worker activity is not an input. Callers pass the already
//!   resolved root-visible cursor; a read survives background worker churn
//!   because only reader-visible activity can outrun its acknowledgement.
//! - Active work stays authoritative. A thread in [`RunState::Active`] never
//!   becomes a needs-attention outcome merely because it is also unread, and
//!   a durable acknowledgement never retires a live run.
//! - Only an inactive, unread, terminal outcome ([`RunState::Completed`] or
//!   [`RunState::Failed`]) needs attention. An inactive [`RunState::Idle`]
//!   thread may be unread without ever needing attention.
//!
//! Deliberately out of boundary: acknowledgement mutation, persistence,
//! transport, ordering, routing, title markers, question and approval
//! semantics, colors, and GPUI rendering. The legacy `ThreadSummary` cuts
//! exactly these cursors and statuses out of the shared domain model
//! (`modules/domain/src/model.rs`); this module is where they land for the
//! native frontend.

use artisan_domain::UnixMillis;

/// The Forge-owned run authority behind one thread, collapsed to the cases
/// the presentation model distinguishes.
///
/// Legacy live-status strings collapse here: running and other
/// still-run-owned statuses map to [`RunState::Active`], settled threads
/// with no pending outcome map to [`RunState::Idle`], and the sticky
/// terminal outcomes map to [`RunState::Completed`] and [`RunState::Failed`].
/// Question and approval statuses are deliberate exclusions of this model.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RunState {
    /// A run currently owns the thread; the run, not the reader, decides
    /// what happens next.
    Active,
    /// No run owns the thread and no terminal outcome is pending on it.
    Idle,
    /// The latest run finished cleanly.
    Completed,
    /// The latest run errored.
    Failed,
}

/// The derived unread and attention observations for one thread.
///
/// Constructed only through [`ThreadAttention::derive`]; the fields are
/// private so callers cannot assemble contradictory states. The two
/// observations stay independently accessible and relate one way only:
/// needs-attention implies unread, but unread alone does not imply
/// needs-attention — unread spans every run state, while needs-attention
/// covers only the inactive unread terminal outcomes.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ThreadAttention {
    unread: bool,
    needs_attention: bool,
}

impl ThreadAttention {
    /// Derives the presentation state from the run authority and the
    /// durable reader cursors.
    ///
    /// `reader_cursor` is the thread's current root-visible reader activity
    /// instant (the legacy `reader_activity_at ?? last_activity_at`
    /// projection, resolved by the caller). `acknowledged` is Forge's
    /// durable reader acknowledgement, or [`None`] when none was recorded.
    /// The thread presents as read exactly when the acknowledgement exists
    /// and equals the current cursor; every other combination is unread.
    ///
    /// Attention additionally requires an inactive terminal outcome: active
    /// work keeps authority even while unread, and an idle thread carries no
    /// outcome at all.
    #[must_use]
    pub fn derive(
        run_state: RunState,
        reader_cursor: UnixMillis,
        acknowledged: Option<UnixMillis>,
    ) -> Self {
        // Durable read means exact cursor equality — never ordering or
        // elapsed time. Either direction of drift reopens the thread.
        let unread = acknowledged != Some(reader_cursor);

        Self {
            unread,
            needs_attention: unread && matches!(run_state, RunState::Completed | RunState::Failed),
        }
    }

    /// Whether the durable acknowledgement is absent from, or unequal to,
    /// the current root-visible activity cursor.
    #[must_use]
    pub fn is_unread(self) -> bool {
        self.unread
    }

    /// Whether an inactive, unread, completed or failed outcome is waiting
    /// for the reader.
    #[must_use]
    pub fn needs_attention(self) -> bool {
        self.needs_attention
    }
}
