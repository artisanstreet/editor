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
#[serde(try_from = "RunWire", into = "RunWire")]
pub struct Run {
    pub config: Option<EngineConfig>,
    pub id: RunId,
    pub thread_id: ThreadId,
    pub engine: EngineId,
    pub model: Option<String>,
    pub input_cutoff: Seq,
    pub state: RunState,
    pub usage: Usage,
    pub session_id: Option<String>,
    pub seq: Seq,
    pub started_at: Timestamp,
    pub continuation: Option<Continuation>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunState {
    Running,
    Finished {
        outcome: RunOutcome,
        error: Option<String>,
        ended_at: Timestamp,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RunOutcome {
    Done,
    Failed,
    Cancelled,
    Interrupted,
    Refused,
}

impl RunOutcome {
    #[must_use]
    pub const fn status(self) -> RunStatus {
        match self {
            Self::Done => RunStatus::Done,
            Self::Failed => RunStatus::Failed,
            Self::Cancelled => RunStatus::Cancelled,
            Self::Interrupted => RunStatus::Interrupted,
            Self::Refused => RunStatus::Refused,
        }
    }

    #[must_use]
    pub const fn of(status: RunStatus) -> Option<Self> {
        match status {
            RunStatus::Running => None,
            RunStatus::Done => Some(Self::Done),
            RunStatus::Failed => Some(Self::Failed),
            RunStatus::Cancelled => Some(Self::Cancelled),
            RunStatus::Interrupted => Some(Self::Interrupted),
            RunStatus::Refused => Some(Self::Refused),
        }
    }
}

impl RunState {
    #[must_use]
    pub const fn status(&self) -> RunStatus {
        match self {
            Self::Running => RunStatus::Running,
            Self::Finished { outcome, .. } => outcome.status(),
        }
    }

    #[must_use]
    pub fn error(&self) -> Option<&str> {
        match self {
            Self::Running => None,
            Self::Finished { error, .. } => error.as_deref(),
        }
    }

    #[must_use]
    pub const fn ended_at(&self) -> Option<Timestamp> {
        match self {
            Self::Running => None,
            Self::Finished { ended_at, .. } => Some(*ended_at),
        }
    }
}

impl Run {
    #[must_use]
    pub const fn status(&self) -> RunStatus {
        self.state.status()
    }

    #[must_use]
    pub fn error(&self) -> Option<&str> {
        self.state.error()
    }

    #[must_use]
    pub const fn ended_at(&self) -> Option<Timestamp> {
        self.state.ended_at()
    }

    #[must_use]
    pub const fn is_live(&self) -> bool {
        matches!(self.state, RunState::Running)
    }

    pub fn finish(&mut self, outcome: RunOutcome, error: Option<String>, at: Timestamp) {
        self.state = RunState::Finished {
            outcome,
            error,
            ended_at: self.ended_at().unwrap_or(at),
        };
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RunWireError {
    #[error("run {run} is running but carries an ending time or an error")]
    RunningWithEnding { run: RunId },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct RunWire {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    config: Option<EngineConfig>,
    id: RunId,
    thread_id: ThreadId,
    engine: EngineId,
    model: Option<String>,
    input_cutoff: Seq,
    status: RunStatus,
    usage: Usage,
    session_id: Option<String>,
    error: Option<String>,
    seq: Seq,
    started_at: Timestamp,
    ended_at: Option<Timestamp>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    continuation: Option<Continuation>,
}

impl TryFrom<RunWire> for Run {
    type Error = RunWireError;

    fn try_from(wire: RunWire) -> Result<Self, Self::Error> {
        let state = match RunOutcome::of(wire.status) {
            None if wire.ended_at.is_some() || wire.error.is_some() => {
                return Err(RunWireError::RunningWithEnding { run: wire.id });
            }
            None => RunState::Running,
            Some(outcome) => RunState::Finished {
                outcome,
                error: wire.error,
                ended_at: wire.ended_at.unwrap_or(wire.started_at),
            },
        };
        Ok(Self {
            config: wire.config,
            id: wire.id,
            thread_id: wire.thread_id,
            engine: wire.engine,
            model: wire.model,
            input_cutoff: wire.input_cutoff,
            state,
            usage: wire.usage,
            session_id: wire.session_id,
            seq: wire.seq,
            started_at: wire.started_at,
            continuation: wire.continuation,
        })
    }
}

impl From<Run> for RunWire {
    fn from(run: Run) -> Self {
        let (status, error, ended_at) = match run.state {
            RunState::Running => (RunStatus::Running, None, None),
            RunState::Finished {
                outcome,
                error,
                ended_at,
            } => (outcome.status(), error, Some(ended_at)),
        };
        Self {
            config: run.config,
            id: run.id,
            thread_id: run.thread_id,
            engine: run.engine,
            model: run.model,
            input_cutoff: run.input_cutoff,
            status,
            usage: run.usage,
            session_id: run.session_id,
            error,
            seq: run.seq,
            started_at: run.started_at,
            ended_at,
            continuation: run.continuation,
        }
    }
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
