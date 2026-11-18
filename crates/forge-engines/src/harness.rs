use artisan_api::{
    Answer, EngineConfig, EngineId, ItemId, ItemKind, McpServer, RunOutcome, RunStatus, Status,
    ThreadId, Timestamp, Usage,
};
use futures::future::BoxFuture;
use std::path::PathBuf;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
pub struct Turn {
    pub thread_id: ThreadId,
    pub engine: EngineId,
    pub binary: PathBuf,
    pub home: PathBuf,
    pub cwd: PathBuf,
    pub config: EngineConfig,
    pub session_id: Option<String>,
    pub input: UserInput,
    pub launch: Launch,
    pub trace_dir: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UserInput {
    pub id: ItemId,
    pub text: String,
    pub images: Vec<PathBuf>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Launch {
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub gateway_mcp: Vec<(String, String)>,
    pub mcp_servers: Vec<McpServer>,
    pub readable: Vec<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Input {
    Steer(UserInput),
    Answer { key: String, answer: Answer },
    ClearGoal,
    SetPermissions(artisan_api::Permissions),
}

#[expect(
    clippy::large_enum_variant,
    reason = "nearly every event is an `Item`; see above"
)]
#[derive(Clone, Debug, PartialEq)]
pub enum EngineEvent {
    Session {
        id: String,
    },
    SessionGone,
    Model {
        id: String,
    },
    Title {
        title: String,
    },
    Item {
        key: String,
        parent: Option<String>,
        status: Status,
        kind: ItemKind,
    },
    Image {
        key: String,
        parent: Option<String>,
        path: PathBuf,
        caption: Option<String>,
    },
    Status {
        key: String,
        status: Status,
    },
    Live {
        target: LiveKind,
        text: String,
        append: bool,
    },
    Usage(Usage),
    Commands(Vec<artisan_api::SlashCommand>),
    InputAccepted {
        id: ItemId,
    },
}

impl EngineEvent {
    #[must_use]
    pub fn background_wait(engine: artisan_api::EngineId, waiting: bool) -> Self {
        let name = match engine {
            artisan_api::EngineId::Claude => "Claude",
            artisan_api::EngineId::Codex => "Codex",
            artisan_api::EngineId::OpenCode => "OpenCode",
        };
        Self::Live {
            target: LiveKind::Status,
            text: if waiting {
                format!("{name} is waiting on a background job")
            } else {
                String::new()
            },
            append: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LiveKind {
    Text { key: String },
    Reasoning,
    Narration,
    Shell { key: String },
    Progress { key: String },
    Status,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Done,
    Cancelled,
    Refused,
    Failed(Failure),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub message: String,
    pub retry: Option<Retry>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Retry {
    Overloaded,
    UsageLimit { resets_at: Option<Timestamp> },
}

impl Outcome {
    #[must_use]
    pub const fn done() -> Self {
        Self::Done
    }

    #[must_use]
    pub const fn cancelled() -> Self {
        Self::Cancelled
    }

    #[must_use]
    pub fn failed(message: impl Into<String>) -> Self {
        Self::Failed(Failure {
            message: message.into(),
            retry: None,
        })
    }

    #[must_use]
    pub fn retryable(message: impl Into<String>, retry: Retry) -> Self {
        Self::Failed(Failure {
            message: message.into(),
            retry: Some(retry),
        })
    }

    #[must_use]
    pub const fn run_outcome(&self) -> RunOutcome {
        match self {
            Self::Done => RunOutcome::Done,
            Self::Cancelled => RunOutcome::Cancelled,
            Self::Refused => RunOutcome::Refused,
            Self::Failed(_) => RunOutcome::Failed,
        }
    }

    #[must_use]
    pub const fn status(&self) -> RunStatus {
        self.run_outcome().status()
    }

    #[must_use]
    pub fn error(&self) -> Option<&str> {
        self.failure().map(|failure| failure.message.as_str())
    }

    #[must_use]
    pub fn retry(&self) -> Option<Retry> {
        self.failure().and_then(|failure| failure.retry)
    }

    #[must_use]
    pub const fn failure(&self) -> Option<&Failure> {
        match self {
            Self::Failed(failure) => Some(failure),
            Self::Done | Self::Cancelled | Self::Refused => None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SideQueryError {
    #[error("{engine} has no side queries")]
    Unsupported { engine: EngineId },
    #[error("the side query on {engine} thread {thread} was cancelled")]
    Cancelled { engine: EngineId, thread: ThreadId },
    #[error("Claude side query for thread {thread}")]
    Claude {
        thread: ThreadId,
        #[source]
        source: Box<crate::claude::SideQueryError>,
    },
    #[error("Codex side query for thread {thread}")]
    Codex {
        thread: ThreadId,
        #[source]
        source: Box<crate::codex::SideQueryError>,
    },
    #[error("OpenCode side query for thread {thread}")]
    OpenCode {
        thread: ThreadId,
        #[source]
        source: Box<crate::opencode::SideQueryError>,
    },
    #[error("claude.ai side query for thread {thread}")]
    ClaudeAi {
        thread: ThreadId,
        #[source]
        source: Box<crate::claudeai::SideQueryError>,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum SetTitleError {
    #[error("name Claude thread {thread}")]
    Claude {
        thread: ThreadId,
        #[source]
        source: Box<crate::claude::SetTitleError>,
    },
    #[error("name Codex thread {thread}")]
    Codex {
        thread: ThreadId,
        #[source]
        source: Box<crate::codex::SetTitleError>,
    },
    #[error("name OpenCode thread {thread}")]
    OpenCode {
        thread: ThreadId,
        #[source]
        source: Box<crate::opencode::SetTitleError>,
    },
    #[error("name claude.ai thread {thread}")]
    ClaudeAi {
        thread: ThreadId,
        #[source]
        source: Box<crate::claudeai::SetTitleError>,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ClearGoalError {
    #[error("{engine} can't clear a goal")]
    Unsupported { engine: EngineId },
    #[error("clear the goal of Codex thread {thread}")]
    Codex {
        thread: ThreadId,
        #[source]
        source: Box<crate::codex::ClearGoalError>,
    },
    #[error("clear the goal of OpenCode thread {thread}")]
    OpenCode {
        thread: ThreadId,
        #[source]
        source: Box<crate::opencode::ClearGoalError>,
    },
}

pub struct RunIo {
    pub inbox: mpsc::Receiver<Input>,
    pub events: mpsc::Sender<EngineEvent>,
    pub cancel: CancellationToken,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SideReply {
    pub text: String,
    pub usage: SideUsage,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SideUsage {
    pub input: u64,
    pub cached_input: u64,
    pub output: u64,
}

impl RunIo {
    pub async fn emit(&self, event: EngineEvent) {
        self.events.send(event).await.ok();
    }
}

pub trait Harness: Send + Sync {
    fn run(&self, turn: Turn, io: RunIo) -> BoxFuture<'static, Outcome>;

    fn set_title(
        &self,
        turn: Turn,
        title: String,
    ) -> BoxFuture<'static, Result<(), SetTitleError>> {
        let _ = (turn, title);
        Box::pin(async { Ok(()) })
    }

    fn side_query(
        &self,
        turn: Turn,
        cancel: CancellationToken,
    ) -> BoxFuture<'static, Result<SideReply, SideQueryError>> {
        let _ = cancel;
        let engine = turn.engine;
        Box::pin(async move { Err(SideQueryError::Unsupported { engine }) })
    }

    fn compact(
        &self,
        turn: Turn,
        cancel: CancellationToken,
    ) -> BoxFuture<'static, Result<SideReply, SideQueryError>> {
        self.side_query(turn, cancel)
    }

    fn clear_goal(&self, turn: Turn) -> BoxFuture<'static, Result<(), ClearGoalError>> {
        let engine = turn.engine;
        Box::pin(async move { Err(ClearGoalError::Unsupported { engine }) })
    }
}
