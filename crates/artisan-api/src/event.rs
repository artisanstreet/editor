use crate::{EngineStatus, Item, ItemId, Project, Run, RunId, Seq, Status, Thread, ThreadId};
use serde::{Deserialize, Serialize};

/// One message on `GET /events?since=N`.
///
/// Stored changes carry `seq`; a client that saw it can resume after it.
/// Live events (`seq: None`) are memory-only: streaming text, reasoning,
/// shell output. A reconnecting client gets the current live text again.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    pub seq: Option<Seq>,
    /// Internal publication order for snapshot overlap; never part of the wire format.
    #[serde(skip)]
    pub live_revision: u64,
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
    /// Compact pending-interaction update for an unobserved transcript.
    ItemState {
        id: ItemId,
        thread_id: ThreadId,
        status: Status,
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
        coverage: LiveCoverage,
    },
    /// Live: an engine's install, sign-in, models or usage changed.
    Engine {
        status: EngineStatus,
    },
    /// Live: only download progress changed; catalog/account metadata stays shared.
    EngineInstall {
        engine: crate::EngineId,
        install: crate::InstallState,
    },
    /// The client fell too far behind; refetch `GET /state`.
    Reset,
}

/// Byte coverage of a bounded live preview in its complete source.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveCoverage {
    pub start: u64,
    pub total: u64,
}

impl LiveCoverage {
    #[must_use]
    pub fn is_complete(self, retained_bytes: usize) -> bool {
        self.start == 0 && self.total == retained_bytes as u64
    }
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
            Self::Live { thread_id, .. } | Self::ItemState { thread_id, .. } => Some(thread_id),
            _ => None,
        }
    }
}

#[cfg(test)]
mod engine_delta_tests {
    use super::*;
    #[test]
    fn install_delta_wire_does_not_repeat_catalog_or_account_fields() {
        let event = Event::EngineInstall {
            engine: crate::EngineId::Codex,
            install: crate::InstallState::Downloading {
                version: "1.2.3".into(),
                received: 1024,
                total: Some(2048),
            },
        };
        let wire = serde_json::to_value(&event).unwrap();
        assert_eq!(wire["type"], "engine_install");
        assert_eq!(wire.as_object().unwrap().len(), 3);
        assert!(wire.get("models").is_none() && wire.get("status").is_none());
        let replay: Event = serde_json::from_value(wire).unwrap();
        assert_eq!(event, replay);
    }
}
