use super::controller::{Conversation, ScrollAction};
use super::fold::{self, Row, RowKey, RunSummary};
use super::thread::ItemUi;
use super::viewport::Input;
use editor_core::Core;
use artisan_api::{Answer, Item, ItemId, LiveTarget, RunId, Seq};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

#[derive(Clone)]
pub enum Action {
    OpenAgent(ItemId),
    ToggleWork { run: RunId, open: bool },
    ToggleChain { id: ItemId, open: bool },
    ToggleItem { id: ItemId, expanded: bool },
    Answer { id: ItemId, answer: Answer },
    Jump(Seq),
    JumpLatest,
    LoadNewer,
    LoadEarlierWork(Seq),
}

#[derive(Clone)]
pub struct ActionScope {
    scope: String,
    instance: editor_core::HostInstance,
}

impl ActionScope {
    pub fn new(conversation: &Conversation) -> Option<Self> {
        Some(Self {
            scope: conversation.selection_scope()?,
            instance: conversation.host_instance()?,
        })
    }

    pub fn bind(&self, action: Action) -> ScopedAction {
        ScopedAction {
            scope: self.scope.clone(),
            instance: self.instance,
            action,
        }
    }
}

pub struct ScopedAction {
    scope: String,
    instance: editor_core::HostInstance,
    action: Action,
}

impl ScopedAction {
    pub fn new(conversation: &Conversation, action: Action) -> Option<Self> {
        Some(ActionScope::new(conversation)?.bind(action))
    }

    pub fn dispatch(
        self,
        conversation: &mut Conversation,
        core: &Core,
        reduce_motion: bool,
        now: Instant,
    ) -> bool {
        if conversation.selection_scope().as_deref() != Some(self.scope.as_str())
            || conversation.host_instance() != Some(self.instance)
            || !conversation.host_is_current(core)
        {
            return false;
        }
        match self.action {
            Action::OpenAgent(id) => {
                let before = conversation.agent.clone();
                conversation.open_agent(core, Some(id));
                before != conversation.agent
            }
            Action::ToggleWork { run, open } => {
                let before = conversation.runs.get(&run).map(|run| run.open);
                conversation.toggle_work(core, &run, open, reduce_motion, now);
                before != conversation.runs.get(&run).map(|run| run.open)
            }
            Action::ToggleChain { id, open } => {
                let before = conversation
                    .view
                    .as_ref()
                    .and_then(|view| view.ui.get(&id))
                    .and_then(|ui| ui.chain_open)
                    .unwrap_or(false);
                conversation.toggle_chain(core, &id, open, reduce_motion, now);
                let after = conversation
                    .view
                    .as_ref()
                    .and_then(|view| view.ui.get(&id))
                    .and_then(|ui| ui.chain_open)
                    .unwrap_or(false);
                before != after
            }
            Action::ToggleItem { id, expanded } => {
                let before = conversation
                    .view
                    .as_ref()
                    .and_then(|view| view.ui.get(&id))
                    .and_then(|ui| ui.expanded)
                    .unwrap_or(false);
                conversation.toggle_item(core, &id, expanded);
                let after = conversation
                    .view
                    .as_ref()
                    .and_then(|view| view.ui.get(&id))
                    .and_then(|ui| ui.expanded)
                    .unwrap_or(false);
                before != after
            }
            Action::Answer { id, answer } => {
                let before = conversation
                    .view
                    .as_ref()
                    .and_then(|view| view.ui.get(&id))
                    .is_some_and(|ui| ui.sending);
                conversation.answer(core, id.clone(), answer);
                let after = conversation
                    .view
                    .as_ref()
                    .and_then(|view| view.ui.get(&id))
                    .is_some_and(|ui| ui.sending);
                before != after
            }
            Action::Jump(position) => {
                let before = conversation.view.as_ref().map(|view| view.viewport.clone());
                conversation.jump(core, crate::conversation::viewport::Jump::To(position));
                before != conversation.view.as_ref().map(|view| view.viewport.clone())
            }
            Action::JumpLatest => {
                let before = conversation.view.as_ref().map(|view| view.viewport.clone());
                conversation.jump(core, crate::conversation::viewport::Jump::Latest);
                before != conversation.view.as_ref().map(|view| view.viewport.clone())
            }
            Action::LoadNewer => {
                let before = conversation.view.as_ref().map(|view| view.viewport.clone());
                conversation.viewport(core, Input::BottomReached);
                before != conversation.view.as_ref().map(|view| view.viewport.clone())
            }
            Action::LoadEarlierWork(first) => {
                let before = conversation.work_reading();
                conversation.load_work(core, first);
                before != conversation.work_reading()
            }
        }
    }
}

pub struct RowModel {
    pub row: Row,
    pub version: u64,
    pub items: HashMap<ItemId, Arc<Item>>,
    pub live: HashMap<LiveTarget, Arc<ropey::Rope>>,
    pub ui: HashMap<ItemId, ItemUi>,
    pub run: Option<RunSummary>,
    pub author: Option<artisan_api::EngineId>,
    pub gap: f32,
    pub end: f32,
}

pub struct RowSnapshots {
    view: super::thread::ThreadView,
    rows: Vec<(Row, u64)>,
    runs: HashMap<RunId, RunSummary>,
    agent: bool,
    composer_height: Option<f32>,
}

impl RowSnapshots {
    pub fn capture(conversation: &Conversation, composer_height: Option<f32>) -> Option<Self> {
        Some(Self {
            view: conversation.view.as_ref()?.clone(),
            rows: conversation.rows.clone(),
            runs: conversation.runs.clone(),
            agent: conversation.agent.is_some(),
            composer_height,
        })
    }

    pub fn model(&self, index: usize) -> Option<RowModel> {
        row_model_from(
            &self.view,
            &self.rows,
            &self.runs,
            self.agent,
            self.composer_height,
            index,
        )
    }
}

pub fn row_model(
    conversation: &Conversation,
    index: usize,
    composer_height: Option<f32>,
) -> Option<RowModel> {
    let view = conversation.view.as_ref()?;
    row_model_from(
        view,
        &conversation.rows,
        &conversation.runs,
        conversation.agent.is_some(),
        composer_height,
        index,
    )
}

fn row_model_from(
    view: &super::thread::ThreadView,
    rows: &[(Row, u64)],
    runs: &HashMap<RunId, RunSummary>,
    agent: bool,
    composer_height: Option<f32>,
    index: usize,
) -> Option<RowModel> {
    let (row, version) = rows.get(index)?;
    let ids: Vec<ItemId> = match row {
        Row::Work { .. } => Vec::new(),
        Row::Chain { ids, .. } => ids.clone(),
        other => other.anchor().cloned().into_iter().collect(),
    };
    let items: HashMap<ItemId, Arc<Item>> = ids
        .iter()
        .filter_map(|id| Some((id.clone(), view.shared_item(id)?)))
        .collect();
    let ui = ids
        .iter()
        .filter_map(|id| Some((id.clone(), view.ui.get(id)?.clone())))
        .collect();
    let run = row.run().and_then(|run| runs.get(run)).cloned();
    let author = row
        .anchor()
        .and_then(|id| items.get(id))
        .and_then(|item| item.run_id.as_ref())
        .and_then(|run| view.runs.get(run))
        .map(|run| run.engine);
    let gap = fold::gap(view, rows, index);
    let end = if index + 1 == rows.len() {
        if agent {
            24.0
        } else {
            composer_height.map_or(24.0, |height| 192.0_f32.max(height + 16.0 + 24.0))
        }
    } else {
        0.0
    };
    let live = view
        .live
        .iter()
        .filter(|(target, _)| match target {
            LiveTarget::Text { item_id }
            | LiveTarget::Shell { item_id }
            | LiveTarget::Progress { item_id } => items.contains_key(item_id),
            LiveTarget::Reasoning { run_id } | LiveTarget::Status { run_id } => {
                row.run() == Some(run_id)
            }
        })
        .map(|(target, text)| (target.clone(), Arc::clone(text)))
        .collect();
    Some(RowModel {
        row: row.clone(),
        version: *version,
        items,
        live,
        ui,
        run,
        author,
        gap,
        end,
    })
}

pub enum ScrollDirective {
    FollowEnd,
    EaseToEnd,
    ToRow { index: usize, animate: bool },
}

pub fn scroll_directive(
    conversation: &Conversation,
    action: ScrollAction,
    reduce_motion: bool,
) -> Option<ScrollDirective> {
    let animate = !reduce_motion && !conversation.is_landing();
    match action {
        ScrollAction::End if animate => Some(ScrollDirective::EaseToEnd),
        ScrollAction::End => Some(ScrollDirective::FollowEnd),
        ScrollAction::To(position) => Some(ScrollDirective::ToRow {
            index: conversation.jump_row(position)?,
            animate,
        }),
        ScrollAction::Row(index) => Some(ScrollDirective::ToRow { index, animate }),
    }
}

pub fn changed_disclosures(
    conversation: &mut Conversation,
    now: Instant,
    reduce_motion: bool,
) -> super::disclosure::DisclosureFrame {
    conversation.disclosures.advance_disclosures(
        &conversation.rows,
        &conversation.row_heights,
        conversation.view.as_ref(),
        now,
        reduce_motion,
    )
}

pub fn row_key(model: &RowModel) -> RowKey {
    model.row.key()
}

pub fn sync_questionnaires(
    conversation: &Conversation,
    core: &Core,
    composer: &mut editor_composer::controller::Composer,
) {
    if !conversation.host_is_current(core) {
        return;
    }
    let Some((host_id, thread)) = conversation.open.as_ref() else {
        return;
    };
    if composer.target
        != (editor_composer::controller::ComposerTarget::Thread {
            host: host_id.clone(),
            thread: thread.clone(),
        })
    {
        return;
    }
    let Some(summary) = core
        .host(host_id)
        .and_then(|host| host.thread_summary(thread))
        .and_then(|summary| summary.value.as_ref())
    else {
        return;
    };
    let questionnaires = summary
        .questions
        .iter()
        .map(
            |question| editor_composer::questionnaire::ComposerQuestionnaire {
                item: question.id.clone(),
                questions: question.questions.clone(),
            },
        )
        .collect();
    let mut text = composer.editor.borrow().text().to_owned();
    composer
        .questionnaire
        .set_questionnaires(questionnaires, &mut text);
    if composer.editor.borrow().text() != text {
        composer.editor.borrow_mut().set_text(&text);
    }
}

pub fn composer_goal(
    conversation: &Conversation,
    core: &Core,
) -> Option<Option<editor_composer::goal_strip::ComposerGoal>> {
    use editor_composer::goal_strip::{ComposerGoal, ComposerGoalState};
    use artisan_api::ThreadGoalState;
    if !conversation.host_is_current(core) {
        return None;
    }
    let (host, thread) = conversation.open.as_ref()?;
    let summary = core.host(host)?.thread_summary(thread)?.value.as_ref()?;
    Some(summary.goal.as_ref().map(|goal| ComposerGoal {
        objective: goal.summary.clone(),
        state: match goal.state {
            ThreadGoalState::Active => ComposerGoalState::Active,
            ThreadGoalState::Paused => ComposerGoalState::Paused,
            ThreadGoalState::Blocked => ComposerGoalState::Blocked,
            ThreadGoalState::UsageLimited => ComposerGoalState::UsageLimited,
            ThreadGoalState::BudgetLimited => ComposerGoalState::BudgetLimited,
        },
        detail: None,
    }))
}

pub fn read_boundary(conversation: &Conversation, core: &Core) -> Option<(ItemId, Seq)> {
    if !conversation.host_is_current(core) {
        return None;
    }
    let (host, thread) = conversation.open.as_ref()?;
    let view = conversation.view.as_ref()?;
    if conversation.agent.is_some() || view.viewport.gap.is_some() {
        return None;
    }
    let rowed = || view.shown().filter(|item| !fold::in_composer_strip(item));
    let last = rowed().max_by_key(|item| item.position)?;
    let newest = rowed().map(|item| item.seq).max()?;
    let run = core.host(host)?.runs.get(thread)?;
    let ends_run = last.run_id.as_ref() == Some(&run.id)
        || rowed().all(|item| item.run_id.as_ref() != Some(&run.id));
    (!run.status.is_live() && ends_run && run.seq >= newest).then(|| (last.id.clone(), run.seq))
}

pub fn restore_anchor(
    conversation: &Conversation,
    anchors: &[(RowKey, Option<ItemId>, f32)],
) -> Option<(usize, f32)> {
    anchors.iter().find_map(|(key, item, offset)| {
        conversation
            .rows
            .iter()
            .position(|(row, _)| row.key() == *key && row.anchor() == item.as_ref())
            .map(|index| (index, *offset))
    })
}

pub fn agent_names(conversation: &Conversation) -> HashMap<ItemId, String> {
    let Some(view) = conversation.view.as_ref() else {
        return HashMap::new();
    };
    view.items
        .values()
        .chain(view.context.values())
        .filter_map(|item| {
            let artisan_api::ItemKind::Agent { description, .. } = &item.kind else {
                return None;
            };
            let task = description
                .inline()
                .and_then(|text| text.lines().map(str::trim).find(|line| !line.is_empty()))
                .map(str::to_owned)
                .or_else(|| item.agent_label())?;
            Some((item.id.clone(), task))
        })
        .collect()
}
