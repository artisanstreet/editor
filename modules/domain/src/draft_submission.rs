//! Sending a Forge-owned composer draft as a message.
//!
//! The Editor never sends a message body: it names the draft revision it
//! last saved, and the Forge queues exactly that draft, records the
//! submission under the draft's scope and revision, and clears the draft in
//! one transaction. A thread's draft is queued in that thread; a project's
//! new-task draft creates the thread it is queued in, in the same
//! transaction. Submitting the same revision again answers the message (and
//! thread) the first submission queued as a duplicate and changes nothing,
//! so a send whose answer was lost can be repeated safely under any request
//! id.
//!
//! The send carries the user's model selection, not a configuration: the
//! Forge resolves it, saves it when it changes the thread's configuration,
//! decides whether the message steers the thread's live run, and refuses a
//! send it cannot admit with a typed, presentation-ready reason.

use crate::{
    CatalogSelection, ComposerDraftRevision, ComposerDraftScope, EngineConfigRevision, MessageId,
    ReceiptDisposition, RequestId, SubmissionRefusal, ThreadId,
};

/// Queues a composer draft at exactly `draft_revision`.
///
/// `request_id` only correlates this request with its answer; the
/// submission's identity is the draft scope and revision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SubmitComposerDraft {
    /// Transport correlation identity.
    pub request_id: RequestId,
    /// The draft that is sent: a thread's, or a project's new-task draft,
    /// whose submission creates the thread it is queued in.
    pub scope: ComposerDraftScope,
    /// The draft revision the sender last saved.
    pub draft_revision: ComposerDraftRevision,
    /// The model the user selected for this send; `None` sends with the
    /// thread's saved configuration (for a new thread, the default one).
    pub selection: Option<CatalogSelection>,
}

/// What the Forge did with a submitted draft.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DraftSubmissionOutcome {
    /// The draft is the queued message `message_id` of `thread_id`:
    /// `Duplicate` when this revision had already been submitted. The draft
    /// is empty at `cleared_revision` either way.
    Queued {
        /// Thread the message is queued in: the draft's own thread, or the
        /// thread a project draft's submission created.
        thread_id: ThreadId,
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
    /// The stored draft is at another revision (`None`: the scope has no
    /// draft); nothing was queued. The sender saves its draft again and
    /// submits the revision that save is given.
    Stale {
        /// The scope's current draft revision.
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
    /// Scope whose draft was submitted.
    pub scope: ComposerDraftScope,
    /// The revision the command named.
    pub draft_revision: ComposerDraftRevision,
    /// What the Forge did.
    pub outcome: DraftSubmissionOutcome,
}
