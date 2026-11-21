use crate::{EngineStatus, Item, ItemId, Project, Run, RunId, Seq, Status, Thread, ThreadId};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    pub seq: Option<Seq>,
    #[serde(skip)]
    pub live_revision: u64,
    #[serde(flatten)]
    pub event: Event,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
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
    ThreadRemoved {
        thread_id: ThreadId,
    },
    Item {
        item: Item,
    },
    Run {
        run: Run,
    },
    ItemState {
        id: ItemId,
        thread_id: ThreadId,
        status: Status,
    },
    Setting {
        key: String,
        value: serde_json::Value,
    },
    Live {
        thread_id: ThreadId,
        target: LiveTarget,
        text: String,
        append: bool,
        coverage: LiveCoverage,
    },
    Suggestion {
        thread_id: ThreadId,
        text: Option<String>,
    },
    Engine {
        status: EngineStatus,
    },
    EngineInstall {
        engine: crate::EngineId,
        install: crate::InstallState,
    },
    EditorCall {
        id: String,
        call: crate::EditorCall,
    },
    Reset,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveCoverage {
    pub start: u64,
    pub total: u64,
}

impl LiveCoverage {
    #[must_use]
    pub const fn is_complete(self, retained_bytes: usize) -> bool {
        self.start == 0 && self.total == retained_bytes as u64
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LiveTarget {
    Text { item_id: ItemId },
    Reasoning { run_id: RunId },
    Shell { item_id: ItemId },
    Progress { item_id: ItemId },
    Status { run_id: RunId },
}

impl Event {
    #[must_use]
    pub const fn thread_id(&self) -> Option<&ThreadId> {
        match self {
            Self::Thread { thread } => Some(&thread.id),
            Self::Item { item } => Some(&item.thread_id),
            Self::Run { run } => Some(&run.thread_id),
            Self::Live { thread_id, .. }
            | Self::ThreadRemoved { thread_id }
            | Self::ItemState { thread_id, .. }
            | Self::Suggestion { thread_id, .. } => Some(thread_id),
            Self::Hello { .. }
            | Self::Project { .. }
            | Self::Setting { .. }
            | Self::Engine { .. }
            | Self::EngineInstall { .. }
            | Self::EditorCall { .. }
            | Self::Reset => None,
        }
    }
}

#[cfg(test)]
#[path = "../tests/event/engine_delta.rs"]
mod engine_delta_tests;
