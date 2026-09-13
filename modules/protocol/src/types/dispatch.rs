//! First-workflow client requests and their correlated receipt records.

#![forbid(unsafe_code)]

use artisan_domain::{
    Command, ConversationCursor, ConversationRequest, ConversationSubscriptionStart, DirectoryId,
    EngineConfigRevision, EngineId, MessageId, ObservationId, Query, ReceiptDisposition, RequestId,
    RunId, ThreadId,
};

use crate::repository::ProjectRepositoryQuery;

use super::catalog::ResolveRichLinkRequest;
use super::handshake::LifecycleRequest;
/// First-workflow request payload after frame correlation is separated out.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClientRequest {
    /// Pure domain read.
    Query(Query),
    /// Idempotent domain mutation.
    Command(Command),
    /// Bounded conversation read or subscription control.
    Conversation(ConversationRequest),
    /// Explicit host interaction: ask the local Forge process to show its
    /// native directory picker once. Deliberately outside the pure domain
    /// [`Query`] and durable [`Command`] vocabularies: nothing durable is
    /// created or mutated, the request must not be automatically replayed,
    /// and every deliberate new attempt uses a fresh frame identity -- a
    /// fresh [`FrameId`] as its wire `Envelope.messageId`, unlike the stable
    /// verbatim-retry identity of durable commands. This schema slice
    /// implements neither duplicate-request suppression nor cancellation
    /// propagation.
    PickDirectory,
    /// Validate a host-native directory chosen by the authenticated editor.
    ValidateDirectory(artisan_domain::RootPath),
    /// Negotiated native lifecycle status or stop control.
    Lifecycle(LifecycleRequest),
    /// Bounded rich-link metadata read for one absolute HTTP(S) URL.
    ///
    /// Not a durable command: nothing is persisted, a resolve may be retried
    /// or dropped freely, and each deliberate attempt uses a fresh frame
    /// identity.
    ResolveRichLink(ResolveRichLinkRequest),
    /// Bounded repository-identity read for named attached projects.
    ///
    /// Not a durable command: the read inspects the attached project roots on
    /// demand, persists nothing, and uses a fresh frame identity per attempt.
    QueryProjectRepository(ProjectRepositoryQuery),
}

/// Successful durable thread engine-configuration mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SetThreadEngineConfigResult {
    /// Stable client request identity echoed by the nested result.
    pub request_id: RequestId,
    /// Thread whose configuration was changed.
    pub thread_id: ThreadId,
    /// Resulting one-based configuration revision.
    pub revision: EngineConfigRevision,
    /// Newly accepted or exact duplicate replay.
    pub disposition: ReceiptDisposition,
}

/// Receipt returned when a first message is durably queued.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FirstMessageReceipt {
    /// Stable client correlation identity.
    pub request_id: RequestId,
    /// Forge-minted durable message identity.
    pub message_id: MessageId,
    /// Owning thread.
    pub thread_id: ThreadId,
    /// Accepted or exact duplicate replay.
    pub disposition: artisan_domain::ReceiptDisposition,
}

/// Receipt returned when a general text/image message is durably queued.
///
/// This deliberately has a distinct public type from [`FirstMessageReceipt`]
/// so callers cannot infer first-message-only uniqueness from the response
/// shape. The identity fields retain the same replay contract.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueueMessageReceipt {
    /// Stable client correlation identity.
    pub request_id: RequestId,
    /// Forge-minted durable message identity.
    pub message_id: MessageId,
    /// Owning thread.
    pub thread_id: ThreadId,
    /// Accepted or exact duplicate replay.
    pub disposition: artisan_domain::ReceiptDisposition,
}

/// Disposition of one exact live-run cancellation signal.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum StopRunDisposition {
    /// The first cancellation signal was published to the live run.
    Requested,
    /// The live run had already received a cancellation signal.
    AlreadyRequested,
    /// No live registry entry matched the requested thread/run pair.
    NotActive,
}

/// Correlated result of one exact live-run cancellation request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StopRunReceipt {
    /// Stable client request identity echoed by the enclosing response.
    pub request_id: RequestId,
    /// Thread supplied by the caller.
    pub thread_id: ThreadId,
    /// Exact run supplied by the caller.
    pub run_id: RunId,
    /// Signal disposition; this never claims durable terminal completion.
    pub disposition: StopRunDisposition,
}

/// Disposition of one live approval/question response against its target.
///
/// These are per-target routing results for a well-formed, authenticated
/// request, not wire rejections: the request was valid, but its target may
/// be absent, already settled, or owned by another run.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RunInteractionOutcome {
    /// The decision was recorded and delivered to the owning run.
    Applied,
    /// No pending request carries this target id on the live run.
    UnknownTarget,
    /// The target was already resolved by an earlier response.
    AlreadyResolved,
    /// The named thread/run pair is not the live owning run.
    WrongRun,
}

/// Correlated result of one approval response.
///
/// The nested request id must equal the enclosing response request id
/// exactly; the decision echoes so a replay can prove it answers the
/// identical intent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RespondApprovalReceipt {
    /// Stable client request identity echoed by the enclosing response.
    pub request_id: RequestId,
    /// Thread supplied by the caller.
    pub thread_id: ThreadId,
    /// Exact run supplied by the caller.
    pub run_id: RunId,
    /// Provider approval identity that was answered.
    pub approval_id: ObservationId,
    /// The explicit decision that was recorded.
    pub approved: bool,
    /// How the response settled its target.
    pub outcome: RunInteractionOutcome,
    /// Accepted now or exact duplicate replay.
    pub disposition: ReceiptDisposition,
}

/// Correlated result of one question response.
///
/// Same correlation and intent-echo contract as the approval receipt; an
/// empty answer list echoes an explicitly skipped question.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RespondQuestionReceipt {
    /// Stable client request identity echoed by the enclosing response.
    pub request_id: RequestId,
    /// Thread supplied by the caller.
    pub thread_id: ThreadId,
    /// Exact run supplied by the caller.
    pub run_id: RunId,
    /// Provider question identity that was answered.
    pub question_id: ObservationId,
    /// The explicit answers that were recorded.
    pub answers: Vec<String>,
    /// How the response settled its target.
    pub outcome: RunInteractionOutcome,
    /// Accepted now or exact duplicate replay.
    pub disposition: ReceiptDisposition,
}

/// Authoritative live-run query result for one thread.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ActiveRunResult {
    /// No exact live run is registered for the thread.
    NoActive { thread_id: ThreadId },
    /// Exactly one exact live run is registered for the thread.
    ///
    /// `status` and `engine_id` always describe the live run: the current
    /// backend emits all three live statuses plus the engine, and wire
    /// decode rejects anything else (native QUIC is a same-version
    /// build; there is no cross-version unknown tolerance here).
    Active {
        thread_id: ThreadId,
        run_id: RunId,
        status: RunLiveStatus,
        engine_id: EngineId,
    },
}

/// Live lifecycle of the registered run, for the starting-guard.
///
/// Maps the durable run lifecycle (`assistant_run.lifecycle`):
/// `Queued|Launching` ⇒ `Queued`, `Running` ⇒ `Running`,
/// `Waiting|CancelRequested` ⇒ `Waiting`. Settled lifecycles surface
/// as [`ActiveRunResult::NoActive`], never here.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RunLiveStatus {
    /// The run is accepted but its turn has not started.
    Queued,
    /// The run's turn is executing.
    Running,
    /// The run's turn is waiting (for example on approval input).
    Waiting,
}

/// Successful start of authoritative conversation delivery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConversationSubscriptionStarted {
    /// Fresh delivery begins with a complete validated snapshot.
    Fresh(ConversationSubscriptionStart),
    /// Resumed delivery continues strictly after an already-applied cursor.
    Resumed {
        /// Thread whose delivery is resuming.
        thread_id: ThreadId,
        /// Last patch already applied by the subscriber.
        cursor: ConversationCursor,
    },
}

/// Successful stop of authoritative conversation delivery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConversationSubscriptionStopped {
    /// Thread no longer being delivered.
    pub thread_id: ThreadId,
}

/// Validated outcome of one explicit directory-picker interaction.
///
/// [`DirectoryPickOutcome::Selected`] carries only the opaque, validated
/// [`DirectoryId`] of the chosen directory: never a filesystem path, label,
/// enumeration, or child flag. [`DirectoryPickOutcome::Cancelled`] reports
/// an actual user dismissal of the picker rather than a request cancellation
/// or a dropped frame; cancellation propagation and late-response handling
/// stay explicitly outside this slice.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DirectoryPickOutcome {
    /// The user chose a directory.
    Selected(DirectoryId),
    /// The user dismissed the picker.
    Cancelled,
}
