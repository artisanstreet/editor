use crate::{EngineStatus, Item, ItemId, Project, Run, RunId, Seq, Thread, ThreadId};
use serde::{Deserialize, Serialize};

/// One message on `GET /events?since=N`.
///
/// Stored changes carry `seq`; a client that saw it can resume after it.
/// Live events (`seq: None`) are memory-only: streaming text, reasoning,
/// shell output. A reconnecting client gets the current live text again.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    pub seq: Option<Seq>,
    #[serde(flatten)]
    pub event: Event,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// First message on every stream.
    Hello {
        head: Seq,
        protocol: u32,
        forge_version: String,
    },
    Project {
        project: Project,
    },
    Thread {
        thread: Thread,
    },
    Item {
        item: Item,
    },
    Run {
        run: Run,
    },
    Setting {
        key: String,
        value: serde_json::Value,
    },
    /// Live: streaming text for an item, reasoning for a run, shell output.
    Live {
        thread_id: ThreadId,
        target: LiveTarget,
        text: String,
        append: bool,
    },
    /// Live: an engine's install, sign-in, models or usage changed.
    Engine {
        status: EngineStatus,
    },
    /// The client fell too far behind; refetch `GET /state`.
    Reset,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LiveTarget {
    /// An `AssistantMessage` still streaming.
    Text { item_id: ItemId },
    /// The run's current reasoning; dropped when the run ends.
    Reasoning { run_id: RunId },
    /// A running shell tool's output; dropped when the tool ends.
    Shell { item_id: ItemId },
    /// A tool's progress line.
    Progress { item_id: ItemId },
    /// A transient run status line ("Retrying (2)…").
    Status { run_id: RunId },
}

impl Event {
    #[must_use]
    pub fn thread_id(&self) -> Option<&ThreadId> {
        match self {
            Self::Thread { thread } => Some(&thread.id),
            Self::Item { item } => Some(&item.thread_id),
            Self::Run { run } => Some(&run.thread_id),
            Self::Live { thread_id, .. } => Some(thread_id),
            _ => None,
        }
    }
}
