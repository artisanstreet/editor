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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thread {
    pub id: ThreadId,
    pub project_id: ProjectId,
    /// From the engine; until then the first line of the first message.
    pub title: Option<String>,
    pub config: EngineConfig,
    /// Forge-local order of accepted input and run boundaries; unread after `read_seq`.
    pub activity_seq: Seq,
    /// Event time of that activity, for approximate recency across independent Forges.
    /// Metadata edits and read acknowledgments leave this unchanged.
    #[serde(default)]
    pub activity_at: Timestamp,
    pub read_seq: Seq,
    #[serde(default)]
    pub marked_unread: bool,
    /// Activity acknowledged as settled by the client; monotonic.
    #[serde(default)]
    pub settled_seq: Seq,
    pub seq: Seq,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    #[serde(default)]
    pub archived: bool,
    /// The thread this one was handed off from, or the thread whose agent
    /// it is (with `spawned_by`).
    #[serde(default)]
    pub parent_thread_id: Option<ThreadId>,
    /// The `Agent` item in the parent thread this thread runs, for an agent
    /// started through the Artisan MCP.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawned_by: Option<crate::ItemId>,
    /// When Forge picks the thread's failed turn up again by itself (the
    /// provider was overloaded, or the usage limit resets); set while it
    /// waits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parked_until: Option<Timestamp>,
}

impl Thread {
    #[must_use]
    pub const fn is_unread(&self) -> bool {
        self.marked_unread || self.activity_seq > self.read_seq
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Run {
    /// The source launch configuration, so handoff forks retain the model's
    /// effort, permission mode and other cache-sensitive settings after a switch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<EngineConfig>,
    pub id: RunId,
    pub thread_id: ThreadId,
    pub engine: EngineId,
    pub model: Option<String>,
    /// Highest pending-input revision observed when this run started.
    pub input_cutoff: Seq,
    pub status: RunStatus,
    pub usage: Usage,
    /// The engine's session or thread id, used to resume.
    pub session_id: Option<String>,
    pub error: Option<String>,
    pub seq: Seq,
    pub started_at: Timestamp,
    pub ended_at: Option<Timestamp>,
    /// Set when the run carries on a turn that stopped before it finished,
    /// with no new message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuation: Option<Continuation>,
}

/// Why a run carries on a stopped turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Continuation {
    /// The user asked for it (the composer's continue button).
    Requested,
    /// Forge restarted while the turn was running.
    Restart,
    /// The provider was overloaded or briefly unavailable, and the turn
    /// failed after the engine's own retries.
    Overloaded,
    /// The account's usage limit ended the turn; it carries on once the
    /// limit resets.
    UsageLimit,
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

    /// The turn stopped before it finished, so it can be continued.
    #[must_use]
    pub const fn is_stopped(self) -> bool {
        matches!(self, Self::Cancelled | Self::Interrupted)
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
