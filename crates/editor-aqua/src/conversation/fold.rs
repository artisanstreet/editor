use super::activity;
use super::phase::{self, Phase};
use super::thread::ThreadView;
use artisan_api::{
    Item, ItemId, ItemKind, LiveTarget, NoticeKind, Phase as TextPhase, Run, RunId, RunStatus, Seq,
    Status, Timestamp,
};
use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher as _};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Row {
    Projected {
        row: Box<Self>,
        visual: usize,
    },
    Work {
        run: Option<RunId>,
        segment: usize,
        entries: Vec<ItemId>,
        unloaded: Vec<Seq>,
    },
    EarlierWork {
        run: Option<RunId>,
        first: Seq,
        member: ItemId,
    },
    WorkEntry {
        id: ItemId,
        run: Option<RunId>,
    },
    Chain {
        ids: Vec<ItemId>,
        run: Option<RunId>,
    },
    Text {
        id: ItemId,
        slot: Slot,
        block: usize,
        run: Option<RunId>,
    },
    Prompt(ItemId),
    Compaction(ItemId),
    Image(ItemId),
    Gap,
    Status(RunId),
    AgentResult(ItemId),
    AgentStatus(ItemId, bool),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Slot {
    Prose,
    Plain(PlainSection),
    Text(TextSection),
    Output,
    Answer,
    Preview,
}
impl Slot {
    pub(crate) const fn is_card(self) -> bool {
        matches!(self, Self::Text(TextSection::Message | TextSection::Report))
    }
    pub(crate) const fn literal(self) -> bool {
        matches!(self, Self::Plain(_))
    }
    pub(crate) fn text(self, item: &Item) -> Option<&artisan_api::TextContent> {
        match self {
            Self::Prose => match &item.kind {
                ItemKind::AssistantMessage { text, .. } => Some(text),
                ItemKind::UserMessage { .. }
                | ItemKind::Tool { .. }
                | ItemKind::Agent { .. }
                | ItemKind::Question { .. }
                | ItemKind::Plan { .. }
                | ItemKind::Image { .. }
                | ItemKind::Compaction { .. }
                | ItemKind::Notice { .. }
                | ItemKind::Error { .. } => None,
            },
            Self::Plain(section) => section.text(item),
            Self::Text(section) => section.text(item),
            Self::Output | Self::Answer | Self::Preview => None,
        }
    }
}
impl std::fmt::Display for Slot {
    #[expect(
        clippy::match_same_arms,
        reason = "one name per slot reads as a table; user bubbles share a name on purpose"
    )]
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
            Self::Text(TextSection::Report) => "report",
            Self::Text(TextSection::Message) => "message",
            Self::Text(TextSection::Note) => "note",
            Self::Text(TextSection::User) => "user",
            Self::Output => "output",
            Self::Answer => "answer",
            Self::Preview => "preview",
        })
    }
}

const fn user_slot(item: &Item) -> Slot {
    match &item.kind {
        ItemKind::UserMessage {
            text: artisan_api::TextContent::Stored(_),
            sender: None,
            ..
        } => Slot::Plain(PlainSection::User),
        ItemKind::UserMessage { .. }
        | ItemKind::AssistantMessage { .. }
        | ItemKind::Tool { .. }
        | ItemKind::Agent { .. }
        | ItemKind::Question { .. }
        | ItemKind::Plan { .. }
        | ItemKind::Image { .. }
        | ItemKind::Compaction { .. }
        | ItemKind::Notice { .. }
        | ItemKind::Error { .. } => Slot::Text(TextSection::User),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PlainSection {
    User,
    AgentTask,
    AgentMessage,
    Error,
    ImageCaption,
}
impl PlainSection {
    pub(crate) fn inline_text(self, item: &Item) -> Option<&str> {
        if self == Self::AgentTask
            && let ItemKind::Agent {
                prompt: None,
                description,
                name,
                ..
            } = &item.kind
            && let Some(description) = description.inline()
        {
            if name.as_deref().is_some_and(|name| {
                (name == "/root" || name.starts_with("/root/")) && name == description
            }) {
                return Some("[This message is encrypted.]");
            }
            if description.is_empty() {
                return Some("Task message not provided by the harness.");
            }
        }
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

    pub(crate) const fn order(self) -> u64 {
        match self {
            Self::User => 0,
            Self::AgentTask => 1,
            Self::AgentMessage => 2,
            Self::Error => 3,
            Self::ImageCaption => 4,
        }
    }
    #[expect(
        clippy::match_same_arms,
        reason = "one arm per section and item kind reads as a table"
    )]
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
pub fn selection_order(agent_scope: bool, position: u64, slot: Slot) -> Vec<u64> {
    if agent_scope {
        vec![
            match slot {
                Slot::Plain(PlainSection::AgentTask) => 0,
                Slot::Text(TextSection::AgentResult) => 2,
                Slot::Prose
                | Slot::Plain(_)
                | Slot::Text(_)
                | Slot::Output
                | Slot::Answer
                | Slot::Preview => 1,
            },
            position,
        ]
    } else {
        vec![position]
    }
}

pub fn plain_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
    const ROW_BYTES: usize = 1024;
    let mut ranges = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let mut end = text.floor_char_boundary((start + ROW_BYTES).min(text.len()));
        if end < text.len() {
            let prefix = &text[start..end];
            if let Some((index, space)) = prefix
                .char_indices()
                .rev()
                .find(|(_, char)| char.is_whitespace())
                .filter(|(index, _)| *index >= ROW_BYTES / 2)
            {
                end = start + index + space.len_utf8();
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextSection {
    Proposal,
    AgentResult,
    Report,
    Message,
    Note,
    User,
}
impl TextSection {
    #[expect(
        clippy::match_same_arms,
        reason = "one arm per section and item kind reads as a table"
    )]
    pub(crate) const fn text(self, item: &Item) -> Option<&artisan_api::TextContent> {
        match (self, &item.kind) {
            (Self::Proposal, ItemKind::Plan { proposal, .. }) => proposal.as_ref(),
            (Self::AgentResult | Self::Report, ItemKind::Agent { result, .. }) => result.as_ref(),
            (
                Self::Message,
                ItemKind::Tool {
                    tool: artisan_api::ToolKind::AgentMessage { text, .. },
                    ..
                },
            ) => Some(text),
            (
                Self::Message,
                ItemKind::UserMessage {
                    text,
                    from_agent: Some(_),
                    ..
                },
            ) => Some(text),
            (Self::User, ItemKind::UserMessage { text, .. }) => Some(text),
            (
                Self::Note,
                ItemKind::Notice {
                    kind: NoticeKind::YouShouldKnow { .. },
                    text,
                },
            ) => Some(text),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum RowKey {
    Projected(Box<Self>, usize),
    Item(ItemId),
    Text(ItemId, Slot, usize),
    Chain(ItemId),
    Work(RunId, usize),
    Loose(ItemId),
    EarlierWork(Seq),
    LooseSpan(Seq),
    Gap,
    Status(RunId),
    AgentTask(ItemId),
    AgentResult(ItemId),
    AgentStatus(ItemId),
}

impl Row {
    pub(crate) const fn text(id: ItemId, slot: Slot) -> Self {
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
            Self::Work { .. }
            | Self::EarlierWork { .. }
            | Self::WorkEntry { .. }
            | Self::Chain { .. }
            | Self::Text { .. }
            | Self::Prompt(_)
            | Self::Compaction(_)
            | Self::Image(_)
            | Self::Gap
            | Self::Status(_)
            | Self::AgentResult(_)
            | Self::AgentStatus(..) => self,
        }
    }
    pub(crate) const fn visual(&self) -> Option<usize> {
        match self {
            Self::Projected { visual, .. } => Some(*visual),
            Self::Work { .. }
            | Self::EarlierWork { .. }
            | Self::WorkEntry { .. }
            | Self::Chain { .. }
            | Self::Text { .. }
            | Self::Prompt(_)
            | Self::Compaction(_)
            | Self::Image(_)
            | Self::Gap
            | Self::Status(_)
            | Self::AgentResult(_)
            | Self::AgentStatus(..) => None,
        }
    }
    pub(crate) fn anchor(&self) -> Option<&ItemId> {
        match self {
            Self::Projected { row, .. } => row.anchor(),
            Self::Text { id, .. }
            | Self::WorkEntry { id, .. }
            | Self::Prompt(id)
            | Self::Compaction(id)
            | Self::Image(id)
            | Self::AgentResult(id) => Some(id),
            Self::Work { entries, .. } => entries.first(),
            Self::Chain { ids, .. } => ids.first(),
            Self::Gap | Self::Status(_) | Self::AgentStatus(..) | Self::EarlierWork { .. } => None,
        }
    }

    pub(crate) fn last_anchor(&self) -> Option<&ItemId> {
        match self {
            Self::Projected { row, .. } => row.last_anchor(),
            Self::Work { entries, .. } => entries.last(),
            Self::Chain { ids, .. } => ids.last(),
            row @ (Self::EarlierWork { .. }
            | Self::WorkEntry { .. }
            | Self::Text { .. }
            | Self::Prompt(_)
            | Self::Compaction(_)
            | Self::Image(_)
            | Self::Gap
            | Self::Status(_)
            | Self::AgentResult(_)
            | Self::AgentStatus(..)) => row.anchor(),
        }
    }

    pub(crate) fn member(&self) -> Option<&ItemId> {
        match self.semantic() {
            Self::Work { .. } => None,
            Self::EarlierWork { member, .. } => Some(member),
            row @ (Self::Projected { .. }
            | Self::WorkEntry { .. }
            | Self::Chain { .. }
            | Self::Text { .. }
            | Self::Prompt(_)
            | Self::Compaction(_)
            | Self::Image(_)
            | Self::Gap
            | Self::Status(_)
            | Self::AgentResult(_)
            | Self::AgentStatus(..)) => row.anchor(),
        }
    }

    pub fn key(&self) -> RowKey {
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
            }
            | Self::AgentResult(id) => RowKey::AgentResult(id.clone()),
            Self::Text {
                id,
                slot:
                    Slot::Prose
                    | Slot::Text(TextSection::User)
                    | Slot::Plain(PlainSection::User | PlainSection::AgentMessage | PlainSection::Error),
                block: 0,
                ..
            } => RowKey::Item(id.clone()),
            Self::Text {
                id, slot, block, ..
            } => RowKey::Text(id.clone(), *slot, *block),
            Self::Chain { ids, .. } => RowKey::Chain(ids.first().cloned().unwrap_or_default()),
            Self::Work {
                run: Some(run),
                segment,
                ..
            } => RowKey::Work(run.clone(), *segment),
            Self::Work {
                run: None,
                entries,
                unloaded,
                ..
            } => match (entries.first(), unloaded.first()) {
                (Some(entry), _) => RowKey::Loose(entry.clone()),
                (None, Some(first)) => RowKey::LooseSpan(*first),
                (None, None) => RowKey::Loose(ItemId::default()),
            },
            Self::EarlierWork { first, .. } => RowKey::EarlierWork(*first),
            Self::Gap => RowKey::Gap,
            Self::Status(run) => RowKey::Status(run.clone()),
            Self::AgentStatus(id, _) => RowKey::AgentStatus(id.clone()),
            other @ (Self::WorkEntry { .. }
            | Self::Prompt(_)
            | Self::Compaction(_)
            | Self::Image(_)) => RowKey::Item(other.anchor().cloned().unwrap_or_default()),
        }
    }

    pub(crate) fn is_user(&self) -> bool {
        matches!(
            self.semantic(),
            Self::Text {
                slot: Slot::Plain(PlainSection::User) | Slot::Text(TextSection::User),
                ..
            }
        )
    }

    pub(crate) fn run(&self) -> Option<&RunId> {
        match self {
            Self::Projected { row, .. } => row.run(),
            Self::Work { run, .. }
            | Self::WorkEntry { run, .. }
            | Self::Chain { run, .. }
            | Self::EarlierWork { run, .. }
            | Self::Text { run, .. } => run.as_ref(),
            Self::Status(run) => Some(run),
            Self::Prompt(_)
            | Self::Compaction(_)
            | Self::Image(_)
            | Self::Gap
            | Self::AgentResult(_)
            | Self::AgentStatus(..) => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RunSummary {
    pub phase: Phase,
    pub started_at: Option<Timestamp>,
    pub ended_at: Option<Timestamp>,
    pub open: bool,
    pub worked: bool,
}

impl RunSummary {
    pub(crate) const fn live(&self) -> bool {
        !matches!(self.phase, Phase::Unknown | Phase::Settled(_))
    }
}

pub struct Folded {
    pub rows: Vec<(Row, u64)>,
    pub runs: HashMap<RunId, RunSummary>,
}

pub const fn in_composer_strip(item: &Item) -> bool {
    matches!(
        &item.kind,
        ItemKind::Notice {
            kind: NoticeKind::Goal { .. } | NoticeKind::YouShouldKnow { .. },
            ..
        }
    )
}

pub fn turn_of(view: &ThreadView, latest: Option<&Run>, run: &RunId) -> RunId {
    let find = |id: &RunId| {
        latest
            .filter(|latest| &latest.id == id)
            .or_else(|| view.runs.get(id))
    };
    let mut current = run.clone();
    while let Some(carried) = find(&current).filter(|run| run.continuation.is_some()) {
        let previous = view
            .runs
            .values()
            .chain(latest)
            .filter(|other| other.started_at < carried.started_at)
            .max_by_key(|other| other.started_at);
        match previous {
            Some(previous)
                if !matches!(
                    previous.status,
                    RunStatus::Done | RunStatus::Failed | RunStatus::Refused
                ) =>
            {
                current = previous.id.clone();
            }
            _ => break,
        }
    }
    current
}

fn ended_empty(item: &Item) -> bool {
    item.status.is_terminal()
        && matches!(&item.kind, ItemKind::AssistantMessage { text, .. }
            if text.inline().is_some_and(|text| text.trim().is_empty()))
}

#[expect(
    clippy::too_many_lines,
    reason = "one pass over the items, in transcript order, deciding each one's row"
)]
pub fn fold(view: &ThreadView, latest: Option<&Run>, scope: Option<&ItemId>) -> Folded {
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
        .filter(|item| !ended_empty(item))
        .filter(|item| scope.is_none_or(|agent| descends_from(view, item, agent) || to_scope(item)))
        .collect();
    let loaded: HashSet<&ItemId> = shown.iter().map(|item| &item.id).collect();
    let children = children(&shown);
    let unloaded: Vec<&artisan_api::WorkSpan> = if scope.is_none() {
        view.shown_work().collect()
    } else {
        Vec::new()
    };
    let runs = summaries(view, &shown, &unloaded, latest);
    let turn = |run: &Option<RunId>| run.as_ref().map(|run| turn_of(view, latest, run));
    let mut rows: Vec<Row> = Vec::new();
    let mut work: Option<(Option<RunId>, Vec<ItemId>, Vec<Seq>)> = None;
    let mut segments: HashMap<Option<RunId>, usize> = HashMap::new();
    let mut run_starts: HashMap<RunId, Seq> = HashMap::new();
    for item in &shown {
        if let (Some(run), ItemKind::UserMessage { steer: false, .. }) =
            (turn(&item.run_id), &item.kind)
        {
            run_starts.entry(run).or_insert(item.position);
        }
    }
    let mut flush = |work: &mut Option<(Option<RunId>, Vec<ItemId>, Vec<Seq>)>,
                     rows: &mut Vec<Row>| {
        if let Some((run, entries, unloaded)) = work.take() {
            let next = segments.entry(run.clone()).or_default();
            rows.push(Row::Work {
                run,
                segment: *next,
                entries,
                unloaded,
            });
            *next += 1;
        }
    };
    let mut context: Option<RunId> = None;
    let mut spans = unloaded.iter().peekable();
    for item in &shown {
        while let Some(span) = spans.next_if(|span| span.first < item.position) {
            fold_span(
                span,
                &turn,
                &run_starts,
                &mut context,
                &mut work,
                &mut rows,
                &mut flush,
            );
        }
        if item.parent_id.as_ref().is_some_and(|parent| {
            scope.is_none() || (loaded.contains(parent) && Some(parent) != scope)
        }) {
            continue;
        }
        if in_composer_strip(item)
            || matches!(item.kind, ItemKind::UserMessage { .. })
                && item.id.as_str().starts_with("answer-")
        {
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
        let mid_run = match &work {
            Some((current, ..)) => {
                scope.is_some() || current.is_some() && *current == turn(&item.run_id)
            }
            None => false,
        };
        let own_row = match &item.kind {
            ItemKind::UserMessage { steer: true, .. } if item.run_id.is_some() => None,
            ItemKind::UserMessage {
                from_agent: Some(_),
                ..
            } => None,
            ItemKind::UserMessage { .. } => Some(Row::text(item.id.clone(), user_slot(item))),
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
            ItemKind::Compaction { .. } if !mid_run => Some(Row::Compaction(item.id.clone())),
            ItemKind::Error { .. } => {
                Some(Row::text(item.id.clone(), Slot::Plain(PlainSection::Error)))
            }
            ItemKind::Image { .. } => Some(Row::Image(item.id.clone())),
            ItemKind::AssistantMessage { .. }
            | ItemKind::Tool { .. }
            | ItemKind::Agent { .. }
            | ItemKind::Plan { .. }
            | ItemKind::Compaction { .. }
            | ItemKind::Notice { .. } => None,
        };
        if let Some(row) = own_row {
            flush(&mut work, &mut rows);
            rows.push(row);
            context = turn(&item.run_id);
            continue;
        }
        let run = if scope.is_some() {
            None
        } else if turn(&item.run_id)
            .and_then(|run| run_starts.get(&run))
            .is_some_and(|start| item.position < *start)
        {
            context.clone()
        } else {
            turn(&item.run_id)
        };
        context.clone_from(&run);
        match &mut work {
            Some((current, entries, _)) if *current == run => entries.push(item.id.clone()),
            _ => {
                flush(&mut work, &mut rows);
                work = Some((run, vec![item.id.clone()], Vec::new()));
            }
        }
    }
    for span in spans {
        fold_span(
            span,
            &turn,
            &run_starts,
            &mut context,
            &mut work,
            &mut rows,
            &mut flush,
        );
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
    if scope.is_none() {
        head_every_run(&mut rows, &shown, &run_starts, latest, &turn);
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

fn head_every_run(
    rows: &mut Vec<Row>,
    shown: &[&Item],
    run_starts: &HashMap<RunId, Seq>,
    latest: Option<&Run>,
    turn: &impl Fn(&Option<RunId>) -> Option<RunId>,
) {
    let headed: HashSet<RunId> = rows
        .iter()
        .filter_map(|row| match row {
            Row::Work { run: Some(run), .. } => Some(run.clone()),
            Row::Projected { .. }
            | Row::Work { .. }
            | Row::EarlierWork { .. }
            | Row::WorkEntry { .. }
            | Row::Chain { .. }
            | Row::Text { .. }
            | Row::Prompt(_)
            | Row::Compaction(_)
            | Row::Image(_)
            | Row::Gap
            | Row::Status(_)
            | Row::AgentResult(_)
            | Row::AgentStatus(..) => None,
        })
        .collect();
    let work_header = |run: RunId| Row::Work {
        run: Some(run),
        segment: 0,
        entries: Vec::new(),
        unloaded: Vec::new(),
    };
    let mut inserts: Vec<(usize, RunId)> = run_starts
        .iter()
        .filter(|(run, _)| !headed.contains(*run))
        .filter_map(|(run, start)| {
            let message = shown.iter().find(|item| {
                item.position == *start
                    && matches!(item.kind, ItemKind::UserMessage { steer: false, .. })
            })?;
            let at = rows
                .iter()
                .rposition(|row| row.anchor() == Some(&message.id))?;
            Some((at + 1, run.clone()))
        })
        .collect();
    inserts.sort_by_key(|&(at, _)| std::cmp::Reverse(at));
    let inserted: HashSet<RunId> = inserts.iter().map(|(_, run)| run.clone()).collect();
    for (at, run) in inserts {
        rows.insert(at, work_header(run));
    }
    if let Some(run) = latest.filter(|run| run.status.is_live()) {
        let id = turn(&Some(run.id.clone())).unwrap_or_else(|| run.id.clone());
        if !headed.contains(&id) && !inserted.contains(&id) {
            let status = rows
                .iter()
                .rposition(|row| matches!(row, Row::Status(_)))
                .unwrap_or(rows.len());
            rows.insert(status, work_header(id));
        }
    }
}

pub fn span_member(first: Seq) -> ItemId {
    ItemId::from(format!("unloaded-work:{first}"))
}

fn fold_span(
    span: &artisan_api::WorkSpan,
    turn: &impl Fn(&Option<RunId>) -> Option<RunId>,
    run_starts: &HashMap<RunId, Seq>,
    context: &mut Option<RunId>,
    work: &mut Option<(Option<RunId>, Vec<ItemId>, Vec<Seq>)>,
    rows: &mut Vec<Row>,
    flush: &mut impl FnMut(&mut Option<(Option<RunId>, Vec<ItemId>, Vec<Seq>)>, &mut Vec<Row>),
) {
    let run = if turn(&span.run_id)
        .and_then(|run| run_starts.get(&run))
        .is_some_and(|start| span.first < *start)
    {
        context.clone()
    } else {
        turn(&span.run_id)
    };
    context.clone_from(&run);
    match work {
        Some((current, _, unloaded)) if *current == run => unloaded.push(span.first),
        _ => {
            flush(work, rows);
            *work = Some((run, Vec::new(), vec![span.first]));
        }
    }
}

pub fn expanded(
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
                unloaded,
            } => {
                let open = run
                    .as_ref()
                    .and_then(|id| runs.get(id))
                    .is_none_or(|summary| summary.open);
                if segment == 0 {
                    emit(Row::Work {
                        run: run.clone(),
                        segment,
                        entries: entries.clone(),
                        unloaded: unloaded.clone(),
                    });
                }
                if !open {
                    continue;
                }
                let mut rest = entries.as_slice();
                for first in &unloaded {
                    let before = rest
                        .iter()
                        .take_while(|id| view.position_of(id).is_some_and(|at| at < *first))
                        .count();
                    expand_entries(view, run.as_ref(), &rest[..before], &mut emit);
                    rest = &rest[before..];
                    emit(Row::EarlierWork {
                        run: run.clone(),
                        first: *first,
                        member: span_member(*first),
                    });
                }
                expand_entries(view, run.as_ref(), rest, &mut emit);
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
            other @ (Row::Projected { .. }
            | Row::EarlierWork { .. }
            | Row::WorkEntry { .. }
            | Row::Chain { .. }
            | Row::Text { .. }
            | Row::Prompt(_)
            | Row::Compaction(_)
            | Row::Gap
            | Row::Status(_)
            | Row::AgentStatus(..)) => {
                let _ = revision;
                emit(other);
            }
        }
    }
    expanded
}

#[expect(
    clippy::too_many_lines,
    reason = "one walk over a section's entries, emitting chains and loose entries in order"
)]
fn expand_entries(
    view: &ThreadView,
    run: Option<&RunId>,
    entries: &[ItemId],
    emit: &mut impl FnMut(Row),
) {
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
                run: run.cloned(),
            });
            if chain_open {
                for id in ids {
                    emit(Row::WorkEntry {
                        id: id.clone(),
                        run: run.cloned(),
                    });
                    let expanded = view.ui.get(&id).and_then(|ui| ui.expanded).unwrap_or(false);
                    let reported = view.item(&id).and_then(|item| match &item.kind {
                        ItemKind::UserMessage {
                            from_agent: Some(from),
                            ..
                        } => Some(from.clone()),
                        ItemKind::UserMessage { .. }
                        | ItemKind::AssistantMessage { .. }
                        | ItemKind::Tool { .. }
                        | ItemKind::Agent { .. }
                        | ItemKind::Question { .. }
                        | ItemKind::Plan { .. }
                        | ItemKind::Image { .. }
                        | ItemKind::Compaction { .. }
                        | ItemKind::Notice { .. }
                        | ItemKind::Error { .. } => None,
                    });
                    if let Some(agent) = reported.filter(|agent| {
                        expanded && view.item(agent).and_then(agent_report).is_some()
                    }) {
                        emit(Row::Text {
                            id: agent,
                            slot: Slot::Text(TextSection::Report),
                            block: 0,
                            run: run.cloned(),
                        });
                    } else if expanded
                        && view
                            .item(&id)
                            .is_some_and(|item| activity::agent_message(item).is_some())
                    {
                        emit(Row::Text {
                            id,
                            slot: Slot::Text(TextSection::Message),
                            block: 0,
                            run: run.cloned(),
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
                    proposal, approval, ..
                }) => {
                    if proposal.is_some() {
                        emit(Row::Text {
                            id: id.clone(),
                            slot: Slot::Text(TextSection::Proposal),
                            block: 0,
                            run: run.cloned(),
                        });
                    }
                    if approval.is_some() {
                        emit(Row::WorkEntry {
                            id,
                            run: run.cloned(),
                        });
                    }
                }
                Some(ItemKind::Compaction { .. }) => emit(Row::Compaction(id)),
                Some(ItemKind::UserMessage { .. }) => {
                    let slot = view
                        .item(&id)
                        .map_or(Slot::Text(TextSection::User), user_slot);
                    emit(Row::text(id, slot));
                }
                _ => emit(Row::WorkEntry {
                    id,
                    run: run.cloned(),
                }),
            }
        }
    }
}

pub const fn agent_report(item: &Item) -> Option<&artisan_api::TextContent> {
    match &item.kind {
        ItemKind::Agent { result, .. } => result.as_ref(),
        ItemKind::UserMessage { .. }
        | ItemKind::AssistantMessage { .. }
        | ItemKind::Tool { .. }
        | ItemKind::Question { .. }
        | ItemKind::Plan { .. }
        | ItemKind::Image { .. }
        | ItemKind::Compaction { .. }
        | ItemKind::Notice { .. }
        | ItemKind::Error { .. } => None,
    }
}

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

pub fn children<'a>(items: &[&'a Item]) -> HashMap<ItemId, Vec<&'a Item>> {
    let mut children: HashMap<ItemId, Vec<&Item>> = HashMap::new();
    for item in items {
        if let Some(parent) = &item.parent_id {
            children.entry(parent.clone()).or_default().push(item);
        }
    }
    children
}

fn summaries(
    view: &ThreadView,
    shown: &[&Item],
    unloaded: &[&artisan_api::WorkSpan],
    latest: Option<&Run>,
) -> HashMap<RunId, RunSummary> {
    let mut by_run: HashMap<&RunId, Vec<&Item>> = HashMap::new();
    for item in shown {
        if let Some(run) = &item.run_id {
            by_run.entry(run).or_default().push(item);
        }
    }
    let mut unloaded_work: HashSet<&RunId> = HashSet::new();
    for span in unloaded {
        if let Some(run) = &span.run_id {
            by_run.entry(run).or_default();
            if span.worked {
                unloaded_work.insert(run);
            }
        }
    }
    if let Some(run) = latest {
        by_run.entry(&run.id).or_default();
    }
    let mut summaries: HashMap<RunId, RunSummary> = by_run
        .into_iter()
        .map(|(run_id, items)| {
            let latest = latest
                .filter(|run| &run.id == run_id)
                .or_else(|| view.runs.get(run_id));
            let (phase, started_at, ended_at) =
                latest.map_or((Phase::Unknown, None, None), |run| {
                    (
                        phase::turn_phase(run, &items, &view.live),
                        Some(run.started_at),
                        run.ended_at,
                    )
                });
            let live = !matches!(phase, Phase::Unknown | Phase::Settled(_));
            let open = if live {
                true
            } else {
                view.work_open
                    .get(run_id)
                    .copied()
                    .unwrap_or_else(|| phase != Phase::Settled(RunStatus::Done))
            };
            (
                run_id.clone(),
                RunSummary {
                    phase,
                    started_at,
                    ended_at: if live { None } else { ended_at },
                    open,
                    worked: unloaded_work.contains(run_id)
                        || items.iter().any(|item| {
                            matches!(
                                item.kind,
                                ItemKind::Tool { .. }
                                    | ItemKind::Agent { .. }
                                    | ItemKind::Plan { .. }
                            )
                        }),
                },
            )
        })
        .collect();
    let mut turns: HashMap<RunId, Vec<RunId>> = HashMap::new();
    for run in summaries.keys() {
        turns
            .entry(turn_of(view, latest, run))
            .or_default()
            .push(run.clone());
    }
    for (root, mut members) in turns {
        if members.len() < 2 && members.first() == Some(&root) {
            continue;
        }
        members.sort_by_key(|run| summaries[run].started_at);
        let (first, last) = (
            &summaries[&members[0]],
            &summaries[&members[members.len() - 1]],
        );
        let live = last.live();
        let open = if live {
            true
        } else {
            view.work_open
                .get(&root)
                .copied()
                .unwrap_or_else(|| last.phase != Phase::Settled(RunStatus::Done))
        };
        let merged = RunSummary {
            phase: last.phase,
            started_at: first.started_at,
            ended_at: last.ended_at,
            open,
            worked: members.iter().any(|run| summaries[run].worked),
        };
        summaries.insert(root, merged.clone());
        for run in members {
            summaries.insert(run, merged.clone());
        }
    }
    summaries
}

const PAD_TOP: f32 = 40.0;
const TURN_GAP: f32 = 32.0;
const BLOCK_GAP: f32 = 24.0;
const REPLY_SEAM: f32 = 8.0;
const STATUS_AFTER_WORK: f32 = 12.0;
const STATUS_GAP: f32 = 32.0;

#[expect(
    clippy::too_many_lines,
    reason = "the spacing rules between every pair of row kinds read as one table"
)]
pub fn gap(view: &ThreadView, rows: &[(Row, u64)], index: usize) -> f32 {
    let run_of = |row: &Row| -> Option<RunId> {
        match row {
            Row::Work { run, .. }
            | Row::WorkEntry { run, .. }
            | Row::Chain { run, .. }
            | Row::EarlierWork { run, .. } => run.clone(),
            Row::Text {
                slot: Slot::Text(section),
                run,
                ..
            } if *section != TextSection::User => run.clone(),
            Row::Status(run) => Some(turn_of(view, None, run)),
            other @ (Row::Projected { .. }
            | Row::Text { .. }
            | Row::Prompt(_)
            | Row::Compaction(_)
            | Row::Image(_)
            | Row::Gap
            | Row::AgentResult(_)
            | Row::AgentStatus(..)) => other
                .anchor()
                .and_then(|id| view.item(id))
                .and_then(|item| item.run_id.as_ref())
                .map(|run| turn_of(view, None, run)),
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
            (
                Row::WorkEntry { id: previous, .. },
                Row::Text {
                    id,
                    slot: Slot::Text(TextSection::Message),
                    block: 0,
                    ..
                },
            ) if id == previous => 4.0,
            (_, Row::Text { block: 1.., .. }) => 0.0,
            (
                Row::Work { .. }
                | Row::WorkEntry { .. }
                | Row::Chain { .. }
                | Row::EarlierWork { .. }
                | Row::Text {
                    slot: Slot::Text(_),
                    ..
                },
                Row::WorkEntry { .. }
                | Row::Chain { .. }
                | Row::EarlierWork { .. }
                | Row::Text {
                    slot: Slot::Text(_),
                    ..
                },
            ) if run == previous_run && !row.is_user() && !previous.is_user() => 8.0,
            (
                Row::Work { .. },
                Row::Text {
                    slot: Slot::Prose, ..
                },
            ) => REPLY_SEAM,
            (Row::Work { .. }, Row::Status(_)) => STATUS_AFTER_WORK,
            (_, Row::Status(_)) => STATUS_GAP,
            (
                _,
                Row::Text {
                    slot: Slot::Plain(PlainSection::User) | Slot::Text(TextSection::User),
                    ..
                },
            ) if run.is_some() && run == previous_run => BLOCK_GAP,
            (
                _,
                Row::Text {
                    slot: Slot::Plain(PlainSection::User) | Slot::Text(TextSection::User),
                    ..
                },
            ) => TURN_GAP,
            (
                Row::Text {
                    slot: Slot::Plain(PlainSection::User) | Slot::Text(TextSection::User),
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
        Row::Projected { .. }
        | Row::EarlierWork { .. }
        | Row::WorkEntry { .. }
        | Row::Text { .. }
        | Row::Prompt(_)
        | Row::Compaction(_)
        | Row::Image(_)
        | Row::Gap => row.anchor().into_iter().collect(),
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
#[path = "../../tests/conversation/fold.rs"]
mod tests;
