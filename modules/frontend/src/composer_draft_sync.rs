//! Latest-wins save chain for Forge-owned composer drafts.
//!
//! The Forge owns every draft and assigns its revisions; every save applies
//! and the last to arrive wins. Per scope, at most one save is in flight: a
//! newer body typed meanwhile replaces the unsent one and is sent as soon as
//! the in-flight save is acknowledged, so this Editor's saves arrive in the
//! order they were typed. Saves are matched to their acknowledgements by a
//! local per-scope send sequence. A scope keeps one connection hold from its
//! first unsaved change (or attachment upload) until the acknowledgement of
//! its latest sent save arrives and nothing else is pending, so a host
//! switch or quit drains every draft before the connection closes.
//!
//! A message is sent by naming the draft revision the Forge gave its body,
//! so a send waits until the save carrying that body is acknowledged. While
//! it waits, the body being sent is saved ahead of anything typed after
//! Send, and later bodies are held until the send is on the wire; the
//! revision is therefore exactly the sent body's. The latest revision the
//! Forge reported per scope outlives the connection's save chains, so a send
//! repeated after a lost answer names the same revision.
//!
//! The chain is generic over the hold so its bookkeeping is testable without
//! a connection.

#![forbid(unsafe_code)]

use std::collections::HashMap;

use artisan_domain::{ComposerAttachmentRef, ComposerDraftRevision, ComposerDraftScope};

/// The saved content of one draft: text exactly as typed and the stored
/// attachments in tray order.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct DraftBody {
    /// Authored text.
    pub(crate) text: String,
    /// Stored attachments in tray order.
    pub(crate) attachments: Vec<ComposerAttachmentRef>,
}

/// One save the caller must send now.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DraftSave {
    /// Draft scope.
    pub(crate) scope: ComposerDraftScope,
    /// Local send sequence that pairs the save with its acknowledgement.
    pub(crate) sequence: u64,
    /// Content to store.
    pub(crate) body: DraftBody,
}

/// Whether a waiting send may name its draft revision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SubmitReadiness {
    /// The body being sent is not acknowledged yet.
    Waiting,
    /// The Forge stored the body being sent at this revision.
    Ready(ComposerDraftRevision),
    /// The body being sent could not be stored.
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SubmitWait {
    /// The body being sent is waiting behind an in-flight save.
    Pinned,
    /// The body being sent is the save with this sequence.
    Sequence(u64),
    /// The body being sent is stored.
    Ready(ComposerDraftRevision),
    Failed,
}

struct Slot<H> {
    /// Sequence of the latest save sent for this scope.
    sent: u64,
    in_flight: Option<u64>,
    pending: Option<DraftBody>,
    /// The body a waiting send carries, saved ahead of `pending`.
    pinned: Option<DraftBody>,
    submit: Option<SubmitWait>,
    uploads: usize,
    hold: Option<H>,
}

impl<H> Slot<H> {
    const fn new() -> Self {
        Self {
            sent: 0,
            in_flight: None,
            pending: None,
            pinned: None,
            submit: None,
            uploads: 0,
            hold: None,
        }
    }

    const fn is_settled(&self) -> bool {
        self.in_flight.is_none()
            && self.pending.is_none()
            && self.pinned.is_none()
            && self.uploads == 0
    }

    fn send(&mut self, scope: &ComposerDraftScope, body: DraftBody) -> DraftSave {
        self.sent = self.sent.wrapping_add(1);
        self.in_flight = Some(self.sent);
        DraftSave {
            scope: scope.clone(),
            sequence: self.sent,
            body,
        }
    }
}

/// Per-scope save chains.
pub(crate) struct DraftSync<H> {
    slots: HashMap<ComposerDraftScope, Slot<H>>,
    /// Latest revision the Forge reported per scope.
    revisions: HashMap<ComposerDraftScope, ComposerDraftRevision>,
}

impl<H> Default for DraftSync<H> {
    fn default() -> Self {
        Self {
            slots: HashMap::new(),
            revisions: HashMap::new(),
        }
    }
}

impl<H> DraftSync<H> {
    fn slot(
        &mut self,
        scope: &ComposerDraftScope,
        hold: impl FnOnce() -> Option<H>,
    ) -> &mut Slot<H> {
        let slot = self.slots.entry(scope.clone()).or_insert_with(Slot::new);
        if slot.hold.is_none() {
            slot.hold = hold();
        }
        slot
    }

    /// Records a new body. Returns the save to send now, or `None` when a
    /// save is in flight (the body replaces any unsent one). `hold` is asked
    /// for a connection hold only when the scope has none.
    pub(crate) fn edit(
        &mut self,
        scope: &ComposerDraftScope,
        body: DraftBody,
        hold: impl FnOnce() -> Option<H>,
    ) -> Option<DraftSave> {
        self.slot(scope, hold).pending = Some(body);
        self.advance(scope)
    }

    /// Settles the save with `sequence` as definitively failed and returns
    /// the next pending save, if any. A failed body is not retried; the next
    /// change sends the complete current body.
    pub(crate) fn settled(
        &mut self,
        scope: &ComposerDraftScope,
        sequence: u64,
    ) -> Option<DraftSave> {
        self.settle(scope, sequence, None)
    }

    /// Settles the save with `sequence` as stored at `revision`.
    pub(crate) fn acknowledged(
        &mut self,
        scope: &ComposerDraftScope,
        sequence: u64,
        revision: ComposerDraftRevision,
    ) -> Option<DraftSave> {
        self.settle(scope, sequence, Some(revision))
    }

    /// Records a revision the Forge reported for a scope (a read, or the
    /// emptied draft after a send). Revisions only grow.
    pub(crate) fn observe_revision(
        &mut self,
        scope: &ComposerDraftScope,
        revision: ComposerDraftRevision,
    ) {
        let known = self.revisions.entry(scope.clone()).or_insert(revision);
        *known = (*known).max(revision);
    }

    /// Starts a send of `body`, the scope's current content. The send
    /// names the revision of the save that carries this body: the unsent
    /// body, the in-flight save, or, when everything is stored, the latest
    /// known revision. Returns a save the caller must send now.
    pub(crate) fn begin_submit(
        &mut self,
        scope: &ComposerDraftScope,
        body: DraftBody,
    ) -> Option<DraftSave> {
        let known = self.revisions.get(scope).copied();
        let slot = self.slots.entry(scope.clone()).or_insert_with(Slot::new);
        slot.pinned = slot.pending.take();
        if slot.pinned.is_none() && slot.in_flight.is_none() && known.is_none() {
            // Nothing the Forge acknowledged names this body yet.
            slot.pinned = Some(body);
        }
        slot.submit = Some(match (&slot.pinned, slot.in_flight, known) {
            (Some(_), _, _) => SubmitWait::Pinned,
            (None, Some(sequence), _) => SubmitWait::Sequence(sequence),
            (None, None, Some(revision)) => SubmitWait::Ready(revision),
            (None, None, None) => SubmitWait::Failed,
        });
        self.advance(scope)
    }

    /// Whether a send of this scope is waiting for its draft.
    pub(crate) fn is_submitting(&self, scope: &ComposerDraftScope) -> bool {
        self.slots
            .get(scope)
            .is_some_and(|slot| slot.submit.is_some())
    }

    /// Whether the send started with [`Self::begin_submit`] may name its
    /// revision.
    pub(crate) fn submit_readiness(&self, scope: &ComposerDraftScope) -> SubmitReadiness {
        match self.slots.get(scope).and_then(|slot| slot.submit) {
            Some(SubmitWait::Ready(revision)) => SubmitReadiness::Ready(revision),
            Some(SubmitWait::Pinned | SubmitWait::Sequence(_)) => SubmitReadiness::Waiting,
            Some(SubmitWait::Failed) | None => SubmitReadiness::Failed,
        }
    }

    /// Ends a send (it is on the wire, or it was abandoned) and returns the
    /// next save held behind it. An abandoned send's body is still saved
    /// unless something newer was typed.
    pub(crate) fn end_submit(&mut self, scope: &ComposerDraftScope) -> Option<DraftSave> {
        let slot = self.slots.get_mut(scope)?;
        slot.submit = None;
        if let Some(pinned) = slot.pinned.take()
            && slot.pending.is_none()
        {
            slot.pending = Some(pinned);
        }
        self.advance(scope)
    }

    fn settle(
        &mut self,
        scope: &ComposerDraftScope,
        sequence: u64,
        revision: Option<ComposerDraftRevision>,
    ) -> Option<DraftSave> {
        if let Some(revision) = revision {
            self.observe_revision(scope, revision);
        }
        let slot = self.slots.get_mut(scope)?;
        if slot.in_flight != Some(sequence) {
            return None;
        }
        slot.in_flight = None;
        if slot.submit == Some(SubmitWait::Sequence(sequence)) {
            slot.submit = Some(revision.map_or(SubmitWait::Failed, SubmitWait::Ready));
        }
        self.advance(scope)
    }

    /// Keeps the scope busy while one attachment uploads.
    pub(crate) fn begin_upload(
        &mut self,
        scope: &ComposerDraftScope,
        hold: impl FnOnce() -> Option<H>,
    ) {
        self.slot(scope, hold).uploads += 1;
    }

    /// Ends one upload started with [`Self::begin_upload`].
    pub(crate) fn finish_upload(&mut self, scope: &ComposerDraftScope) {
        if let Some(slot) = self.slots.get_mut(scope) {
            slot.uploads = slot.uploads.saturating_sub(1);
            if slot.is_settled() {
                slot.hold = None;
            }
        }
    }

    /// Sends every unsent body now, without waiting for in-flight saves, for
    /// a connection that is about to close. The service loop is serial, so
    /// each flushed save still arrives after the saves sent before it.
    pub(crate) fn flush(&mut self) -> Vec<(DraftSave, Option<&H>)> {
        self.slots
            .iter_mut()
            .filter_map(|(scope, slot)| {
                slot.submit = None;
                let pinned = slot.pinned.take();
                let body = slot.pending.take().or(pinned)?;
                Some((slot.send(scope, body), slot.hold.as_ref()))
            })
            .collect()
    }

    /// Releases every hold, for a connection that has stopped or closed.
    pub(crate) fn release_all(&mut self) {
        self.slots.clear();
    }

    /// The hold that admits a scope's next save, if the scope holds one.
    pub(crate) fn hold(&self, scope: &ComposerDraftScope) -> Option<&H> {
        self.slots.get(scope)?.hold.as_ref()
    }

    /// Whether a scope has no save in flight, no unsent body, and no upload.
    #[cfg(test)]
    pub(crate) fn is_settled(&self, scope: &ComposerDraftScope) -> bool {
        self.slots.get(scope).is_none_or(Slot::is_settled)
    }

    fn advance(&mut self, scope: &ComposerDraftScope) -> Option<DraftSave> {
        let slot = self.slots.get_mut(scope)?;
        let save = if slot.in_flight.is_some() {
            None
        } else if let Some(body) = slot.pinned.take() {
            let save = slot.send(scope, body);
            slot.submit = Some(SubmitWait::Sequence(save.sequence));
            Some(save)
        } else if slot.submit.is_none() {
            // Bodies typed after Send wait until the send is on the wire.
            slot.pending.take().map(|body| slot.send(scope, body))
        } else {
            None
        };
        if slot.is_settled() {
            slot.hold = None;
        }
        save
    }
}

#[cfg(test)]
#[path = "composer_draft_sync/tests.rs"]
mod tests;
