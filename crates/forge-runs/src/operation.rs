use crate::error::{self, ErrorClass, Refused};
use crate::runtime::{PeerError, RegistryFileError};
use artisan_api::{EngineId, ItemId, ThreadId};

#[derive(Debug, thiserror::Error)]
pub enum InputError {
    #[error(transparent)]
    Text(#[from] forge_store::ResolveTextError),
    #[error(transparent)]
    Thread(#[from] forge_store::ReadThreadError),
    #[error("{engine} cannot read the command in message {item} of thread {thread}")]
    Command {
        thread: ThreadId,
        item: ItemId,
        engine: EngineId,
        #[source]
        source: forge_engines::MissingPrompt,
    },
    #[error(transparent)]
    Images(#[from] forge_store::InputImagesError),
}

impl InputError {
    #[must_use]
    pub const fn class(&self) -> ErrorClass {
        match self {
            Self::Text(text) => error::content(&text.cause),
            Self::Command { .. } => ErrorClass::BadRequest,
            Self::Thread(_) | Self::Images(_) => ErrorClass::Internal,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SendError {
    #[error(transparent)]
    ReadThread(#[from] forge_store::ReadThreadError),
    #[error("thread {thread} does not exist")]
    ThreadNotFound { thread: ThreadId },
    #[error("{engine} cannot read the command sent to thread {thread}")]
    Command {
        thread: ThreadId,
        engine: EngineId,
        #[source]
        source: forge_engines::MissingPrompt,
    },
    #[error(transparent)]
    Store(#[from] forge_store::SendMessageError),
    #[error("hand message {item} to the live run of thread {thread}")]
    Steer {
        thread: ThreadId,
        item: ItemId,
        #[source]
        source: Box<InputError>,
    },
    #[error("hand message {item} for thread {thread} to the generation running it")]
    Peer {
        thread: ThreadId,
        item: ItemId,
        #[source]
        source: Box<PeerError>,
    },
    #[error("check where thread {thread} runs")]
    Route {
        thread: ThreadId,
        #[source]
        source: RegistryFileError,
    },
}

impl SendError {
    #[must_use]
    pub const fn class(&self) -> ErrorClass {
        match self {
            Self::ThreadNotFound { .. } => ErrorClass::NotFound,
            Self::Command { .. } => ErrorClass::BadRequest,
            Self::Store(store) => error::input(&store.cause),
            Self::Steer { source, .. } => source.class(),
            Self::Peer { source, .. } => source.class(),
            Self::ReadThread(_) | Self::Route { .. } => ErrorClass::Internal,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AnswerError {
    #[error(transparent)]
    ReadItem(#[from] forge_store::ReadItemError),
    #[error("item {item} does not exist")]
    ItemNotFound { item: ItemId },
    #[error(transparent)]
    Validate(#[from] forge_store::ValidateAnswersError),
    #[error("answer item {item}")]
    Refused {
        item: ItemId,
        #[source]
        refused: Refused,
    },
    #[error(transparent)]
    Save(#[from] forge_store::AnswerQuestionError),
    #[error(transparent)]
    NativeId(#[from] forge_store::NativeIdError),
    #[error("item {item} waits for an answer but has no engine id to answer")]
    NoEngineId { item: ItemId },
    #[error("hand the answer to item {item} to the generation running it")]
    Peer {
        item: ItemId,
        #[source]
        source: Box<PeerError>,
    },
    #[error("check where the thread of item {item} runs")]
    Route {
        item: ItemId,
        #[source]
        source: RegistryFileError,
    },
}

impl AnswerError {
    #[must_use]
    pub const fn class(&self) -> ErrorClass {
        match self {
            Self::ItemNotFound { .. } => ErrorClass::NotFound,
            Self::Refused { .. } => ErrorClass::Conflict,
            Self::Validate(validate) => error::answer(&validate.cause),
            Self::Save(save) => error::answer(&save.cause),
            Self::Peer { source, .. } => source.class(),
            Self::ReadItem(_)
            | Self::NativeId(_)
            | Self::NoEngineId { .. }
            | Self::Route { .. } => ErrorClass::Internal,
        }
    }

    pub(crate) fn refused(item: &ItemId, message: impl Into<String>) -> Self {
        Self::Refused {
            item: item.clone(),
            refused: Refused(message.into()),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ContinueError {
    #[error("continue thread {thread}")]
    Refused {
        thread: ThreadId,
        #[source]
        refused: Refused,
    },
    #[error(transparent)]
    LatestRun(#[from] forge_store::LatestRunError),
    #[error(transparent)]
    Restore(#[from] forge_store::RestoreInputsError),
    #[error(transparent)]
    Request(#[from] forge_store::RequestContinueError),
}

impl ContinueError {
    #[must_use]
    pub const fn class(&self) -> ErrorClass {
        match self {
            Self::Refused { .. } => ErrorClass::Conflict,
            Self::LatestRun(_) | Self::Restore(_) | Self::Request(_) => ErrorClass::Internal,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error(transparent)]
    ReadThread(#[from] forge_store::ReadThreadError),
    #[error(transparent)]
    Store(#[from] forge_store::UpdateThreadError),
    #[error("hand the update of thread {thread} to the generation running it")]
    Peer {
        thread: ThreadId,
        #[source]
        source: Box<PeerError>,
    },
    #[error("check where thread {thread} runs")]
    Route {
        thread: ThreadId,
        #[source]
        source: RegistryFileError,
    },
}

impl UpdateError {
    #[must_use]
    pub const fn class(&self) -> ErrorClass {
        match self {
            Self::Store(store) => error::update_thread(&store.cause),
            Self::Peer { source, .. } => source.class(),
            Self::ReadThread(_) | Self::Route { .. } => ErrorClass::Internal,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ClearGoalError {
    #[error(transparent)]
    ReadThread(#[from] forge_store::ReadThreadError),
    #[error("thread {thread} does not exist")]
    ThreadNotFound { thread: ThreadId },
    #[error(transparent)]
    Inspector(#[from] forge_store::InspectorError),
    #[error("clear the goal of thread {thread}")]
    Refused {
        thread: ThreadId,
        #[source]
        refused: Refused,
    },
    #[error("send /goal clear to thread {thread}")]
    Send {
        thread: ThreadId,
        #[source]
        source: Box<SendError>,
    },
    #[error(transparent)]
    LatestRun(#[from] forge_store::LatestRunError),
    #[error("prepare {engine} to clear the goal of thread {thread}")]
    Prepare {
        thread: ThreadId,
        engine: EngineId,
        #[source]
        source: Box<crate::run::PrepareError>,
    },
    #[error("{engine} clearing the goal of thread {thread}")]
    Engine {
        thread: ThreadId,
        engine: EngineId,
        #[source]
        source: forge_engines::ClearGoalError,
    },
    #[error(transparent)]
    Notice(#[from] forge_store::UpsertItemError),
    #[error("hand clearing the goal of thread {thread} to the generation running it")]
    Peer {
        thread: ThreadId,
        #[source]
        source: Box<PeerError>,
    },
    #[error("check where thread {thread} runs")]
    Route {
        thread: ThreadId,
        #[source]
        source: RegistryFileError,
    },
}

impl ClearGoalError {
    #[must_use]
    pub const fn class(&self) -> ErrorClass {
        match self {
            Self::ThreadNotFound { .. } => ErrorClass::NotFound,
            Self::Refused { .. } => ErrorClass::Conflict,
            Self::Send { source, .. } => source.class(),
            Self::Peer { source, .. } => source.class(),
            Self::ReadThread(_)
            | Self::Inspector(_)
            | Self::LatestRun(_)
            | Self::Prepare { .. }
            | Self::Engine { .. }
            | Self::Notice(_)
            | Self::Route { .. } => ErrorClass::Internal,
        }
    }

    pub(crate) fn refused(thread: &ThreadId, message: impl Into<String>) -> Self {
        Self::Refused {
            thread: thread.clone(),
            refused: Refused(message.into()),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CancelError {
    #[error(transparent)]
    Tree(#[from] forge_store::AgentThreadsError),
    #[error(transparent)]
    Inputs(#[from] forge_store::CancelInputsError),
    #[error(transparent)]
    Unpark(#[from] forge_store::UnparkError),
    #[error("hand cancelling thread {thread} to the generation running it")]
    Peer {
        thread: ThreadId,
        #[source]
        source: Box<PeerError>,
    },
    #[error("check where thread {thread} runs")]
    Route {
        thread: ThreadId,
        #[source]
        source: RegistryFileError,
    },
}

impl CancelError {
    #[must_use]
    pub const fn class(&self) -> ErrorClass {
        match self {
            Self::Peer { source, .. } => source.class(),
            Self::Tree(_) | Self::Inputs(_) | Self::Unpark(_) | Self::Route { .. } => {
                ErrorClass::Internal
            }
        }
    }
}
