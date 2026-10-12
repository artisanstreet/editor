use crate::{EngineConfig, EngineId, ProjectId, RunId, Seq, ThreadId, Timestamp};
use serde::{Deserialize, Serialize};

/// A working directory on the Forge host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub id: ProjectId,
    pub path: String,
    pub name: String,
    pub seq: Seq,
    pub created_at: Timestamp,
    #[serde(default)]
    pub removed: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Thread {
    pub id: ThreadId,
    pub project_id: ProjectId,
    /// From the engine; until then the first line of the first message.
    pub title: Option<String>,
    pub config: EngineConfig,
    /// Seq of the last item change; unread while greater than `read_seq`.
    pub activity_seq: Seq,
    pub read_seq: Seq,
    pub seq: Seq,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    #[serde(default)]
    pub archived: bool,
    /// The thread this one was handed off from.
    #[serde(default)]
    pub parent_thread_id: Option<ThreadId>,
}

impl Thread {
    #[must_use]
    pub fn is_unread(&self) -> bool {
        self.activity_seq > self.read_seq
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub id: RunId,
    pub thread_id: ThreadId,
    pub engine: EngineId,
    pub model: Option<String>,
    pub status: RunStatus,
    pub usage: Usage,
    /// The engine's session or thread id, used to resume.
    pub session_id: Option<String>,
    pub error: Option<String>,
    pub seq: Seq,
    pub started_at: Timestamp,
    pub ended_at: Option<Timestamp>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Done,
    Failed,
    Cancelled,
    /// Forge stopped while the run was live.
    Interrupted,
    Refused,
}

impl RunStatus {
    #[must_use]
    pub const fn is_live(self) -> bool {
        matches!(self, Self::Running)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub reasoning: u64,
    pub cache_read: u64,
    pub cache_write: Option<u64>,
    /// Tokens in the context window after the last step.
    pub context_used: Option<u64>,
    pub context_window: Option<u64>,
    pub cost_usd: Option<f64>,
}
