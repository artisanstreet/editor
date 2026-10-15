//! The transcript as rows: a pure fold of the loaded items, the live text
//! and the UI toggles. Each row is one entry of the scrolling list and one
//! cached row view; its version changes exactly when what it shows does.

use crate::activity;
use crate::phase::{self, Phase};
use crate::thread::ThreadView;
use artisan_api::{
    Item, ItemId, ItemKind, LiveTarget, NoticeKind, Phase as TextPhase, Run, RunId, RunStatus, Seq,
    Status, Timestamp,
};
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher as _};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Row {
    /// An original native visual ordinal within one semantic source leaf.
    /// Only the window around the reader is registered with the native list.
    Projected {
        row: Box<Row>,
        visual: usize,
    },
    /// One contiguous stretch of work. Explicit transcript rows split stretches;
    /// steers stay inside their run's work. Only the first stretch carries its header.
    Work {
        run: Option<RunId>,
        segment: usize,
        entries: Vec<ItemId>,
    },
    /// One independently mounted entry inside an expanded work section.
    WorkEntry {
        id: ItemId,
        run: Option<RunId>,
    },
    /// A consecutive activity group's disclosure, never its descendants.
    Chain {
        ids: Vec<ItemId>,
        run: Option<RunId>,
    },
    PlanStep {
        id: ItemId,
        step: usize,
        run: Option<RunId>,
    },
    /// One block of an item's text body (see [`Slot`]): a reply's
    /// prose, a literal body or a formatted section. The fold emits block 0
    /// for the whole body; publishing splits it into the blocks prepared or
    /// loaded so far, and until then that one row shows all of it.
    Text {
        id: ItemId,
        slot: Slot,
        block: usize,
        /// The work the row sits in, for formatted sections.
        run: Option<RunId>,
    },
    /// An approval or a question.
    Prompt(ItemId),
    Compaction(ItemId),
    Image(ItemId),
    /// Newer items exist below; reaching here loads them.
    Gap,
    /// The live run's status line, reasoning and stop button.
    Status(RunId),
    /// A subagent's view: what it reported back, at the end (its task is
    /// a text row at the top, a message it got one where it arrived).
    AgentResult(ItemId),
    /// A live agent's status line: working (a tool runs) or thinking.
    AgentStatus(ItemId, bool),
}

/// One text body of an item, with its own source, preparation and rows:
/// the reply's prose, a plain (literal) section or a formatted section.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Slot {
    Prose,
    Plain(PlainSection),
    Text(TextSection),
    /// A tool's shown output and a question's answer: inline text only.
    Output,
    Answer,
    /// An opened activity's preview: markdown built from the item.
    Preview,
}
impl Slot {
    /// Literal bodies keep their bytes; the rest are Markdown.
    pub(crate) fn literal(self) -> bool {
        matches!(self, Self::Plain(_))
    }
    /// The body's text in `item`, if the item has it.
    pub(crate) fn text(self, item: &Item) -> Option<&artisan_api::TextContent> {
        match self {
            Self::Prose => match &item.kind {
                ItemKind::AssistantMessage { text, .. } => Some(text),
                _ => None,
            },
            Self::Plain(section) => section.text(item),
            Self::Text(section) => section.text(item),
            Self::Output | Self::Answer | Self::Preview => None,
        }
    }
}
/// The renderer's cache keys and element ids name the slot.
impl std::fmt::Display for Slot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Prose => "prose",
            Self::Plain(PlainSection::User) => "user",
            Self::Plain(PlainSection::AgentTask) => "agent-task",
            Self::Plain(PlainSection::AgentMessage) => "agent-message",
            Self::Plain(PlainSection::Error) => "error",
            Self::Plain(PlainSection::ImageCaption) => "caption",
            Self::Text(TextSection::Proposal) => "plan-proposal",
            Self::Text(TextSection::AgentResult) => "result-view",
            Self::Text(TextSection::Message) => "message",
            Self::Output => "output",
            Self::Answer => "answer",
            Self::Preview => "preview",
        })
    }
}

/// Plain content stays literal while each native row has a bounded text body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum PlainSection {
    User,
    AgentTask,
    AgentMessage,
    Error,
    ImageCaption,
}
impl PlainSection {
    pub(crate) fn slot(self) -> Slot {
        Slot::Plain(self)
    }
    pub(crate) fn order(self) -> u64 {
        match self {
            Self::User => 0,
            Self::AgentTask => 1,
            Self::AgentMessage => 2,
            Self::Error => 3,
            Self::ImageCaption => 4,
        }
    }
    pub(crate) fn inline_text(self, item: &Item) -> Option<&str> {
        if self == Self::AgentMessage
            && let ItemKind::Tool {
                tool:
                    artisan_api::ToolKind::AgentMessage {
                        text: artisan_api::TextContent::Inline(text),
                        summary: Some(summary),
                        ..
                    },
                ..
            } = &item.kind
            && text.is_empty()
        {
            return Some(summary);
        }
        self.text(item)?.inline()
    }
    pub(crate) fn text(self, item: &Item) -> Option<&artisan_api::TextContent> {
        match (self, &item.kind) {
            (Self::User, ItemKind::UserMessage { text, .. }) => Some(text),
            (
                Self::AgentTask,
                ItemKind::Agent {
                    description,
                    prompt,
                    ..
                },
            ) => Some(prompt.as_ref().unwrap_or(description)),
            (
                Self::AgentMessage,
                ItemKind::Tool {
                    tool: artisan_api::ToolKind::AgentMessage { text, .. },
                    ..
                },
            ) => Some(text),
            (Self::Error, ItemKind::Error { message, .. }) => Some(message),
            (Self::ImageCaption, ItemKind::Image { caption, .. }) => caption.as_ref(),
            _ => None,
        }
    }
}
/// Agent task/result metadata shares one Item position, so its scoped presentation
/// order surrounds child Items. Root transcript ordering remains canonical.
pub(crate) fn selection_order(agent_scope: bool, position: u64, slot: Slot) -> Vec<u64> {
    if agent_scope {
        vec![
            match slot {
                Slot::Plain(PlainSection::AgentTask) => 0,
                Slot::Text(TextSection::AgentResult) => 2,
                _ => 1,
            },
            position,
        ]
    } else {
        vec![position]
    }
}

/// UTF-8 byte partitions preserve every source byte, including whitespace.
/// Break at a nearby word/newline where possible; never measure arbitrary-length text.
pub(crate) fn plain_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
    const ROW_BYTES: usize = 1024;
    let mut ranges = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let mut end = text.floor_char_boundary((start + ROW_BYTES).min(text.len()));
        if end < text.len() {
            let prefix = &text[start..end];
            if let Some(index) = prefix
                .rfind(char::is_whitespace)
                .filter(|index| *index >= ROW_BYTES / 2)
            {
                end = start + index + prefix[index..].chars().next().unwrap().len_utf8();
            }
        }
        ranges.push(start..end);
        start = end;
    }
    if ranges.is_empty() {
        ranges.push(0..0);
    }
    ranges
}

/// Each independently prepared text body has its own cache and native row keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum TextSection {
    Proposal,
    AgentResult,
    Message,
}
impl TextSection {
    pub(crate) fn text(self, item: &Item) -> Option<&artisan_api::TextContent> {
        match (self, &item.kind) {
            (Self::Proposal, ItemKind::Plan { proposal, .. }) => proposal.as_ref(),
            (Self::AgentResult, ItemKind::Agent { result, .. }) => result.as_ref(),
            (
                Self::Message,
                ItemKind::Tool {
                    tool: artisan_api::ToolKind::AgentMessage { text, .. },
                    ..
                },
            ) => Some(text),
            _ => None,
        }
    }
}

/// A row's identity across folds: a work section keeps its key as items
/// join it, so its view (and its measured height) survive.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum RowKey {
    Projected(Box<RowKey>, usize),
    Item(ItemId),
    Text(ItemId, Slot, usize),
    Chain(ItemId),
    PlanStep(ItemId, usize),
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
    /// The whole of an item's text body, before it splits into blocks.
    pub(crate) fn text(id: ItemId, slot: Slot) -> Self {
        Self::Text {
            id,
            slot,
            block: 0,
            run: None,
        }
    }
    pub(crate) fn semantic(&self) -> &Self {
        match self {
            Self::Projected { row, .. } => row,
            _ => self,
        }
    }
    pub(crate) fn visual(&self) -> Option<usize> {
        match self {
            Self::Projected { visual, .. } => Some(*visual),
            _ => None,
        }
    }
    /// The item that anchors this row in the thread.
    pub(crate) fn anchor(&self) -> Option<&ItemId> {
        match self {
            Self::Projected { row, .. } => row.anchor(),
            Self::Text { id, .. }
            | Self::WorkEntry { id, .. }
            | Self::PlanStep { id, .. }
            | Self::Prompt(id)
            | Self::Compaction(id)
            | Self::Image(id)
            | Self::AgentResult(id) => Some(id),
            Self::Work { entries, .. } => entries.first(),
            Self::Chain { ids, .. } => ids.first(),
            Self::Gap | Self::Status(_) | Self::AgentStatus(..) => None,
        }
    }

    pub(crate) fn key(&self) -> RowKey {
        match self {
            Self::Projected { row, visual } => RowKey::Projected(Box::new(row.key()), *visual),
            Self::Text {
                id,
                slot: Slot::Plain(PlainSection::AgentTask),
                block: 0,
                ..
            } => RowKey::AgentTask(id.clone()),
            Self::Text {
                id,
                slot: Slot::Text(TextSection::AgentResult),
                block: 0,
                ..
            } => RowKey::AgentResult(id.clone()),
            Self::Text {
                id,
                slot:
                    Slot::Prose
                    | Slot::Plain(PlainSection::User | PlainSection::AgentMessage | PlainSection::Error),
                block: 0,
                ..
            } => RowKey::Item(id.clone()),
            Self::Text {
                id, slot, block, ..
            } => RowKey::Text(id.clone(), *slot, *block),
            Self::Chain { ids, .. } => RowKey::Chain(ids.first().cloned().unwrap_or_default()),
            Self::PlanStep { id, step, .. } => RowKey::PlanStep(id.clone(), *step),
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
            Self::AgentResult(id) => RowKey::AgentResult(id.clone()),
            Self::AgentStatus(id, _) => RowKey::AgentStatus(id.clone()),
            other => RowKey::Item(other.anchor().cloned().unwrap_or_default()),
        }
    }

    /// The row's name on screen for tests (`row:<selector>`): what it
    /// shows, never where it sits in the list.
    pub(crate) fn selector(&self) -> String {
        match self {
            Self::Projected { row, visual } => format!("{}:{visual}", row.selector()),
            Self::Work {
                run: Some(run),
                segment,
                ..
            } => format!("work:{run}:{segment}"),
            Self::Work {
                run: None, entries, ..
            } => format!("work:{}", entries.first().cloned().unwrap_or_default()),
            Self::WorkEntry { id, .. } => format!("entry:{id}"),
            Self::Chain { ids, .. } => {
                format!("chain:{}", ids.first().cloned().unwrap_or_default())
            }
            Self::PlanStep { id, step, .. } => format!("plan-step:{id}:{step}"),
            Self::Text {
                id, slot, block, ..
            } => format!("{slot}:{id}:{block}"),
            Self::Prompt(id) => format!("prompt:{id}"),
            Self::Compaction(id) => format!("compaction:{id}"),
            Self::Image(id) => format!("image:{id}"),
            Self::Gap => "gap".to_owned(),
            Self::Status(run) => format!("status:{run}"),
            Self::AgentResult(id) => format!("result-view:{id}:0"),
            Self::AgentStatus(id, working) => {
                format!(
                    "agent-status:{id}:{}",
                    if *working { "working" } else { "thinking" }
                )
            }
        }
    }

    /// The run this row reports on (work sections and the status line).
    pub(crate) fn run(&self) -> Option<&RunId> {
        match self {
            Self::Projected { row, .. } => row.run(),
            Self::Work { run, .. }
            | Self::WorkEntry { run, .. }
            | Self::Chain { run, .. }
            | Self::PlanStep { run, .. }
            | Self::Text { run, .. } => run.as_ref(),
            Self::Status(run) => Some(run),
            _ => None,
        }
    }
}

/// What a run's work section and status line show about the run.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RunSummary {
    pub phase: Phase,
    pub started_at: Option<Timestamp>,
    /// `None` while the run is live: the elapsed time counts to now.
    pub ended_at: Option<Timestamp>,
    /// The section is open (derived default, or the user's toggle).
    pub open: bool,
    /// The run used a tool or an agent (its header says "Working", not
    /// "Thinking").
    pub worked: bool,
}

impl RunSummary {
    pub(crate) fn live(&self) -> bool {
        !matches!(self.phase, Phase::Unknown | Phase::Settled(_))
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
    // Messages the root sent the open agent show in its view; its own
    // replies are already there as its own calls.
    let to_scope = |item: &Item| {
        scope.is_some_and(|agent| {
            activity::agent_message(item).is_some_and(|message| {
                message.direction == artisan_api::MessageDirection::Sent
                    && message.to.as_ref() == Some(agent)
            })
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
        // Goal lifecycle is shown once in the composer's thread-scoped strip.
        if matches!(
            &item.kind,
            ItemKind::Notice {
                kind: NoticeKind::Goal { .. },
                ..
            }
        ) {
            continue;
        }
        if scope.is_some() && to_scope(item) {
            flush(&mut work, &mut rows);
            rows.push(Row::text(
                item.id.clone(),
                Slot::Plain(PlainSection::AgentMessage),
            ));
            continue;
        }
        let own_row = match &item.kind {
            // A steer belongs to the running turn: it sits in the work
            // where it was sent and folds with it.
            ItemKind::UserMessage { steer: true, .. } if item.run_id.is_some() => None,
            ItemKind::UserMessage { .. } => {
                Some(Row::text(item.id.clone(), Slot::Plain(PlainSection::User)))
            }
            ItemKind::AssistantMessage {
                phase: TextPhase::Final,
                ..
            } if scope.is_none() => Some(Row::text(item.id.clone(), Slot::Prose)),
            ItemKind::Question { .. } => Some(Row::Prompt(item.id.clone())),
            ItemKind::Tool { .. } | ItemKind::Plan { .. }
                if item.status == Status::AwaitingUser =>
            {
                Some(Row::Prompt(item.id.clone()))
            }
            // A compaction during a run is part of its work; one outside
            // any run divides the transcript.
            ItemKind::Compaction { .. } if item.run_id.is_none() => {
                Some(Row::Compaction(item.id.clone()))
            }
            ItemKind::Error { .. } => {
                Some(Row::text(item.id.clone(), Slot::Plain(PlainSection::Error)))
            }
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
        rows.insert(
            0,
            Row::text(agent.clone(), Slot::Plain(PlainSection::AgentTask)),
        );
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

/// Expand disclosures into native list rows. No row builds an expanded work tree.
/// This separate presentation pass keeps the canonical fold independent of renderer readiness.
pub(crate) fn expanded(
    view: &ThreadView,
    rows: Vec<(Row, u64)>,
    runs: &HashMap<RunId, RunSummary>,
) -> Vec<(Row, u64)> {
    let children = children(&view.shown().collect::<Vec<_>>());
    let mut expanded = Vec::new();
    let mut emit = |row: Row| {
        let revision = version(view, &row, &children, runs);
        expanded.push((row, revision));
    };
    for (row, revision) in rows {
        match row {
            Row::Work {
                run,
                segment,
                entries,
            } => {
                let open = run
                    .as_ref()
                    .and_then(|id| runs.get(id))
                    .is_none_or(|summary| summary.open);
                // Continuations have no header. Closed sections have no descendant rows.
                if segment == 0 {
                    emit(Row::Work {
                        run: run.clone(),
                        segment,
                        entries: entries.clone(),
                    });
                }
                if !open {
                    continue;
                }
                let mut index = 0;
                while index < entries.len() {
                    let count = entries[index..]
                        .iter()
                        .take_while(|id| {
                            view.item(id)
                                .is_some_and(|item| activity::category(item).is_some())
                        })
                        .count();
                    if count > 0 {
                        let ids = entries[index..index + count].to_vec();
                        let chain_open = view
                            .ui
                            .get(&ids[0])
                            .and_then(|ui| ui.chain_open)
                            .unwrap_or(false);
                        emit(Row::Chain {
                            ids: ids.clone(),
                            run: run.clone(),
                        });
                        if chain_open {
                            for id in ids {
                                emit(Row::WorkEntry {
                                    id: id.clone(),
                                    run: run.clone(),
                                });
                                if view.ui.get(&id).and_then(|ui| ui.expanded).unwrap_or(false)
                                    && view.item(&id).is_some_and(|item| {
                                        matches!(
                                            item.kind,
                                            ItemKind::Tool {
                                                tool: artisan_api::ToolKind::AgentMessage { .. },
                                                ..
                                            }
                                        )
                                    })
                                {
                                    emit(Row::Text {
                                        id,
                                        slot: Slot::Text(TextSection::Message),
                                        block: 0,
                                        run: run.clone(),
                                    });
                                }
                            }
                        }
                        index += count;
                    } else {
                        let id = entries[index].clone();
                        index += 1;
                        match view.item(&id).map(|item| &item.kind) {
                            Some(ItemKind::AssistantMessage { .. }) => {
                                emit(Row::text(id, Slot::Prose));
                            }
                            Some(ItemKind::Plan {
                                steps, proposal, ..
                            }) => {
                                if let artisan_api::Collection::Inline(steps) = steps {
                                    for step in 0..steps.len() {
                                        emit(Row::PlanStep {
                                            id: id.clone(),
                                            step,
                                            run: run.clone(),
                                        });
                                    }
                                }
                                if proposal.is_some() {
                                    emit(Row::Text {
                                        id: id.clone(),
                                        slot: Slot::Text(TextSection::Proposal),
                                        block: 0,
                                        run: run.clone(),
                                    });
                                }
                                emit(Row::WorkEntry {
                                    id,
                                    run: run.clone(),
                                });
                            }
                            // A steer sits in the work as the message it is.
                            Some(ItemKind::UserMessage { .. }) => {
                                emit(Row::text(id, Slot::Plain(PlainSection::User)));
                            }
                            _ => emit(Row::WorkEntry {
                                id,
                                run: run.clone(),
                            }),
                        }
                    }
                }
            }
            Row::AgentResult(id) => emit(Row::Text {
                id,
                slot: Slot::Text(TextSection::AgentResult),
                block: 0,
                run: None,
            }),
            Row::Prompt(id)
                if view.item(&id).is_some_and(|item| {
                    matches!(
                        item.kind,
                        ItemKind::Plan {
                            proposal: Some(_),
                            ..
                        }
                    )
                }) =>
            {
                emit(Row::Text {
                    id: id.clone(),
                    slot: Slot::Text(TextSection::Proposal),
                    block: 0,
                    run: None,
                });
                emit(Row::Prompt(id));
            }
            Row::Image(id) => {
                let caption = view
                    .item(&id)
                    .is_some_and(|item| PlainSection::ImageCaption.text(item).is_some());
                emit(Row::Image(id.clone()));
                if caption {
                    emit(Row::text(id, Slot::Plain(PlainSection::ImageCaption)));
                }
            }
            other => {
                let _ = revision;
                emit(other);
            }
        }
    }
    expanded
}

/// Canonical result copy is prepared by the adapter; rendering does not reinterpret it.
pub(crate) fn agent_report(item: &Item) -> Option<&artisan_api::TextContent> {
    match &item.kind {
        ItemKind::Agent { result, .. } => result.as_ref(),
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

/// Phase, timing and fold state from canonical runs supplied with window pages.
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
            let latest = latest
                .filter(|run| &run.id == run_id)
                .or_else(|| view.runs.get(run_id));
            let (phase, started_at, ended_at) = if let Some(run) = latest {
                (
                    phase::turn_phase(run, &items, &view.live),
                    Some(run.started_at),
                    run.ended_at,
                )
            } else {
                (Phase::Unknown, None, None)
            };
            let live = !matches!(phase, Phase::Unknown | Phase::Settled(_));
            // Keep live work open. Canonical Final messages have their own rows
            // independently of run completion. Settled work keeps the user's
            // disclosure choice and defaults closed after success.
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

/// The space above native rows preserves the rhythm of turns (each a
/// user message and what the run did about it) and the blocks inside them.
pub(crate) fn gap(view: &ThreadView, rows: &[(Row, u64)], index: usize) -> f32 {
    let run_of = |row: &Row| -> Option<RunId> {
        match row {
            Row::Work { run, .. }
            | Row::WorkEntry { run, .. }
            | Row::Chain { run, .. }
            | Row::PlanStep { run, .. }
            | Row::Text {
                slot: Slot::Text(_),
                run,
                ..
            } => run.clone(),
            Row::Status(run) => Some(run.clone()),
            other => other
                .anchor()
                .and_then(|id| view.item(id))
                .and_then(|item| item.run_id.clone()),
        }
    };
    let row = &rows[index].0;
    {
        let Some((previous, _)) = index.checked_sub(1).and_then(|index| rows.get(index)) else {
            return PAD_TOP;
        };
        if row.visual().is_some_and(|visual| visual > 0) {
            return 0.0;
        }
        let (row, previous) = (row.semantic(), previous.semantic());
        let (run, previous_run) = (run_of(row), run_of(previous));
        match (previous, row) {
            (Row::Chain { .. }, Row::WorkEntry { id, .. })
                if view
                    .item(id)
                    .is_some_and(|item| activity::category(item).is_some()) =>
            {
                0.0
            }
            (Row::WorkEntry { id: previous, .. }, Row::WorkEntry { id, .. })
                if run == previous_run
                    && view
                        .item(previous)
                        .is_some_and(|item| activity::category(item).is_some())
                    && view
                        .item(id)
                        .is_some_and(|item| activity::category(item).is_some()) =>
            {
                4.0
            }
            (Row::PlanStep { id: previous, .. }, Row::PlanStep { id, .. }) if id == previous => 2.0,
            (
                Row::WorkEntry { id: previous, .. },
                Row::Text {
                    id,
                    slot: Slot::Text(TextSection::Message),
                    block: 0,
                    ..
                },
            ) if id == previous => 4.0,
            // A prepared body or expanded work entry is already one list row.
            (_, Row::Text { block: 1.., .. }) => 0.0,
            (
                Row::Work { .. }
                | Row::WorkEntry { .. }
                | Row::Chain { .. }
                | Row::PlanStep { .. }
                | Row::Text {
                    slot: Slot::Text(_),
                    ..
                },
                Row::WorkEntry { .. }
                | Row::Chain { .. }
                | Row::PlanStep { .. }
                | Row::Text {
                    slot: Slot::Text(_) | Slot::Prose,
                    ..
                },
            ) if run == previous_run => 8.0,
            (
                Row::Work { .. },
                Row::Text {
                    slot: Slot::Prose, ..
                },
            ) => REPLY_SEAM,
            (Row::Work { .. }, Row::Status(_)) => STATUS_AFTER_WORK,
            (_, Row::Status(_)) => STATUS_GAP,
            // A user message starts a turn; a steer stays inside its
            // run's turn.
            (
                _,
                Row::Text {
                    slot: Slot::Plain(PlainSection::User),
                    ..
                },
            ) if run.is_some() && run == previous_run => BLOCK_GAP,
            (
                _,
                Row::Text {
                    slot: Slot::Plain(PlainSection::User),
                    ..
                },
            ) => TURN_GAP,
            (
                Row::Text {
                    slot: Slot::Plain(PlainSection::User),
                    ..
                },
                _,
            ) => BLOCK_GAP,
            _ if run.is_some() && previous_run.is_some() && run != previous_run => TURN_GAP,
            _ => BLOCK_GAP,
        }
    }
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
        Row::Work { .. } => Vec::new(),
        Row::Chain { ids, .. } => ids.iter().collect(),
        Row::Status(run) => {
            view.live
                .get(&LiveTarget::Reasoning {
                    run_id: run.clone(),
                })
                .map(std::sync::Arc::as_ptr)
                .hash(&mut hasher);
            view.live
                .get(&LiveTarget::Status {
                    run_id: run.clone(),
                })
                .map(std::sync::Arc::as_ptr)
                .hash(&mut hasher);
            Vec::new()
        }
        Row::AgentResult(id) | Row::AgentStatus(id, _) => {
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
            view.live
                .get(&target)
                .map(std::sync::Arc::as_ptr)
                .hash(&mut hasher);
        }
        if !matches!(
            row,
            Row::Chain { .. }
                | Row::WorkEntry { .. }
                | Row::PlanStep { .. }
                | Row::Text {
                    slot: Slot::Text(_),
                    ..
                }
        ) && let Some(nested) = children.get(id)
        {
            stack.extend(nested.iter().map(|item| &item.id));
        }
    }
    hasher.finish()
}

#[cfg(test)]
mod run_metadata_tests {
    use super::*;
    use crate::viewport::Fetch;
    use artisan_api::{EngineId, ItemsPage, Usage};

    #[test]
    fn historical_run_summary_uses_canonical_metadata_across_partial_pages() {
        let thread = artisan_api::ThreadId::from("history");
        let run = Run {
            id: RunId::from("historical"),
            thread_id: thread.clone(),
            engine: EngineId::Claude,
            model: None,
            input_cutoff: 0,
            status: RunStatus::Cancelled,
            usage: Usage::default(),
            session_id: None,
            error: None,
            seq: 20,
            started_at: 100,
            ended_at: Some(1000),
        };
        for status in [Status::Done, Status::Cancelled] {
            let item = Item {
                id: ItemId::new(),
                thread_id: thread.clone(),
                position: 5,
                seq: 5,
                run_id: Some(run.id.clone()),
                parent_id: None,
                status,
                created_at: 400,
                updated_at: 600,
                kind: ItemKind::Error {
                    kind: artisan_api::ErrorKind::Execution,
                    message: "an item is not the run outcome".into(),
                    retryable: false,
                },
            };
            let (mut view, _) = ThreadView::open(thread.clone());
            view.apply_page(
                Fetch::Tail,
                ItemsPage {
                    items: vec![item.clone()],
                    runs: vec![run.clone()],
                    more: true,
                },
            );
            let folded = fold(&view, None, None);
            let summary = &folded.runs[&run.id];
            assert_eq!(summary.phase, Phase::Settled(RunStatus::Cancelled));
            assert_eq!(summary.started_at, Some(100));
            assert_eq!(summary.ended_at, Some(1000));
            assert!(!summary.live());

            let (mut missing, _) = ThreadView::open(thread.clone());
            missing.apply_page(
                Fetch::Tail,
                ItemsPage {
                    items: vec![item],
                    runs: vec![],
                    more: true,
                },
            );
            let unknown = fold(&missing, None, None);
            let summary = &unknown.runs[&run.id];
            assert_eq!(summary.phase, Phase::Unknown);
            assert_eq!(summary.started_at, None);
            assert_eq!(summary.ended_at, None);
            assert!(!summary.live());
            assert!(
                summary.open,
                "unknown history remains accessible without a fabricated outcome"
            );
        }
    }
}
