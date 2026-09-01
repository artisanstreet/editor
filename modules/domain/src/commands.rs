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
use crate::identifiers::{DirectoryId, ProjectId, RequestId, ThreadId};
use crate::text::{MessageBody, ThreadTitle};

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
    /// See [`SetThreadEngineConfig`].
    SetThreadEngineConfig(Box<SetThreadEngineConfig>),
}

impl Command {
    /// Returns the client-minted request identity carried by this command.
    #[must_use]
    pub const fn request_id(&self) -> &RequestId {
        match self {
            Self::AttachProject(command) => &command.request_id,
            Self::CreateThread(command) => &command.request_id,
            Self::QueueFirstMessage(command) => &command.request_id,
            Self::SetThreadEngineConfig(command) => command.request_id(),
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
}
