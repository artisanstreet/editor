//! The contract between `forge-runs` and an engine adapter.
//!
//! An adapter turns one [`Turn`] into a stream of [`EngineEvent`]s and
//! returns how the run ended. Everything engine-specific (process wiring,
//! JSON shapes, handshakes) stays behind this boundary.

use artisan_api::{
    Answer, EngineConfig, EngineId, ItemId, ItemKind, McpServer, RunOutcome, RunStatus, Status,
    ThreadId, Timestamp, Usage,
};
use futures::future::BoxFuture;
use std::path::PathBuf;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Everything an adapter needs to run one turn.
#[derive(Clone, Debug)]
pub struct Turn {
    pub thread_id: ThreadId,
    pub engine: EngineId,
    /// The engine executable.
    pub binary: PathBuf,
    /// `HOME` for the engine process (Forge-managed engine home).
    pub home: PathBuf,
    pub cwd: PathBuf,
    pub config: EngineConfig,
    /// Resume this engine session when set.
    pub session_id: Option<String>,
    pub input: UserInput,
    /// Extra arguments and environment from customisation (MCP, settings).
    pub launch: Launch,
    pub trace_dir: Option<PathBuf>,
}

/// A user message as the engine receives it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UserInput {
    pub id: ItemId,
    pub text: String,
    /// WebP files on disk.
    pub images: Vec<PathBuf>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Launch {
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub gateway_mcp: Vec<(String, String)>,
    /// Every MCP server this launch carries, user-configured and Forge's
    /// own. Adapters that run without a process of their own (the claude.ai
    /// transport) connect to these directly.
    pub mcp_servers: Vec<McpServer>,
    pub readable: Vec<PathBuf>,
}

/// What `forge-runs` sends a live run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Input {
    /// Deliver in place if the engine can; otherwise start the next turn with it.
    Steer(UserInput),
    /// Answer the approval or question on the item with this key.
    Answer { key: String, answer: Answer },
    /// Clear the thread's goal; the engine stops working toward it.
    ClearGoal,
    /// The user changed the thread's access mid-run: what the engine asks
    /// from now on. Forge itself approves what waits under Full.
    SetPermissions(artisan_api::Permissions),
}

/// What an adapter reports.
// Nearly every event is an `Item`, so boxing it would add an allocation per
// event without making the channel any smaller.
#[expect(
    clippy::large_enum_variant,
    reason = "nearly every event is an `Item`; see above"
)]
#[derive(Clone, Debug, PartialEq)]
pub enum EngineEvent {
    /// The engine's session id, for resume.
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
    /// Create or replace an item. `key` is the engine's own id, or one the
    /// adapter makes up that stays stable for the item's life. An approval
    /// rides on the item it gates (`Tool`/`Plan` with `approval` set and
    /// status `AwaitingUser`), keyed like that item.
    Item {
        key: String,
        parent: Option<String>,
        status: Status,
        kind: ItemKind,
    },
    /// An adapter produced an image file; Forge stores it as a canonical blob.
    Image {
        key: String,
        parent: Option<String>,
        path: PathBuf,
        caption: Option<String>,
    },
    /// Change only an item's status.
    Status {
        key: String,
        status: Status,
    },
    /// Memory-only text.
    Live {
        target: LiveKind,
        text: String,
        append: bool,
    },
    Usage(Usage),
    /// The `/` commands the engine reads now, as it reported them.
    Commands(Vec<artisan_api::SlashCommand>),
    /// An initial input or steer reached the engine.
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
    /// Streaming text of the item with this key (an `AssistantMessage`).
    Text {
        key: String,
    },
    /// The model's own reasoning, as prose. It is trimmed into a line here.
    Reasoning,
    /// The provider's own recap of what the model is doing, written for the
    /// reader. It is already a line, so it is published whole rather than
    /// trimmed into one.
    Narration,
    Shell {
        key: String,
    },
    Progress {
        key: String,
    },
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

/// The channels of one live run.
pub struct RunIo {
    pub inbox: mpsc::Receiver<Input>,
    pub events: mpsc::Sender<EngineEvent>,
    pub cancel: CancellationToken,
}

/// What a side query answered: the reply's text and what it cost.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SideReply {
    pub text: String,
    pub usage: SideUsage,
}

/// Tokens one side query used. `cached_input` against `input` shows whether
/// the fork reused the thread's prompt cache.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SideUsage {
    pub input: u64,
    pub cached_input: u64,
    pub output: u64,
}

impl RunIo {
    /// Sends an event. A closed receiver means the run is being torn down;
    /// the adapter notices through `cancel`.
    pub async fn emit(&self, event: EngineEvent) {
        self.events.send(event).await.ok();
    }
}

/// Runs turns for one engine.
pub trait Harness: Send + Sync {
    fn run(&self, turn: Turn, io: RunIo) -> BoxFuture<'static, Outcome>;

    /// Persists Forge's generated title in the native session,
    /// without prompting it or changing its continuation.
    fn set_title(
        &self,
        turn: Turn,
        title: String,
    ) -> BoxFuture<'static, Result<(), SetTitleError>> {
        let _ = (turn, title);
        Box::pin(async { Ok(()) })
    }

    /// Forks `turn.session_id` and asks it `turn.input.text` once, with no
    /// tool allowed to change anything. The thread's own session is left as
    /// it was, and the fork is discarded afterwards. The fork keeps the
    /// session's model and prefix so it reads the thread's prompt cache.
    fn side_query(
        &self,
        turn: Turn,
        cancel: CancellationToken,
    ) -> BoxFuture<'static, Result<SideReply, SideQueryError>> {
        let _ = cancel;
        let engine = turn.engine;
        Box::pin(async move { Err(SideQueryError::Unsupported { engine }) })
    }

    /// Summarizes source history without advancing the caller's continuation.
    /// Engines use disposable forks or a sibling branch in the native transport;
    /// others use the OpenCode prompt in a side query.
    fn compact(
        &self,
        turn: Turn,
        cancel: CancellationToken,
    ) -> BoxFuture<'static, Result<SideReply, SideQueryError>> {
        self.side_query(turn, cancel)
    }

    /// Clears `turn.session_id`'s goal while no run has the session.
    fn clear_goal(&self, turn: Turn) -> BoxFuture<'static, Result<(), ClearGoalError>> {
        let engine = turn.engine;
        Box::pin(async move { Err(ClearGoalError::Unsupported { engine }) })
    }
}
