//! The subagents a thread's current run has started, for the thread
//! inspector's Agents section.
//!
//! A subagent reaches the editor as tool activity: the engine reports the
//! tool invocation that started it (canonical kind `subagent`, the task
//! description as its detail) and later settles that same invocation. This
//! projection reads those rows from the thread's
//! [`EngineObservationState`]; it holds no state of its own and asks the
//! backend for nothing.
//!
//! Only the run the thread is on is listed: its subagents while it works,
//! and the same list, settled, once it finishes. A new run starts the list
//! over, so the section never accumulates a whole thread's history.

#![forbid(unsafe_code)]

use crate::conversation_scene::{ActivityCategory, activity_category};
use crate::engine_observation_state::{EngineObservationState, ToolRow};
use artisan_domain::ToolAction;

/// Name shown for a subagent whose engine disclosed no task description.
pub const UNNAMED_AGENT_LABEL: &str = "Subagent";

/// Where one subagent stands.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ThreadAgentState {
    /// Started and not yet settled.
    Working,
    /// Finished its task.
    Completed,
    /// Stopped without finishing.
    Failed,
}

/// One subagent row of the inspector's Agents section.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadAgentEntry {
    /// Stable identity: the tool invocation that started the subagent.
    pub id: String,
    /// The task the subagent was given, or [`UNNAMED_AGENT_LABEL`].
    pub name: String,
    /// Where the subagent stands.
    pub state: ThreadAgentState,
}

impl ThreadAgentEntry {
    /// Returns whether this entry presents `row` exactly, without building
    /// the entry the row would project to.
    #[must_use]
    pub fn presents(&self, row: &ToolRow) -> bool {
        self.id == row.tool_id()
            && self.name == agent_name(row)
            && self.state == agent_state(row.action())
    }
}

fn agent_name(row: &ToolRow) -> &str {
    row.detail()
        .map(str::trim)
        .filter(|detail| !detail.is_empty())
        .unwrap_or(UNNAMED_AGENT_LABEL)
}

const fn agent_state(action: ToolAction) -> ThreadAgentState {
    match action {
        ToolAction::Started | ToolAction::Progress => ThreadAgentState::Working,
        ToolAction::Completed => ThreadAgentState::Completed,
        ToolAction::Failed => ThreadAgentState::Failed,
    }
}

/// Returns the subagent tool rows of the run the thread is on, in the order
/// they started.
///
/// Deliveries without Forge attribution name no run; a thread that has only
/// those lists every subagent it holds.
#[must_use]
pub fn thread_agent_rows(observations: &EngineObservationState) -> Vec<&ToolRow> {
    let run = observations.latest_run();
    observations
        .tools_in_order()
        .into_iter()
        .filter(|row| activity_category(row.tool_name()) == ActivityCategory::Subagent)
        .filter(|row| run.is_none_or(|run| row.attributed_run() == Some(run)))
        .collect()
}

/// Projects one subagent tool row to its inspector entry.
#[must_use]
pub fn thread_agent_entry(row: &ToolRow) -> ThreadAgentEntry {
    ThreadAgentEntry {
        id: row.tool_id().to_owned(),
        name: agent_name(row).to_owned(),
        state: agent_state(row.action()),
    }
}
