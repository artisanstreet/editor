//! Sending a thread's Forge-owned composer draft as a message.
//!
//! The Editor never sends a message body: it names the draft revision it
//! last saved, and the Forge queues exactly that draft, records the
//! submission under the thread and revision, and clears the draft in one
//! transaction. Submitting the same revision again answers the message the
//! first submission queued (a duplicate) and changes nothing, so a send whose
//! answer was lost can be repeated safely under any request id.
//!
//! The send carries the user's model selection, not a configuration: the
//! Forge resolves it, saves it when it changes the thread's configuration,
//! decides whether the message steers the thread's live run, and refuses a
//! send it cannot admit with a typed, presentation-ready reason.

use crate::{
    CatalogSelection, ComposerDraftRevision, EngineConfigRevision, MessageId, ReceiptDisposition,
    RequestId, SubmissionRefusal, ThreadId,
};

/// Queues a thread's composer draft at exactly `draft_revision`.
///
/// `request_id` only correlates this request with its answer; the
/// submission's identity is the thread and draft revision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubmitComposerDraft {
    /// Transport correlation identity.
    pub request_id: RequestId,
    /// Thread whose draft is sent.
    pub thread_id: ThreadId,
    /// The draft revision the sender last saved.
    pub draft_revision: ComposerDraftRevision,
    /// The model the user selected for this send; `None` sends with the
    /// thread's saved configuration.
    pub selection: Option<CatalogSelection>,
}

/// What the Forge did with a submitted draft.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DraftSubmissionOutcome {
    /// The draft is the queued message `message_id`: `Duplicate` when this
    /// revision had already been submitted. The draft is empty at
    /// `cleared_revision` either way.
    Queued {
        /// Forge-minted message identity.
        message_id: MessageId,
        /// First submission or replay of an earlier one.
        disposition: ReceiptDisposition,
        /// Revision of the emptied draft.
        cleared_revision: ComposerDraftRevision,
        /// Revision of the thread's engine configuration after admission,
        /// so a sender can tell whether the send saved a new one.
        engine_config_revision: EngineConfigRevision,
    },
    /// The stored draft is at another revision (`None`: the thread has no
    /// draft); nothing was queued. The sender saves its draft again and
    /// submits the revision that save is given.
    Stale {
        /// The thread's current draft revision.
        current_revision: Option<ComposerDraftRevision>,
    },
    /// The Forge refused the send; nothing was queued and the draft is
    /// unchanged.
    Refused(SubmissionRefusal),
}

/// Answer to [`SubmitComposerDraft`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ComposerDraftSubmitted {
    /// The submit command's request identity.
    pub request_id: RequestId,
    /// Thread whose draft was submitted.
    pub thread_id: ThreadId,
    /// The revision the command named.
    pub draft_revision: ComposerDraftRevision,
    /// What the Forge did.
    pub outcome: DraftSubmissionOutcome,
}
