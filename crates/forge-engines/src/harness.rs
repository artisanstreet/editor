//! The contract between `forge-runs` and an engine adapter.
//!
//! An adapter turns one [`Turn`] into a stream of [`EngineEvent`]s and
//! returns how the run ended. Everything engine-specific (process wiring,
//! JSON shapes, handshakes) stays behind this boundary.

use artisan_api::{
    Answer, Approval, EngineConfig, EngineId, ItemId, ItemKind, RunStatus, Status, ThreadId, Usage,
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
    /// Explicit diagnostic trace destination. Normal runs do not record raw content.
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
    /// MCP servers for engines that take them in-protocol (ACP `session/new`).
    pub mcp_servers: Vec<crate::McpServer>,
}

/// What `forge-runs` sends a live run.
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    /// Deliver in place if the engine can; otherwise start the next turn with it.
    Steer(UserInput),
    /// Answer the approval or question on the item with this key.
    Answer { key: String, answer: Answer },
}

/// What an adapter reports.
// Nearly every event is an `Item`, so boxing it would add an allocation per
// event without making the channel any smaller.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq)]
pub enum EngineEvent {
    /// The engine's session id, for resume.
    Session {
        id: String,
    },
    Model {
        id: String,
    },
    Title {
        title: String,
    },
    /// Create or replace an item. `key` is the engine's own id, or one the
    /// adapter makes up that stays stable for the item's life.
    Item {
        key: String,
        parent: Option<String>,
        status: Status,
        kind: ItemKind,
    },
    /// A request has its own lifecycle; target names the execution it gates.
    Approval {
        key: String,
        parent: Option<String>,
        target: Option<String>,
        status: Status,
        approval: Approval,
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
    /// An initial input or steer reached the engine.
    InputAccepted {
        id: ItemId,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LiveKind {
    /// Streaming text of the item with this key (an `AssistantMessage`).
    Text {
        key: String,
    },
    Reasoning,
    Shell {
        key: String,
    },
    Progress {
        key: String,
    },
    Status,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub status: RunStatus,
    pub error: Option<String>,
}

impl Outcome {
    #[must_use]
    pub fn done() -> Self {
        Self {
            status: RunStatus::Done,
            error: None,
        }
    }

    #[must_use]
    pub fn cancelled() -> Self {
        Self {
            status: RunStatus::Cancelled,
            error: None,
        }
    }

    #[must_use]
    pub fn failed(message: impl Into<String>) -> Self {
        Self {
            status: RunStatus::Failed,
            error: Some(message.into()),
        }
    }
}

/// The channels of one live run.
pub struct RunIo {
    pub inbox: mpsc::Receiver<Input>,
    pub events: mpsc::Sender<EngineEvent>,
    pub cancel: CancellationToken,
}

impl RunIo {
    /// Sends an event. A closed receiver means the run is being torn down;
    /// the adapter notices through `cancel`.
    pub async fn emit(&self, event: EngineEvent) {
        let _ = self.events.send(event).await;
    }
}

/// Runs turns for one engine.
pub trait Harness: Send + Sync {
    fn run(&self, turn: Turn, io: RunIo) -> BoxFuture<'static, Outcome>;
}
