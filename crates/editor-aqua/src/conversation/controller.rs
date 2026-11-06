use super::disclosure::DisclosureState;
use super::fold::{self, Row, RowKey, RunSummary};
use super::thread::{self, ThreadView};
use super::viewport::{Effect, Fetch, Input, Jump};
use crate::core::{
    Core, CoreEvent, HostEvent, HostId, HostInstance, Screen, delivery::Delivery, runtime::runtime,
};
use artisan_api::{Answer, Item, ItemId, ItemKind, ItemsPage, RunId, Seq, ThreadId};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;
use tokio_util::task::AbortOnDropHandle;

pub enum ConversationEvent {
    ItemsChanged,
    AgentChanged,
    NewProject,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Origin {
    host: HostId,
    instance: HostInstance,
    generation: u64,
}

pub fn completion_is_current<I: Copy + Eq>(
    captured: (&HostId, I, u64),
    open: Option<(&HostId, I, u64)>,
    current: Option<I>,
) -> bool {
    let (host, instance, generation) = captured;
    matches!(open, Some((open_host, open_instance, open_generation)) if host == open_host && instance == open_instance && generation == open_generation)
        && current.as_ref() == Some(&instance)
}

enum Completion {
    Page {
        origin: Origin,
        fetch: Fetch,
        result: anyhow::Result<ItemsPage>,
    },
    Work {
        origin: Origin,
        first: Seq,
        result: anyhow::Result<ItemsPage>,
    },
    Agent {
        origin: Origin,
        context: HashMap<ItemId, Arc<Item>>,
        error: Option<String>,
        effects: Vec<Effect>,
    },
    Answer {
        origin: Origin,
        item: ItemId,
        result: anyhow::Result<()>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScrollAction {
    End,
    To(Seq),
}

pub struct Conversation {
    pub open: Option<(HostId, ThreadId)>,
    host_instance: Option<HostInstance>,
    pub agent: Option<ItemId>,
    pub view: Option<ThreadView>,
    pub rows: Vec<(Row, u64)>,
    rows_epoch: u64,
    row_positions: RefCell<(u64, usize, HashMap<RowKey, usize>)>,
    pub runs: HashMap<RunId, RunSummary>,
    pub disclosures: DisclosureState,
    pub question_pages: super::questions::QuestionPages,
    pub question_collection: super::questions::QuestionCollection,
    pub sources: super::content::SourceOwners,
    projections: super::projection::Projections,
    body_versions: HashMap<super::presentation::BodyKey, u64>,
    pub reveal: Rc<RefCell<super::admission::RevealQueue>>,
    pub reveal_frontier: Option<RowKey>,
    pub arrivals: Rc<RefCell<HashMap<ItemId, super::admission::LiveBlockArrivals>>>,
    visible_items: HashSet<ItemId>,
    mounted_rows: HashSet<RowKey>,
    mounted_for: Option<(u64, usize, usize)>,
    pub row_heights: HashMap<RowKey, f32>,
    pub load_error: Option<String>,
    pub landing: Option<Instant>,
    pub scroll_frame: Option<Instant>,
    generation: u64,
    work_reading: Option<Seq>,
    page_task: Option<AbortOnDropHandle<()>>,
    work_task: Option<AbortOnDropHandle<()>>,
    agent_task: Option<AbortOnDropHandle<()>>,
    answer_tasks: HashMap<ItemId, AbortOnDropHandle<()>>,
    delivery: Delivery<Completion>,
    events: Vec<ConversationEvent>,
    scroll: Vec<ScrollAction>,
}

fn next_rows_epoch() -> u64 {
    static EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    EPOCH.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl Conversation {
    pub fn rows_epoch(&self) -> u64 {
        self.rows_epoch
    }

    pub fn row_position(&self, key: &RowKey) -> Option<usize> {
        let mut positions = self.row_positions.borrow_mut();
        if positions.0 != self.rows_epoch || positions.1 != self.rows.len() {
            positions.0 = self.rows_epoch;
            positions.1 = self.rows.len();
            positions.2 = self
                .rows
                .iter()
                .enumerate()
                .map(|(index, (row, _))| (row.key(), index))
                .collect();
        }
        positions.2.get(key).copied()
    }

    #[cfg(feature = "qualification")]
    pub fn work_in_flight(&self) -> bool {
        self.work_reading.is_some() || self.work_task.is_some()
    }

    pub fn new(core: &Core) -> Self {
        Self {
            open: None,
            host_instance: None,
            agent: None,
            view: None,
            rows: Vec::new(),
            rows_epoch: next_rows_epoch(),
            row_positions: RefCell::new((0, 0, HashMap::new())),
            runs: HashMap::new(),
            disclosures: DisclosureState::default(),
            question_pages: super::questions::QuestionPages::default(),
            question_collection: super::questions::QuestionCollection::default(),
            sources: super::content::SourceOwners::default(),
            projections: super::projection::Projections::default(),
            body_versions: HashMap::new(),
            reveal: Rc::new(RefCell::new(super::admission::RevealQueue::default())),
            reveal_frontier: None,
            arrivals: Rc::new(RefCell::new(HashMap::new())),
            visible_items: HashSet::new(),
            mounted_rows: HashSet::new(),
            mounted_for: None,
            row_heights: HashMap::new(),
            load_error: None,
            landing: None,
            scroll_frame: None,
            generation: 0,
            work_reading: None,
            page_task: None,
            work_task: None,
            agent_task: None,
            answer_tasks: HashMap::new(),
            delivery: Delivery::with_wake(
                NonZeroUsize::new(64).unwrap(),
                core.wake_mailbox().sender(),
            ),
            events: Vec::new(),
            scroll: Vec::new(),
        }
    }

    pub fn sync_screen(&mut self, core: &mut Core) {
        match core.workspace_screen().clone() {
            Screen::Thread { host, thread } => {
                let current = core.host(&host).map(|host| host.instance());
                if current.is_none() {
                    if self.open.is_some() {
                        self.close(core);
                    }
                } else if self.open.as_ref() != Some(&(host.clone(), thread.clone()))
                    || self.host_instance != current
                {
                    self.open_thread(core, host, thread);
                }
            }
            Screen::NewThread { .. } => self.close(core),
            _ => {}
        }
    }

    pub fn open_thread(&mut self, core: &mut Core, host: HostId, thread: ThreadId) {
        self.close(core);
        let Some(host_data) = core.host_mut(&host) else {
            return;
        };
        let instance = host_data.instance();
        host_data.observe_thread(Some(thread.clone()));
        let (mut view, effects) = ThreadView::open(thread.clone());
        view.set_live(host_data.live_targets(&thread));
        self.open = Some((host, thread));
        self.host_instance = Some(instance);
        self.view = Some(view);
        self.load_error = None;
        self.landing = Some(Instant::now() + std::time::Duration::from_millis(1500));
        super::open_timing::begin();
        self.rebuild(core);
        self.perform(core, effects);
        self.events.push(ConversationEvent::ItemsChanged);
    }

    pub fn close(&mut self, core: &mut Core) {
        self.generation = self.generation.wrapping_add(1);
        self.page_task = None;
        self.work_task = None;
        self.agent_task = None;
        self.answer_tasks.clear();
        self.work_reading = None;
        self.landing = None;
        self.scroll_frame = None;
        self.scroll.clear();
        self.disclosures = DisclosureState::default();
        self.question_pages.clear();
        self.question_collection.clear();
        self.sources.clear();
        self.projections.clear();
        self.body_versions.clear();
        self.reveal = Rc::new(RefCell::new(super::admission::RevealQueue::default()));
        self.reveal_frontier = None;
        self.arrivals = Rc::new(RefCell::new(HashMap::new()));
        self.visible_items.clear();
        self.mounted_rows.clear();
        self.mounted_for = None;
        self.row_heights.clear();
        self.load_error = None;
        if self.agent.take().is_some() {
            self.events.push(ConversationEvent::AgentChanged);
        }
        if let Some((host, _)) = self.open.take() {
            if let Some(instance) = self.host_instance.take()
                && let Some(host) = core.host_mut_if_instance(&host, instance)
            {
                host.observe_thread(None);
            }
            self.view = None;
            self.rows.clear();
            self.runs.clear();
            self.events.push(ConversationEvent::ItemsChanged);
        }
    }

    pub fn sync_question_pages(
        &mut self,
        core: &Core,
        panel: &crate::composer::questionnaire::QuestionnairePanel,
    ) {
        self.question_pages.sync(
            core,
            self.open.as_ref().map(|(host, _)| host),
            self.host_instance,
            panel,
        );
    }

    pub fn items(&self) -> impl Iterator<Item = &Item> {
        self.view
            .iter()
            .flat_map(|view| view.items.values().map(Arc::as_ref))
    }

    pub fn selection_scope(&self) -> Option<String> {
        let (host, thread) = self.open.as_ref()?;
        Some(format!(
            "{host}/{thread}/{}",
            self.agent.as_ref().map_or("root", ItemId::as_str)
        ))
    }

    pub fn work_idle(&self) -> bool {
        self.work_reading.is_none()
    }

    pub fn landed(&mut self) {
        self.landing = None;
    }

    pub fn expire_landing(&mut self, now: Instant) -> bool {
        if self.landing.is_some_and(|deadline| now >= deadline) {
            self.landing = None;
            return true;
        }
        false
    }

    pub fn is_landing(&self) -> bool {
        self.agent.is_none()
            && self.load_error.is_none()
            && self.view.is_some()
            && self
                .landing
                .is_some_and(|deadline| Instant::now() < deadline)
    }

    pub fn events(&mut self) -> Vec<ConversationEvent> {
        std::mem::take(&mut self.events)
    }
    pub fn scroll_actions(&mut self) -> Vec<ScrollAction> {
        std::mem::take(&mut self.scroll)
    }

    pub fn viewport(&mut self, core: &Core, input: Input) {
        if let Some(view) = &mut self.view {
            let effects = view.handle(&input);
            self.perform(core, effects);
        }
    }

    pub fn jump(&mut self, core: &Core, jump: Jump) {
        if let Some(view) = &mut self.view {
            let effects = view.jump(jump);
            self.perform(core, effects);
        }
    }

    pub fn jump_row(&self, position: Seq) -> Option<usize> {
        let view = self.view.as_ref()?;
        let starts: Vec<_> = self
            .rows
            .iter()
            .map(|(row, _)| row.anchor().and_then(|id| view.position_of(id)))
            .collect();
        let after = starts
            .iter()
            .position(|start| start.is_some_and(|start| start >= position));
        match after {
            Some(index) if starts[index] == Some(position) => Some(index),
            _ => starts[..after.unwrap_or(starts.len())]
                .iter()
                .rposition(Option::is_some)
                .or(after),
        }
    }

    #[track_caller]
    fn rebuild(&mut self, core: &Core) {
        let caller_line = std::panic::Location::caller().line();
        let _span = crate::shell::rebuild_stats::span("conv_rebuild");
        let run = self
            .open
            .as_ref()
            .and_then(|(host, thread)| {
                core.host_if_instance(host, self.host_instance?)?
                    .runs
                    .get(thread)
            })
            .cloned();
        if let (Some(view), Some(run)) = (&mut self.view, &run) {
            view.remember_run(run.clone());
        }
        if let Some(view) = &self.view {
            self.arrivals
                .borrow_mut()
                .retain(|id, _| view.item(id).is_some());
            self.sources.retain(core, |id| view.shared_item(id));
            self.question_collection
                .retain(core, |id| view.shared_item(id));
            let fold_started = Instant::now();
            let folded = fold::fold(view, run.as_ref(), self.agent.as_ref());
            let fold_time = fold_started.elapsed();
            let expand_started = Instant::now();
            let expanded = fold::expanded(view, folded.rows, &folded.runs);
            let expand_time = expand_started.elapsed();
            let project_started = Instant::now();
            self.body_versions.clear();
            for (row, version) in &expanded {
                if let Row::Text { id, slot, .. } = row {
                    self.body_versions.insert(
                        super::presentation::BodyKey {
                            item: id.clone(),
                            slot: *slot,
                        },
                        *version,
                    );
                }
            }
            let projected = self.projections.expand(
                expanded,
                |id, slot| {
                    let item = view.item(id)?;
                    match slot {
                        fold::Slot::Plain(section) => section.inline_text(item),
                        _ => slot.text(item)?.inline(),
                    }
                    .map(str::to_owned)
                },
                |id, slot| shape_source(view, id, slot),
            );
            let rows = self.disclosures.retain_disclosure_rows(projected);
            if rows != self.rows {
                self.rows = rows;
                self.rows_epoch = next_rows_epoch();
            }
            self.disclosures.index_disclosure_rows(&self.rows);
            self.runs = folded.runs;
            super::open_timing::rebuilt(
                caller_line,
                fold_time,
                expand_time,
                project_started.elapsed(),
                self.rows.len(),
            );
        } else {
            if !self.rows.is_empty() {
                self.rows.clear();
                self.rows_epoch = next_rows_epoch();
            }
            self.runs.clear();
        }
    }

    pub fn observe_visible_rows(
        &mut self,
        range: std::ops::Range<usize>,
        mounted: std::ops::Range<usize>,
    ) {
        self.visible_items.clear();
        for (row, _) in self
            .rows
            .iter()
            .skip(range.start)
            .take(range.end.saturating_sub(range.start))
        {
            if let Some(id) = row.anchor() {
                self.visible_items.insert(id.clone());
            }
        }
        let mounted_for = (self.rows_epoch, mounted.start, mounted.end);
        if self.mounted_for != Some(mounted_for) {
            self.mounted_for = Some(mounted_for);
            self.mounted_rows = self
                .rows
                .iter()
                .skip(mounted.start)
                .take(mounted.end.saturating_sub(mounted.start))
                .map(|(row, _)| row.key())
                .collect();
        }
    }

    pub fn advance_disclosure_frame(
        &mut self,
        core: &Core,
        now: Instant,
        reduce_motion: bool,
    ) -> super::disclosure::DisclosureFrame {
        let frame = self.disclosures.advance_disclosures(
            &self.rows,
            &self.row_heights,
            self.view.as_ref(),
            now,
            reduce_motion,
        );
        if frame.refold {
            self.rebuild(core);
        }
        if !frame.changed.is_empty() {
            let changed: HashSet<_> = frame.changed.iter().collect();
            for (row, version) in &mut self.rows {
                if changed.contains(&row.key()) {
                    *version = version.wrapping_add(1);
                }
            }
            self.rows_epoch = next_rows_epoch();
        }
        for key in &frame.released {
            if !self.mounted_rows.contains(key)
                && !self.disclosures.disclosure_clips.contains_key(key)
            {
                self.row_heights.remove(key);
            }
        }
        frame
    }

    pub fn observe_live_preparation(
        &mut self,
        core: &Core,
        binding: &super::live::LiveBinding,
        blocks: usize,
    ) -> bool {
        if !binding.scope.current(self, core) {
            return false;
        }
        let Some(view) = self.view.as_ref() else {
            return false;
        };
        if view.item(&binding.item).is_none() {
            return false;
        }
        let fresh = view
            .ui
            .get(&binding.item)
            .is_some_and(thread::ItemUi::fresh);
        let caught_up = binding.current(self, core);
        let mut arrivals = self.arrivals.borrow_mut();
        if !arrivals.contains_key(&binding.item) && arrivals.len() >= 128 {
            return false;
        }
        let arrival = arrivals
            .entry(binding.item.clone())
            .or_insert_with(|| super::admission::LiveBlockArrivals::baseline(0));
        arrival.prepared(blocks.max(1), caught_up, fresh);
        for (row, _) in &self.rows {
            if self.mounted_rows.contains(&row.key())
                && let Row::Text {
                    id,
                    slot: fold::Slot::Prose,
                    block,
                    ..
                } = row.semantic()
                && *id == binding.item
            {
                arrival.claim(*block);
            }
        }
        true
    }

    pub fn claim_prose_arrival(&mut self, id: &ItemId, block: usize) -> bool {
        self.arrivals.borrow_mut().get_mut(id).map_or_else(
            || {
                self.view
                    .as_ref()
                    .and_then(|view| view.ui.get(id))
                    .is_some_and(thread::ItemUi::fresh)
            },
            |arrival| arrival.claim(block),
        )
    }

    fn observe_live_prose_arrival(&mut self, id: &ItemId) {
        if !self.visible_items.contains(id) {
            return;
        }
        let key = super::presentation::BodyKey {
            item: id.clone(),
            slot: fold::Slot::Prose,
        };
        let blocks = self.projections.leaf_count(&key).unwrap_or(0);
        let mut arrivals = self.arrivals.borrow_mut();
        if !arrivals.contains_key(id)
            && arrivals.len() >= 128
            && let Some(oldest) = arrivals
                .iter()
                .min_by_key(|(_, arrival)| arrival.seen)
                .map(|(id, _)| id.clone())
        {
            arrivals.remove(&oldest);
        }
        arrivals
            .entry(id.clone())
            .or_insert_with(|| super::admission::LiveBlockArrivals::baseline(blocks))
            .observed();
    }

    pub fn publish_projection(
        &mut self,
        core: &Core,
        scope: &super::presentation::PresentationScope,
        key: super::presentation::BodyKey,
        projection: super::projection::BodyProjection,
    ) -> bool {
        if !scope.current(self, core)
            || self.body_versions.get(&key) != Some(&projection.version)
            || projection.leaves.end < projection.leaves.start
        {
            return false;
        }
        if !self.projections.publish(key, projection) {
            return false;
        }
        self.rebuild(core);
        true
    }

    pub fn publish_projections(
        &mut self,
        core: &Core,
        scope: &super::presentation::PresentationScope,
        batch: impl IntoIterator<
            Item = (
                super::presentation::BodyKey,
                super::projection::BodyProjection,
            ),
        >,
    ) -> bool {
        let mut published = false;
        let mut bodies = 0;
        for (key, projection) in batch {
            if !scope.current(self, core)
                || self.body_versions.get(&key) != Some(&projection.version)
                || projection.leaves.end < projection.leaves.start
            {
                continue;
            }
            let accepted = self.projections.publish(key, projection);
            bodies += usize::from(accepted);
            published |= accepted;
        }
        if published {
            self.rebuild(core);
            super::open_timing::projections_published(bodies, self.rows.len());
        }
        published
    }

    pub fn body_shape_count(&self, key: &super::presentation::BodyKey) -> Option<usize> {
        self.projections
            .shaped(key)
            .then(|| self.projections.leaf_count(key))
            .flatten()
    }

    pub fn body_version(&self, key: &super::presentation::BodyKey) -> Option<u64> {
        self.body_versions.get(key).copied()
    }

    fn perform(&mut self, core: &Core, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::Fetch(fetch) => self.fetch(core, fetch),
                Effect::ScrollToEnd => self.scroll.push(ScrollAction::End),
                Effect::ScrollTo(position) => self.scroll.push(ScrollAction::To(position)),
            }
        }
    }

    fn fetch(&mut self, core: &Core, fetch: Fetch) {
        let Some((host, thread)) = &self.open else {
            return;
        };
        let Some(host) = self
            .host_instance
            .and_then(|instance| core.host_if_instance(host, instance))
        else {
            return;
        };
        let client = host.client.clone();
        let thread = thread.clone();
        let agent = self.agent.clone();
        let origin = Origin {
            host: host.id.clone(),
            instance: host.instance(),
            generation: self.generation,
        };
        let sender = self.delivery.sender();
        self.page_task = Some(AbortOnDropHandle::new(runtime().spawn(async move {
            let result = match agent {
                Some(agent) => {
                    client
                        .agent_items(&thread, &agent, &thread::query(fetch, thread::PAGE))
                        .await
                }
                None => {
                    client
                        .transcript(&thread, &thread::query(fetch, thread::ROWS_PAGE))
                        .await
                }
            };
            let _ = sender
                .send(Completion::Page {
                    origin,
                    fetch,
                    result,
                })
                .await;
        })));
    }

    pub fn load_work(&mut self, core: &Core, first: Seq) {
        if self.work_reading.is_some() || self.agent.is_some() {
            return;
        }
        let (Some((host, thread)), Some(view)) = (&self.open, &self.view) else {
            return;
        };
        let (Some(host), Some(span)) = (
            self.host_instance
                .and_then(|instance| core.host_if_instance(host, instance)),
            view.work.get(&first),
        ) else {
            return;
        };
        let client = host.client.clone();
        let thread = thread.clone();
        let query = thread::work_query(span);
        let origin = Origin {
            host: host.id.clone(),
            instance: host.instance(),
            generation: self.generation,
        };
        let sender = self.delivery.sender();
        self.work_reading = Some(first);
        self.work_task = Some(AbortOnDropHandle::new(runtime().spawn(async move {
            let result = client.work(&thread, &query).await;
            let _ = sender
                .send(Completion::Work {
                    origin,
                    first,
                    result,
                })
                .await;
        })));
    }

    pub(crate) fn work_reading(&self) -> Option<Seq> {
        self.work_reading
    }

    pub fn open_agent(&mut self, core: &Core, agent: Option<ItemId>) {
        if self.agent == agent {
            return;
        }
        self.generation = self.generation.wrapping_add(1);
        self.page_task = None;
        self.agent_task = None;
        self.work_task = None;
        self.work_reading = None;
        self.answer_tasks.clear();
        self.load_error = None;
        self.scroll_frame = None;
        self.disclosures = DisclosureState::default();
        self.question_pages.clear();
        self.question_collection.clear();
        self.sources.clear();
        self.projections.clear();
        self.body_versions.clear();
        self.reveal = Rc::new(RefCell::new(super::admission::RevealQueue::default()));
        self.reveal_frontier = None;
        self.arrivals = Rc::new(RefCell::new(HashMap::new()));
        self.visible_items.clear();
        self.mounted_rows.clear();
        self.mounted_for = None;
        self.row_heights.clear();
        let context = agent
            .as_ref()
            .and_then(|id| self.view.as_ref()?.shared_item(id));
        self.agent.clone_from(&agent);
        let effects = if let Some(view) = &mut self.view {
            view.context.clear();
            if let Some(item) = context {
                view.context.insert(item.id.clone(), item);
            }
            view.reset()
        } else {
            Vec::new()
        };
        self.rows.clear();
        self.runs.clear();
        if let (Some(agent), Some((host, thread))) = (agent, self.open.as_ref()) {
            if let Some(host) = self
                .host_instance
                .and_then(|instance| core.host_if_instance(host, instance))
            {
                let client = host.client.clone();
                let thread = thread.clone();
                let origin = Origin {
                    host: host.id.clone(),
                    instance: host.instance(),
                    generation: self.generation,
                };
                let sender = self.delivery.sender();
                self.agent_task = Some(AbortOnDropHandle::new(runtime().spawn(async move {
                    let mut context = HashMap::new();
                    let mut next = Some(agent);
                    let mut error = None;
                    for _ in 0..16 {
                        let Some(id) = next.take() else { break };
                        if context.contains_key(&id) {
                            error = Some("Agent ancestry contains a cycle".to_owned());
                            break;
                        }
                        match client.item(&id).await {
                            Ok(item)
                                if item.thread_id == thread
                                    && matches!(item.kind, ItemKind::Agent { .. }) =>
                            {
                                next.clone_from(&item.parent_id);
                                context.insert(id, Arc::new(item));
                            }
                            Ok(_) => {
                                error = Some("Agent does not belong to this thread".to_owned());
                                break;
                            }
                            Err(err) => {
                                error = Some(format!("Couldn't load agent: {err:#}"));
                                break;
                            }
                        }
                    }
                    if next.is_some() {
                        error = Some("Agent ancestry exceeds 16 levels".to_owned());
                    }
                    let _ = sender
                        .send(Completion::Agent {
                            origin,
                            context,
                            error,
                            effects,
                        })
                        .await;
                })));
            }
        } else {
            self.perform(core, effects);
        }
        self.rebuild(core);
        self.scroll.push(ScrollAction::End);
        self.events.push(ConversationEvent::AgentChanged);
        self.events.push(ConversationEvent::ItemsChanged);
    }

    pub fn agent_context(&self) -> Option<Option<(u64, Option<String>)>> {
        let agent = self.agent.as_ref()?;
        let item = self.view.as_ref()?.item(agent);
        Some(item.and_then(|item| match &item.kind {
            ItemKind::Agent {
                context_used: Some(used),
                model,
                ..
            } => Some((*used, model.clone())),
            _ => None,
        }))
    }

    pub fn agent_trail(&self) -> Vec<(ItemId, String)> {
        let (Some(view), Some(agent)) = (&self.view, &self.agent) else {
            return Vec::new();
        };
        let mut trail = Vec::new();
        let mut next = Some(agent);
        while let Some(id) = next.take() {
            let Some(item) = view.item(id) else { break };
            if let Some(label) = item.agent_label() {
                trail.push((id.clone(), label));
            }
            next = item.parent_id.as_ref();
            if trail.len() > 16 {
                break;
            }
        }
        trail.reverse();
        trail
    }

    pub fn toggle_work(
        &mut self,
        core: &Core,
        run: &RunId,
        open: bool,
        reduce_motion: bool,
        now: Instant,
    ) {
        if !reduce_motion {
            let headers: Vec<_> = self
                .rows
                .iter()
                .filter_map(|(row, _)| match row {
                    Row::Work {
                        run: Some(row_run), ..
                    } if row_run == run => Some(row.key()),
                    _ => None,
                })
                .collect();
            for header in headers {
                self.disclosures
                    .start_disclosure(&self.rows, &self.row_heights, header, open, now);
            }
        }
        if let Some(view) = &mut self.view {
            view.work_open.insert(run.clone(), open);
            self.rebuild(core);
        }
    }

    pub fn toggle_chain(
        &mut self,
        core: &Core,
        id: &ItemId,
        open: bool,
        reduce_motion: bool,
        now: Instant,
    ) {
        if !reduce_motion {
            self.disclosures.start_disclosure(
                &self.rows,
                &self.row_heights,
                RowKey::Chain(id.clone()),
                open,
                now,
            );
        }
        if let Some(view) = &mut self.view {
            view.ui.entry(id.clone()).or_default().chain_open = Some(open);
            self.rebuild(core);
        }
    }

    pub fn toggle_item(&mut self, core: &Core, id: &ItemId, open: bool) {
        if let Some(view) = &mut self.view {
            view.ui.entry(id.clone()).or_default().expanded = Some(open);
            self.rebuild(core);
        }
    }

    pub fn answer(&mut self, core: &Core, item: ItemId, answer: Answer) {
        let Some((host, _)) = &self.open else { return };
        let Some(host) = self
            .host_instance
            .and_then(|instance| core.host_if_instance(host, instance))
        else {
            return;
        };
        let Some(view) = &mut self.view else { return };
        let ui = view.ui.entry(item.clone()).or_default();
        if ui.sending {
            return;
        }
        ui.sending = true;
        ui.error = None;
        let client = host.client.clone();
        let origin = Origin {
            host: host.id.clone(),
            instance: host.instance(),
            generation: self.generation,
        };
        let sender = self.delivery.sender();
        let id = item.clone();
        self.answer_tasks.insert(
            item,
            AbortOnDropHandle::new(runtime().spawn(async move {
                let result = client.answer(&id, &answer).await;
                let _ = sender
                    .send(Completion::Answer {
                        origin,
                        item: id,
                        result,
                    })
                    .await;
            })),
        );
        self.rebuild(core);
    }

    pub fn core_event(&mut self, core: &mut Core, event: &CoreEvent, watched: bool) {
        match event {
            CoreEvent::ScreenChanged | CoreEvent::HostsChanged => self.sync_screen(core),
            CoreEvent::Host { host, event }
                if self.open.as_ref().is_some_and(|(open, _)| open == host) =>
            {
                self.host_event(core, event, watched)
            }
            _ => {}
        }
    }

    fn host_event(&mut self, core: &Core, event: &HostEvent, watched: bool) {
        let Some((host_id, thread)) = &self.open else {
            return;
        };
        let Some(host) = self
            .host_instance
            .and_then(|instance| core.host_if_instance(host_id, instance))
        else {
            return;
        };
        let Some(view) = &mut self.view else { return };
        let thread = thread.clone();
        match event {
            HostEvent::Item(item) => {
                let directed = matches!(&item.kind, ItemKind::Tool { tool: artisan_api::ToolKind::AgentMessage { target: Some(target), direction: artisan_api::MessageDirection::Sent, .. }, .. } if self.agent.as_ref() == Some(target));
                if item.thread_id != thread
                    || (item.parent_id != self.agent
                        && !view.context.contains_key(&item.id)
                        && !directed)
                {
                    return;
                }
                let changed = view.apply_item(Arc::clone(item), watched && !self.landing.is_some());
                view.set_live(host.live_targets(&thread));
                let sent = matches!(
                    item.kind,
                    ItemKind::UserMessage {
                        steer: false,
                        from_agent: None,
                        ..
                    }
                ) && view.ui.get(&item.id).is_some_and(thread::ItemUi::fresh);
                if changed {
                    if watched && matches!(item.kind, ItemKind::AssistantMessage { .. }) {
                        self.observe_live_prose_arrival(&item.id);
                    }
                    if watched && self.landing.is_none() {
                        self.scroll_frame.get_or_insert_with(Instant::now);
                    }
                    if sent {
                        self.reveal.borrow_mut().rush(item.position);
                    }
                    self.rebuild(core);
                    self.events.push(ConversationEvent::ItemsChanged);
                }
            }
            HostEvent::Live { thread_id, target } if thread_id == &thread => {
                let text = host
                    .live_snapshot(&thread, target)
                    .map(|snapshot| snapshot.text);
                if view.update_live(target, text) {
                    if watched && let artisan_api::LiveTarget::Text { item_id } = target {
                        self.observe_live_prose_arrival(item_id);
                    }
                    if watched && self.landing.is_none() {
                        self.scroll_frame.get_or_insert_with(Instant::now);
                    }
                    self.rebuild(core);
                }
            }
            HostEvent::ThreadChanged { thread_id } if thread_id == &thread => {
                view.set_live(host.live_targets(&thread));
                self.rebuild(core);
            }
            HostEvent::Reset => {
                self.page_task = None;
                self.work_task = None;
                self.work_reading = None;
                let effects = view.reset();
                view.set_live(host.live_targets(&thread));
                self.perform(core, effects);
                self.rebuild(core);
                self.events.push(ConversationEvent::ItemsChanged);
            }
            _ => {}
        }
    }

    fn accepts(&self, core: &Core, origin: &Origin) -> bool {
        let open = self.open.as_ref().and_then(|(host, _)| {
            self.host_instance
                .map(|instance| (host, instance, self.generation))
        });
        let current = core.host(&origin.host).map(|host| host.instance());
        completion_is_current(
            (&origin.host, origin.instance, origin.generation),
            open,
            current,
        )
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn host_instance(&self) -> Option<HostInstance> {
        self.host_instance
    }

    pub fn host_is_current(&self, core: &Core) -> bool {
        self.open
            .as_ref()
            .zip(self.host_instance)
            .is_some_and(|((host, _), instance)| core.host_if_instance(host, instance).is_some())
    }

    #[cfg(feature = "test-support")]
    pub fn page_task_finished(&self) -> bool {
        self.page_task
            .as_ref()
            .is_some_and(|task| task.is_finished())
    }

    pub fn drain(&mut self, core: &Core) -> bool {
        let mut changed = false;
        for completion in self.delivery.drain() {
            match completion {
                Completion::Page {
                    origin,
                    fetch,
                    result,
                } if self.accepts(core, &origin) => {
                    self.page_task = None;
                    let Some(view) = &mut self.view else { continue };
                    if view.viewport.requested != Some(fetch) {
                        continue;
                    }
                    changed = true;
                    let mut applied = None;
                    let effects = match result {
                        Ok(page) => {
                            self.load_error = None;
                            let (items, spans) = (page.items.len(), page.work.len());
                            let apply_started = Instant::now();
                            let effects = view.apply_page(fetch, page);
                            applied = Some((items, spans, apply_started.elapsed()));
                            effects
                        }
                        Err(error) => {
                            self.load_error = Some(format!("Couldn't load this thread: {error:#}"));
                            view.page_failed()
                        }
                    };
                    let rebuild_started = Instant::now();
                    self.rebuild(core);
                    if let Some((items, spans, apply)) = applied {
                        super::open_timing::page_applied(
                            items,
                            spans,
                            apply,
                            rebuild_started.elapsed(),
                            self.rows.len(),
                        );
                    }
                    self.perform(core, effects);
                    self.events.push(ConversationEvent::ItemsChanged);
                }
                Completion::Work {
                    origin,
                    first,
                    result,
                } if self.accepts(core, &origin) && self.work_reading == Some(first) => {
                    changed = true;
                    self.work_task = None;
                    self.work_reading = None;
                    let Some(view) = &mut self.view else { continue };
                    match result {
                        Ok(page) => {
                            self.load_error = None;
                            if view.apply_work(first, page) {
                                self.rebuild(core);
                                self.events.push(ConversationEvent::ItemsChanged);
                            }
                        }
                        Err(error) => {
                            self.load_error = Some(format!("Couldn't load this work: {error:#}"))
                        }
                    }
                }
                Completion::Agent {
                    origin,
                    context,
                    error,
                    effects,
                } if self.accepts(core, &origin) => {
                    self.agent_task = None;
                    if let Some(view) = &mut self.view {
                        for (id, item) in context {
                            if view.context.get(&id).is_none_or(|old| old.seq < item.seq) {
                                view.context.insert(id, item);
                                changed = true;
                            }
                        }
                    }
                    if error.is_some() {
                        changed |= self.load_error != error;
                        self.load_error = error;
                    }
                    self.rebuild(core);
                    if self.load_error.is_none() {
                        self.perform(core, effects);
                    }
                    self.events.push(ConversationEvent::AgentChanged);
                }
                Completion::Answer {
                    origin,
                    item,
                    result,
                } if self.accepts(core, &origin) => {
                    self.answer_tasks.remove(&item);
                    if let Some(ui) = self.view.as_mut().and_then(|view| view.ui.get_mut(&item)) {
                        changed |= ui.sending;
                        ui.sending = false;
                        if let Err(error) = result {
                            let error = Some(format!("Couldn't send the answer: {error:#}"));
                            changed |= ui.error != error;
                            ui.error = error;
                        }
                    }
                    self.rebuild(core);
                }
                _ => {}
            }
        }
        changed
    }
}

fn shape_source<'a>(
    view: &'a ThreadView,
    id: &ItemId,
    slot: fold::Slot,
) -> Option<super::projection::ShapeSource<'a>> {
    use super::projection::{ShapeSource, ShapeText};
    let item = view.item(id)?;
    let seq = match slot {
        fold::Slot::Text(fold::TextSection::User) => {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            item.seq.hash(&mut hasher);
            hasher.finish()
        }
        _ => 0,
    };
    match (slot, &item.kind) {
        (fold::Slot::Prose, artisan_api::ItemKind::AssistantMessage { text, .. }) => {
            let live = (item.status == artisan_api::Status::Running)
                .then(|| {
                    view.live.get(&artisan_api::LiveTarget::Text {
                        item_id: id.clone(),
                    })
                })
                .flatten();
            match (live, text) {
                (Some(rope), _) => Some(ShapeSource {
                    text: ShapeText::Live(rope),
                    message: false,
                    seq,
                    running: item.status == artisan_api::Status::Running,
                }),
                (None, artisan_api::TextContent::Inline(source)) => Some(ShapeSource {
                    text: ShapeText::Inline(source),
                    message: false,
                    seq,
                    running: item.status == artisan_api::Status::Running,
                }),
                (None, _) => None,
            }
        }
        (
            fold::Slot::Text(fold::TextSection::User),
            artisan_api::ItemKind::UserMessage {
                text: artisan_api::TextContent::Inline(source),
                ..
            },
        ) => Some(ShapeSource {
            text: ShapeText::Inline(source),
            message: true,
            seq,
            running: false,
        }),
        _ => None,
    }
}
