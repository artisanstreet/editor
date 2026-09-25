//! Sending a thread's Forge-owned composer draft as a message.
//!
//! The Editor never sends a message body: it names the draft revision it
//! last saved, and the Forge queues exactly that draft, records the
//! submission under the thread and revision, and clears the draft in one
//! transaction. Submitting the same revision again answers the message the
//! first submission queued (a duplicate) and changes nothing, so a send whose
//! answer was lost can be repeated safely under any request id.

use crate::{
    ComposerDraftRevision, MessageId, ReceiptDisposition, RequestId, SteerTarget, ThreadId,
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
    /// Observed live run the message must steer into, if named.
    pub steer_target: Option<SteerTarget>,
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
    },
    /// The stored draft is at another revision (`None`: the thread has no
    /// draft); nothing was queued. The sender saves its draft again and
    /// submits the revision that save is given.
    Stale {
        /// The thread's current draft revision.
        current_revision: Option<ComposerDraftRevision>,
    },
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
