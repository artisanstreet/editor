//! The transcript as rows: a pure fold of the loaded items, the live text
//! and the UI toggles. Each row is one entry of the scrolling list and one
//! cached row view; its version changes exactly when what it shows does.

use crate::activity;
use crate::phase::{self, Phase};
use crate::thread::ThreadView;
use artisan_api::{
    EngineId, Item, ItemId, ItemKind, LiveTarget, NoticeKind, Phase as TextPhase, Run, RunId,
    RunStatus, Seq, Status, Timestamp,
};
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher as _};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Row {
    User(ItemId),
    /// One stretch of a run's work. A steer splits a run's work into
    /// segments; only the first carries the header.
    Work {
        run: Option<RunId>,
        segment: usize,
        entries: Vec<ItemId>,
    },
    Reply(ItemId),
    /// An approval or a question.
    Prompt(ItemId),
    Compaction(ItemId),
    Error(ItemId),
    Image(ItemId),
    /// Newer items exist below; reaching here loads them.
    Gap,
    /// The live run's status line, reasoning and stop button.
    Status(RunId),
    /// A subagent's view: the task it was given, at the top…
    AgentTask(ItemId),
    /// …and what it reported back, at the end.
    AgentResult(ItemId),
    /// A message the agent got while it worked (`SendMessage`), where it
    /// arrived.
    AgentMessage(ItemId),
    /// A live agent's status line: working (a tool runs) or thinking.
    AgentStatus(ItemId, bool),
}

/// A row's identity across folds: a work section keeps its key as items
/// join it, so its view (and its measured height) survive.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum RowKey {
    Item(ItemId),
    Work(RunId, usize),
    /// Work without a run, keyed by its first item.
    Loose(ItemId),
    Gap,
    Status(RunId),
    AgentTask(ItemId),
    AgentResult(ItemId),
    AgentStatus(ItemId),
}

impl Row {
    /// The item that anchors this row in the thread.
    pub(crate) fn anchor(&self) -> Option<&ItemId> {
        match self {
            Self::User(id)
            | Self::Reply(id)
            | Self::Prompt(id)
            | Self::Compaction(id)
            | Self::Error(id)
            | Self::Image(id)
            | Self::AgentTask(id)
            | Self::AgentResult(id)
            | Self::AgentMessage(id) => Some(id),
            Self::Work { entries, .. } => entries.first(),
            Self::Gap | Self::Status(_) | Self::AgentStatus(..) => None,
        }
    }

    pub(crate) fn key(&self) -> RowKey {
        match self {
            Self::Work {
                run: Some(run),
                segment,
                ..
            } => RowKey::Work(run.clone(), *segment),
            Self::Work {
                run: None, entries, ..
            } => RowKey::Loose(entries.first().cloned().unwrap_or_default()),
            Self::Gap => RowKey::Gap,
            Self::Status(run) => RowKey::Status(run.clone()),
            Self::AgentTask(id) => RowKey::AgentTask(id.clone()),
            Self::AgentResult(id) => RowKey::AgentResult(id.clone()),
            Self::AgentStatus(id, _) => RowKey::AgentStatus(id.clone()),
            other => RowKey::Item(other.anchor().cloned().unwrap_or_default()),
        }
    }

    /// The run this row reports on (work sections and the status line).
    pub(crate) fn run(&self) -> Option<&RunId> {
        match self {
            Self::Work { run, .. } => run.as_ref(),
            Self::Status(run) => Some(run),
            _ => None,
        }
    }
}

/// What a run's work section and status line show about the run.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RunSummary {
    pub phase: Phase,
    pub started_at: Timestamp,
    /// `None` while the run is live: the elapsed time counts to now.
    pub ended_at: Option<Timestamp>,
    /// The section is open (derived default, or the user's toggle).
    pub open: bool,
    /// The engine, known for the thread's latest run: it picks how the
    /// reasoning summary reduces to the status line.
    pub engine: Option<EngineId>,
    /// The run used a tool or an agent (its header says "Working", not
    /// "Thinking").
    pub worked: bool,
}

impl RunSummary {
    pub(crate) fn live(&self) -> bool {
        !matches!(self.phase, Phase::Settled(_))
    }
}

pub(crate) struct Folded {
    pub rows: Vec<(Row, u64)>,
    pub runs: HashMap<RunId, RunSummary>,
}

/// `scope` is the subagent whose view is open: only its items show, its
/// own as the top level, under the task it got and above its report.
#[expect(
    clippy::too_many_lines,
    reason = "one pass over the items, in transcript order, deciding each one's row"
)]
pub(crate) fn fold(view: &ThreadView, latest: Option<&Run>, scope: Option<&ItemId>) -> Folded {
    // The id messages to the open agent address it by.
    let scope_address =
        scope
            .and_then(|agent| view.item(agent))
            .and_then(|agent| match &agent.kind {
                ItemKind::Agent { agent_id, .. } => agent_id.clone(),
                _ => None,
            });
    let to_scope = |item: &Item| {
        scope_address.as_ref().is_some_and(|address| {
            activity::agent_message(item).is_some_and(|message| message.to == *address)
        })
    };
    let shown: Vec<&Item> = view
        .shown()
        .filter(|item| scope.is_none_or(|agent| descends_from(view, item, agent) || to_scope(item)))
        .collect();
    let loaded: HashSet<&ItemId> = shown.iter().map(|item| &item.id).collect();
    let children = children(&shown);
    let runs = summaries(view, &shown, latest);
    let mut rows: Vec<Row> = Vec::new();
    let mut work: Option<(Option<RunId>, Vec<ItemId>)> = None;
    let mut segments: HashMap<Option<RunId>, usize> = HashMap::new();
    // Where each run's last top-level item sits: text the engine called
    // final but that more of the run followed (a turn woken again by a
    // background notification, say) was commentary after all.
    let mut run_ends: HashMap<&RunId, Seq> = HashMap::new();
    for item in &shown {
        if let (Some(run), None) = (&item.run_id, &item.parent_id) {
            let end = run_ends.entry(run).or_default();
            *end = (*end).max(item.position);
        }
    }
    // Where each run's own turn begins: the message that started it.
    let mut run_starts: HashMap<&RunId, Seq> = HashMap::new();
    for item in &shown {
        if let (Some(run), ItemKind::UserMessage { steer: false, .. }) = (&item.run_id, &item.kind)
        {
            run_starts.entry(run).or_insert(item.position);
        }
    }
    let mut flush = |work: &mut Option<(Option<RunId>, Vec<ItemId>)>, rows: &mut Vec<Row>| {
        if let Some((run, entries)) = work.take() {
            let next = segments.entry(run.clone()).or_default();
            rows.push(Row::Work {
                run,
                segment: *next,
                entries,
            });
            *next += 1;
        }
    };
    // The run of the turn the walk is in.
    let mut context: Option<RunId> = None;
    for item in &shown {
        // Subagent output shows in its agent's view (and, there, under
        // agents below it when loaded), never in the thread itself, even
        // when its agent row is outside the loaded window.
        if item.parent_id.as_ref().is_some_and(|parent| {
            scope.is_none() || (loaded.contains(parent) && Some(parent) != scope)
        }) {
            continue;
        }
        // A goal notice without a goal (Codex cleared one it never set).
        if matches!(&item.kind, ItemKind::Notice { kind: NoticeKind::Goal { .. }, text } if text.trim().is_empty())
        {
            continue;
        }
        if scope.is_some() && to_scope(item) {
            flush(&mut work, &mut rows);
            rows.push(Row::AgentMessage(item.id.clone()));
            continue;
        }
        let own_row = match &item.kind {
            // A steer belongs to the running turn: it sits in the work
            // where it was sent and folds with it.
            ItemKind::UserMessage { steer: true, .. } if item.run_id.is_some() => None,
            ItemKind::UserMessage { .. } => Some(Row::User(item.id.clone())),
            ItemKind::AssistantMessage {
                phase: TextPhase::Final,
                ..
            } if scope.is_none()
                && item
                    .run_id
                    .as_ref()
                    .and_then(|run| run_ends.get(run))
                    .is_none_or(|end| *end <= item.position) =>
            {
                Some(Row::Reply(item.id.clone()))
            }
            ItemKind::Question { .. } => Some(Row::Prompt(item.id.clone())),
            ItemKind::Tool { .. } | ItemKind::Plan { .. }
                if item.status == Status::AwaitingUser =>
            {
                Some(Row::Prompt(item.id.clone()))
            }
            ItemKind::Compaction { .. } => Some(Row::Compaction(item.id.clone())),
            ItemKind::Error { .. } => Some(Row::Error(item.id.clone())),
            ItemKind::Image { .. } => Some(Row::Image(item.id.clone())),
            _ => None,
        };
        if let Some(row) = own_row {
            flush(&mut work, &mut rows);
            rows.push(row);
            context.clone_from(&item.run_id);
            continue;
        }
        // A subagent's work is all one stretch, without its parent run's
        // header.
        let run = if scope.is_some() {
            None
        } else if item
            .run_id
            .as_ref()
            .and_then(|run| run_starts.get(run))
            .is_some_and(|start| item.position < *start)
        {
            // Stored before its run's own message (an agent a later run
            // resumed was moved into it): it belongs to the turn it sits in.
            context.clone()
        } else {
            item.run_id.clone()
        };
        context.clone_from(&run);
        match &mut work {
            Some((current, entries)) if *current == run => entries.push(item.id.clone()),
            _ => {
                flush(&mut work, &mut rows);
                work = Some((run, vec![item.id.clone()]));
            }
        }
    }
    flush(&mut work, &mut rows);
    if let Some(agent) = scope {
        rows.insert(0, Row::AgentTask(agent.clone()));
        if view
            .item(agent)
            .is_some_and(|item| item.status == Status::Running)
        {
            let working = shown.iter().any(|item| {
                item.status == Status::Running
                    && matches!(item.kind, ItemKind::Tool { .. } | ItemKind::Agent { .. })
                    && Some(&item.id) != scope
            });
            rows.push(Row::AgentStatus(agent.clone(), working));
        }
        if view.item(agent).and_then(agent_report).is_some() {
            rows.push(Row::AgentResult(agent.clone()));
        }
    } else if view.window().gap.is_some() {
        rows.push(Row::Gap);
    } else if let Some(run) = latest.filter(|run| run.status.is_live()) {
        rows.push(Row::Status(run.id.clone()));
    }
    let rows = rows
        .into_iter()
        .map(|row| {
            let version = version(view, &row, &children, &runs);
            (row, version)
        })
        .collect();
    Folded { rows, runs }
}

/// What a subagent reported back. A result that only repeats its
/// description is no report (stopped agents stored before Forge stopped
/// taking Claude's summary for one).
pub(crate) fn agent_report(item: &Item) -> Option<&str> {
    match &item.kind {
        ItemKind::Agent {
            result: Some(result),
            description,
            ..
        } if result.trim() != description.trim() => Some(result),
        _ => None,
    }
}

/// Whether `item` is part of the subagent `agent`'s work, at any depth.
fn descends_from(view: &ThreadView, item: &Item, agent: &ItemId) -> bool {
    let mut parent = item.parent_id.as_ref();
    for _ in 0..16 {
        match parent {
            Some(id) if id == agent => return true,
            Some(id) => parent = view.item(id).and_then(|item| item.parent_id.as_ref()),
            None => return false,
        }
    }
    false
}

/// Children of each loaded Agent item, in order.
pub(crate) fn children<'a>(items: &[&'a Item]) -> HashMap<ItemId, Vec<&'a Item>> {
    let mut children: HashMap<ItemId, Vec<&Item>> = HashMap::new();
    for item in items {
        if let Some(parent) = &item.parent_id {
            children.entry(parent.clone()).or_default().push(item);
        }
    }
    children
}

/// Phase, timing and fold state of every run in the window. Only each
/// thread's latest `Run` is known; earlier runs are judged by their items.
fn summaries(
    view: &ThreadView,
    shown: &[&Item],
    latest: Option<&Run>,
) -> HashMap<RunId, RunSummary> {
    let mut by_run: HashMap<&RunId, Vec<&Item>> = HashMap::new();
    for item in shown {
        if let Some(run) = &item.run_id {
            by_run.entry(run).or_default().push(item);
        }
    }
    if let Some(run) = latest {
        by_run.entry(&run.id).or_default();
    }
    by_run
        .into_iter()
        .map(|(run_id, items)| {
            let latest = latest.filter(|run| &run.id == run_id);
            let (phase, started_at, ended_at) = if let Some(run) = latest {
                (
                    phase::turn_phase(run, &items, &view.live),
                    run.started_at,
                    run.ended_at,
                )
            } else {
                let start = items.iter().map(|item| item.created_at).min().unwrap_or(0);
                let end = items
                    .iter()
                    .map(|item| item.updated_at)
                    .max()
                    .unwrap_or(start);
                (
                    Phase::Settled(phase::settled_from_items(&items)),
                    start,
                    Some(end),
                )
            };
            let live = !matches!(phase, Phase::Settled(_));
            // A live section stays open: text streams as commentary inside
            // it (it turns final, and leaves for its own row, only when the
            // run ends), so folding on `Replying` hid the streaming text. A
            // settled one keeps the user's choice, closed by default once
            // the run succeeded.
            let open = if live {
                true
            } else {
                view.work_open
                    .get(run_id)
                    .copied()
                    .unwrap_or(phase != Phase::Settled(RunStatus::Done))
            };
            (
                run_id.clone(),
                RunSummary {
                    phase,
                    started_at,
                    ended_at: if live { None } else { ended_at },
                    open,
                    engine: latest.map(|run| run.engine),
                    worked: items.iter().any(|item| {
                        matches!(
                            item.kind,
                            ItemKind::Tool { .. } | ItemKind::Agent { .. } | ItemKind::Plan { .. }
                        )
                    }),
                },
            )
        })
        .collect()
}

/// Top of the transcript (`TRANSCRIPT_PAD_TOP_PX`).
const PAD_TOP: f32 = 40.0;
/// Between turns (`TRANSCRIPT_TURN_GAP_PX`).
const TURN_GAP: f32 = 32.0;
/// Between blocks of one turn (`steps(6)`).
const BLOCK_GAP: f32 = 24.0;
/// The reply after its work: the 8 px seam (24 px less the reply's 16 px).
const REPLY_SEAM: f32 = 8.0;
/// The live line after the work: 24 px less its own 12 px.
const STATUS_AFTER_WORK: f32 = 12.0;
/// The live line elsewhere: 24 px plus its own 8 px.
const STATUS_GAP: f32 = 32.0;

/// The space above each row: the old transcript's rhythm of turns (each a
/// user message and what the run did about it) and the blocks inside them.
pub(crate) fn gaps(view: &ThreadView, rows: &[(Row, u64)]) -> Vec<f32> {
    let run_of = |row: &Row| -> Option<RunId> {
        match row {
            Row::Work { run, .. } => run.clone(),
            Row::Status(run) => Some(run.clone()),
            other => other
                .anchor()
                .and_then(|id| view.item(id))
                .and_then(|item| item.run_id.clone()),
        }
    };
    rows.iter()
        .enumerate()
        .map(|(index, (row, _))| {
            let Some((previous, _)) = index.checked_sub(1).and_then(|index| rows.get(index)) else {
                return PAD_TOP;
            };
            let (run, previous_run) = (run_of(row), run_of(previous));
            match (previous, row) {
                (Row::Work { .. }, Row::Reply(_)) => REPLY_SEAM,
                (Row::Work { .. }, Row::Status(_)) => STATUS_AFTER_WORK,
                (_, Row::Status(_)) => STATUS_GAP,
                // A steer stays inside its run's turn.
                (_, Row::User(_)) if run.is_some() && run == previous_run => BLOCK_GAP,
                (_, Row::User(_)) => TURN_GAP,
                (Row::User(_), _) => BLOCK_GAP,
                _ if run.is_some() && previous_run.is_some() && run != previous_run => TURN_GAP,
                _ => BLOCK_GAP,
            }
        })
        .collect()
}

fn version(
    view: &ThreadView,
    row: &Row,
    children: &HashMap<ItemId, Vec<&Item>>,
    runs: &HashMap<RunId, RunSummary>,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    row.hash(&mut hasher);
    row.run().and_then(|run| runs.get(run)).hash(&mut hasher);
    let mut stack: Vec<&ItemId> = match row {
        Row::Work { entries, .. } => entries.iter().collect(),
        Row::Status(run) => {
            view.live
                .get(&LiveTarget::Reasoning {
                    run_id: run.clone(),
                })
                .hash(&mut hasher);
            view.live
                .get(&LiveTarget::Status {
                    run_id: run.clone(),
                })
                .hash(&mut hasher);
            Vec::new()
        }
        Row::AgentTask(id) | Row::AgentResult(id) | Row::AgentStatus(id, _) => {
            view.item(id).map(|item| item.seq).hash(&mut hasher);
            Vec::new()
        }
        _ => row.anchor().into_iter().collect(),
    };
    while let Some(id) = stack.pop() {
        if let Some(item) = view.item(id) {
            item.seq.hash(&mut hasher);
        }
        view.ui.get(id).hash(&mut hasher);
        for target in [
            LiveTarget::Text {
                item_id: id.clone(),
            },
            LiveTarget::Shell {
                item_id: id.clone(),
            },
            LiveTarget::Progress {
                item_id: id.clone(),
            },
        ] {
            view.live.get(&target).hash(&mut hasher);
        }
        if let Some(nested) = children.get(id) {
            stack.extend(nested.iter().map(|item| &item.id));
        }
    }
    hasher.finish()
}
