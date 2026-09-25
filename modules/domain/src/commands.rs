//! Commands and queries of the first native workflow.
//!
//! Commands are mutations; every one carries a client-minted
//! [`RequestId`] so retries correlate to a receipt disposition instead of a
//! second effect. Forge-minted identities never appear as creation inputs:
//! attaching names only an opaque directory, thread creation names only the
//! project it belongs to, and queueing names only the existing target thread.
//! Queries cover exactly what this milestone selects: attached-project
//! rediscovery, directory browsing, and project-scoped thread listing.

use crate::engine_config::{EngineConfigUpdatePrecondition, EngineRunConfig};
use crate::identifiers::{DirectoryId, MessageId, ProjectId, RequestId, RunId, ThreadId};
use crate::message::QueueMessagePayload;
use crate::run_interaction::{RespondApproval, RespondQuestion};
use crate::text::{MessageBody, ThreadTitle};
use crate::{
    ImportLegacyPreferences, ListFailedMessages, ListQueuedMessages, QueueStoredMessage,
    ReadAccountUsage, ReadComposerAttachment, ReadComposerDraft, ReadRecalledMessage, ReadRunUsage,
    ReadUserPreferences, RecordNavigation, RecoverFailedMessage, RetryFailedMessage,
    SaveComposerDraft, SubmitComposerDraft, UploadComposerAttachment, WithdrawQueuedMessageCommand,
};

pub use crate::catalog_selection::ResolveModelSelection;
pub use crate::composer_catalog::{
    ReadComposerCatalog, ReadHostCatalog, ReadModelFavorites, SetModelFavorite,
};

/// Attaches one Forge-visible directory, minting its project identity.
///
/// Legacy analogue: `project.directory.select` resolving then attaching a
/// folder addressed purely by opaque identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttachProject {
    /// Client-minted stable request identity for this mutation.
    pub request_id: RequestId,
    /// Opaque directory identity selected by the client.
    pub directory_id: DirectoryId,
}

/// Creates one project-scoped thread.
///
/// The thread id is absent on purpose: Forge mints it during acceptance and
/// returns it through the `ThreadCreated` event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CreateThread {
    /// Client-minted stable request identity for this mutation.
    pub request_id: RequestId,
    /// Attached project the new thread belongs to.
    pub project_id: ProjectId,
    /// Validated title for the new thread.
    pub title: ThreadTitle,
}

/// Durably queues the first bounded text message on an existing thread.
///
/// The message id is absent on purpose: Forge mints it at acceptance. Engine
/// dispatch is explicitly outside this milestone, so no engine, run, or
/// provider field exists here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QueueFirstMessage {
    /// Client-minted stable request identity for this mutation.
    pub request_id: RequestId,
    /// Existing thread receiving the first message.
    pub thread_id: ThreadId,
    /// Validated, bounded body of the first message.
    pub body: MessageBody,
}

/// Durably queues one text and/or image message on an existing thread.
///
/// Unlike [`QueueFirstMessage`], this command is not restricted to ordinal
/// zero. Its payload is validated before admission and retains image bytes in
/// authored order through persistence and engine dispatch.
///
/// An optional [`SteerTarget`] names the live run the sender observed when
/// the message was typed on the same engine. A named message must reach
/// that live run or fail typed with its payload preserved; it must never
/// silently start a fresh run. `None` is a fresh send in every state.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct QueueMessage {
    /// Client-minted stable request identity for this mutation.
    pub request_id: RequestId,
    /// Existing thread receiving the message.
    pub thread_id: ThreadId,
    /// Authored text and ordered owned image attachments.
    pub payload: QueueMessagePayload,
    /// Observed live run to steer into, or `None` for a fresh send.
    pub steer_target: Option<SteerTarget>,
}

/// Names the observed live run a message must steer into.
///
/// The thread is implied by the owning [`QueueMessage`]; the engine match
/// is revalidated at dispatch from durable state, never trusted from the
/// wire alone.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SteerTarget {
    /// Observed live run identity at send time.
    run_id: RunId,
}

impl SteerTarget {
    /// Names an observed live run as the steer target.
    #[must_use]
    pub const fn new(run_id: RunId) -> Self {
        Self { run_id }
    }

    /// Returns the observed live run identity.
    #[must_use]
    pub const fn run_id(&self) -> &RunId {
        &self.run_id
    }
}

impl QueueMessage {
    /// Creates a general message command with an already validated payload.
    #[must_use]
    pub const fn new(
        request_id: RequestId,
        thread_id: ThreadId,
        payload: QueueMessagePayload,
    ) -> Self {
        Self {
            request_id,
            thread_id,
            payload,
            steer_target: None,
        }
    }

    /// Names the observed live run this message must steer into.
    ///
    /// Naming is frontend-observed intent only; dispatch revalidates
    /// liveness and the same-engine rule before delivery, and fails typed
    /// otherwise. Retries must preserve the original target, never re-name.
    #[must_use]
    pub fn with_steer_target(mut self, target: SteerTarget) -> Self {
        self.steer_target = Some(target);
        self
    }

    /// Returns the observed live run to steer into, if named.
    #[must_use]
    pub const fn steer_target(&self) -> Option<&SteerTarget> {
        self.steer_target.as_ref()
    }

    /// Returns the client request identity.
    #[must_use]
    pub const fn request_id(&self) -> &RequestId {
        &self.request_id
    }

    /// Returns the target thread identity.
    #[must_use]
    pub const fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }

    /// Returns the immutable validated message payload.
    #[must_use]
    pub const fn payload(&self) -> &QueueMessagePayload {
        &self.payload
    }
}

/// Requests cancellation of one exact live native run.
///
/// This is a live routing request, not a durable completion receipt. The
/// backend answers from its process-wide run registry and the existing
/// engine-owner path settles the durable run separately.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct StopRun {
    /// Client-minted stable request identity for this cancellation request.
    pub request_id: RequestId,
    /// Thread that must own the exact target run.
    pub thread_id: ThreadId,
    /// Exact native run identity to signal.
    pub run_id: RunId,
}

impl StopRun {
    /// Creates an exact thread/run cancellation request.
    #[must_use]
    pub const fn new(request_id: RequestId, thread_id: ThreadId, run_id: RunId) -> Self {
        Self {
            request_id,
            thread_id,
            run_id,
        }
    }

    /// Returns the stable request identity.
    #[must_use]
    pub const fn request_id(&self) -> &RequestId {
        &self.request_id
    }

    /// Returns the authenticated owning thread.
    #[must_use]
    pub const fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }

    /// Returns the exact target run.
    #[must_use]
    pub const fn run_id(&self) -> &RunId {
        &self.run_id
    }
}

/// Changes the complete engine configuration for one existing thread.
///
/// The fields remain private so a caller cannot accidentally omit the
/// optimistic precondition or mutate a configuration after it is accepted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SetThreadEngineConfig {
    request_id: RequestId,
    thread_id: ThreadId,
    precondition: EngineConfigUpdatePrecondition,
    config: EngineRunConfig,
}

impl SetThreadEngineConfig {
    /// Constructs a complete authenticated configuration mutation.
    #[must_use]
    pub fn new(
        request_id: RequestId,
        thread_id: ThreadId,
        precondition: EngineConfigUpdatePrecondition,
        config: EngineRunConfig,
    ) -> Self {
        Self {
            request_id,
            thread_id,
            precondition,
            config,
        }
    }

    /// Returns the request identity.
    #[must_use]
    pub const fn request_id(&self) -> &RequestId {
        &self.request_id
    }

    /// Returns the target thread identity.
    #[must_use]
    pub const fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }

    /// Returns the optimistic precondition.
    #[must_use]
    pub const fn precondition(&self) -> EngineConfigUpdatePrecondition {
        self.precondition
    }

    /// Returns the immutable configuration.
    #[must_use]
    pub const fn config(&self) -> &EngineRunConfig {
        &self.config
    }
}

/// Every mutation of the first native workflow.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Command {
    /// See [`AttachProject`].
    AttachProject(AttachProject),
    /// See [`CreateThread`].
    CreateThread(CreateThread),
    /// See [`QueueFirstMessage`].
    QueueFirstMessage(QueueFirstMessage),
    /// See [`QueueMessage`].
    QueueMessage(QueueMessage),
    /// See [`StopRun`].
    StopRun(StopRun),
    /// See [`SetModelFavorite`].
    SetModelFavorite(SetModelFavorite),
    WithdrawQueuedMessage(WithdrawQueuedMessageCommand),
    /// See [`RespondApproval`].
    RespondApproval(RespondApproval),
    /// See [`RespondQuestion`].
    RespondQuestion(RespondQuestion),
    /// See [`SetThreadEngineConfig`].
    SetThreadEngineConfig(Box<SetThreadEngineConfig>),
    /// See [`SaveComposerDraft`].
    SaveComposerDraft(SaveComposerDraft),
    /// See [`UploadComposerAttachment`].
    UploadComposerAttachment(UploadComposerAttachment),
    /// See [`QueueStoredMessage`].
    QueueStoredMessage(QueueStoredMessage),
    /// See [`RetryFailedMessage`].
    RetryFailedMessage(RetryFailedMessage),
    /// See [`RecoverFailedMessage`].
    RecoverFailedMessage(RecoverFailedMessage),
    /// See [`SubmitComposerDraft`].
    SubmitComposerDraft(SubmitComposerDraft),
    /// See [`RecordNavigation`].
    RecordNavigation(RecordNavigation),
    /// See [`ImportLegacyPreferences`].
    ImportLegacyPreferences(ImportLegacyPreferences),
}

impl Command {
    /// Returns the client-minted request identity carried by this command.
    #[must_use]
    pub const fn request_id(&self) -> &RequestId {
        match self {
            Self::AttachProject(command) => &command.request_id,
            Self::CreateThread(command) => &command.request_id,
            Self::QueueFirstMessage(command) => &command.request_id,
            Self::QueueMessage(command) => &command.request_id,
            Self::StopRun(command) => &command.request_id,
            Self::SetModelFavorite(command) => command.request_id(),
            Self::WithdrawQueuedMessage(command) => command.request_id(),
            Self::RespondApproval(command) => command.request_id(),
            Self::RespondQuestion(command) => command.request_id(),
            Self::SetThreadEngineConfig(command) => command.request_id(),
            Self::SaveComposerDraft(command) => command.request_id(),
            Self::UploadComposerAttachment(command) => &command.request_id,
            Self::QueueStoredMessage(command) => command.request_id(),
            Self::RetryFailedMessage(command) => &command.request_id,
            Self::RecoverFailedMessage(command) => &command.request_id,
            Self::SubmitComposerDraft(command) => &command.request_id,
            Self::RecordNavigation(command) => &command.request_id,
            Self::ImportLegacyPreferences(command) => &command.request_id,
        }
    }
}

/// Lists Forge-visible directories, optionally below one parent.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ListDirectories {
    /// Parent directory whose entries are listed, or none for root views.
    pub parent: Option<DirectoryId>,
}

/// Lists the threads scoped to one attached project.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ListProjectThreads {
    /// Project whose threads are listed.
    pub project_id: ProjectId,
}

/// Lists every currently attached project.
///
/// The rediscovery read of the milestone: a returning client asks once,
/// carrying no identity at all, and Forge answers with the complete
/// attached-project catalog. Legacy analogue: `ProjectCatalogSnapshot`
/// served on session start (`modules/protocol/src/project.ts`).
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ListAttachedProjects;

/// Reads the persisted engine configuration for one existing thread.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ReadThreadEngineSettings {
    thread_id: ThreadId,
}

impl ReadThreadEngineSettings {
    /// Constructs a pure read naming exactly one existing thread.
    #[must_use]
    pub fn new(thread_id: ThreadId) -> Self {
        Self { thread_id }
    }

    /// Returns the target thread identity.
    #[must_use]
    pub const fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }
}

/// Reads one owned image attachment by its authenticated thread, message, and
/// authored position. The result is bounded by the native image limits and
/// never permits a client filesystem path or URI.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ReadMessageImage {
    thread_id: ThreadId,
    message_id: MessageId,
    index: u32,
}

impl ReadMessageImage {
    /// Constructs a single-image ownership query.
    #[must_use]
    pub const fn new(thread_id: ThreadId, message_id: MessageId, index: u32) -> Self {
        Self {
            thread_id,
            message_id,
            index,
        }
    }

    /// Returns the authenticated owning thread.
    #[must_use]
    pub const fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }

    /// Returns the persisted message identity.
    #[must_use]
    pub const fn message_id(&self) -> &MessageId {
        &self.message_id
    }

    /// Returns the zero-based authored attachment position.
    #[must_use]
    pub const fn index(&self) -> u32 {
        self.index
    }
}

/// Reads the authoritative live native run for one thread, if exactly one is
/// registered. Multiple live runs are a fail-closed backend condition rather
/// than a reason to guess the newest run.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ReadActiveRun {
    thread_id: ThreadId,
}

impl ReadActiveRun {
    /// Constructs a bounded live-run query.
    #[must_use]
    pub const fn new(thread_id: ThreadId) -> Self {
        Self { thread_id }
    }

    /// Returns the thread being queried.
    #[must_use]
    pub const fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }
}

/// Lists every registered native engine profile.
///
/// The registry may be absent, empty, or contain up to 64 ordered profile
/// identifiers. This query carries no thread, database path, home kind,
/// engine path, or request identity; correlation stays on the triggering
/// frame.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ListRegisteredEngineProfiles;

/// Every query of the first native workflow.
///
/// Queries carry no [`RequestId`]: they are pure reads with no durable effect
/// to receipt.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum Query {
    /// See [`ListDirectories`].
    ListDirectories(ListDirectories),
    /// See [`ListProjectThreads`].
    ListProjectThreads(ListProjectThreads),
    /// See [`ListAttachedProjects`].
    ListAttachedProjects(ListAttachedProjects),
    /// See [`ReadThreadEngineSettings`].
    ReadThreadEngineSettings(ReadThreadEngineSettings),
    /// See [`ListRegisteredEngineProfiles`].
    ListRegisteredEngineProfiles(ListRegisteredEngineProfiles),
    /// See [`ReadMessageImage`].
    ReadMessageImage(ReadMessageImage),
    /// See [`ReadActiveRun`].
    ReadActiveRun(ReadActiveRun),
    /// See [`ReadComposerCatalog`].
    ReadComposerCatalog(ReadComposerCatalog),
    /// See [`ReadModelFavorites`].
    ReadModelFavorites(ReadModelFavorites),
    /// See [`ReadHostCatalog`].
    ReadHostCatalog(ReadHostCatalog),
    /// See [`ResolveModelSelection`].
    ResolveModelSelection(ResolveModelSelection),
    ListQueuedMessages(ListQueuedMessages),
    /// Terminally failed dispatches for one thread, newest first.
    ListFailedMessages(ListFailedMessages),
    ReadRecalledMessage(ReadRecalledMessage),
    ReadRunUsage(ReadRunUsage),
    /// See [`ReadAccountUsage`].
    ReadAccountUsage(ReadAccountUsage),
    /// See [`ReadComposerDraft`].
    ReadComposerDraft(ReadComposerDraft),
    /// See [`ReadComposerAttachment`].
    ReadComposerAttachment(ReadComposerAttachment),
    /// See [`ReadUserPreferences`].
    ReadUserPreferences(ReadUserPreferences),
}
