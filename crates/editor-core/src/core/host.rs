mod claude_ai;
mod database_rollback;
mod images;
mod pages;
pub use images::{BlobImage, BlobImages};
mod preferences;
mod project_icons;
mod settings;
pub use settings::{
    DEFAULT_ENGINE_CONFIG_KEY, HostSettingError, HostSettings, MCP_SERVERS_KEY, McpServersError,
};
pub mod stopping;
mod wsl;
pub use database_rollback::RollbackNotice;
pub use pages::{LoadThreadError, ThreadPage, ThreadPageDirection, ThreadUnavailable};
pub use wsl::WslStatus;

use crate::core::{
    ForgeClient,
    connection::{self, ConnectionStatus, Update},
    delivery::{AdmissionOrder, Delivery},
    hosts::HostId,
};
use artisan_api::{
    Account, EngineId, EngineStatus, Event, Item, ItemId, LiveCoverage, LiveTarget, Project,
    ProjectId, Run, Status, Thread, ThreadId,
};
use ropey::Rope;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    num::NonZeroUsize,
    rc::{Rc, Weak},
    sync::Arc,
};
use tokio::sync::{Notify, watch};
use tokio_util::task::AbortOnDropHandle;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HostInstance(u64);

#[derive(Debug, Clone)]
pub struct ScopedSourceProvider {
    host: HostId,
    instance: HostInstance,
    client: ForgeClient,
    authority: Weak<()>,
}

impl ScopedSourceProvider {
    pub fn client(&self) -> Option<ForgeClient> {
        let _authority = self.authority.upgrade()?;
        Some(self.client.clone())
    }

    pub fn host(&self) -> &HostId {
        &self.host
    }
    pub fn instance(&self) -> HostInstance {
        self.instance
    }
    pub fn is_available(&self) -> bool {
        self.authority.strong_count() != 0
    }
    pub fn index_range(
        &self,
        source: artisan_api::ContentRef,
        format: artisan_api::SourceIndexFormat,
        offset: Option<u64>,
        limit: u32,
    ) -> Option<
        futures::future::BoxFuture<
            'static,
            Result<artisan_api::SourceIndexRange, crate::core::client::RangeError>,
        >,
    > {
        let _authority = self.authority.upgrade()?;
        let client = self.client.clone();
        Some(Box::pin(async move {
            client
                .source_index_range(&source, format, offset, limit)
                .await
        }))
    }
    pub fn content_range(
        &self,
        source: artisan_api::ContentRef,
        offset: u64,
        limit: u32,
    ) -> Option<
        futures::future::BoxFuture<
            'static,
            Result<artisan_api::ContentRange, crate::core::client::RangeError>,
        >,
    > {
        let _authority = self.authority.upgrade()?;
        let client = self.client.clone();
        Some(Box::pin(async move {
            client.content_range(&source, offset, limit).await
        }))
    }
}

static NEXT_HOST_INSTANCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

#[derive(Debug, Clone)]
pub struct LiveSnapshot {
    pub epoch: u64,
    pub text: Arc<Rope>,
    pub coverage: LiveCoverage,
}

#[derive(Clone, Debug)]
pub enum HostEvent {
    Reset,
    ImagesChanged,
    EngineInstall {
        engine: EngineId,
    },
    ThreadSummaryChanged {
        thread_id: ThreadId,
    },
    Changed,
    WslDistro {
        distro: String,
    },
    ThreadChanged {
        thread_id: ThreadId,
    },
    Item(Arc<Item>),
    Live {
        thread_id: ThreadId,
        target: LiveTarget,
    },
    EditorCall {
        id: String,
        call: artisan_api::EditorCall,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Attention {
    pub waiting: usize,
    pub stopped: usize,
}
impl std::ops::Add for Attention {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self {
            waiting: self.waiting + rhs.waiting,
            stopped: self.stopped + rhs.stopped,
        }
    }
}

#[derive(Debug, Default)]
pub struct ThreadSummary {
    pub value: Option<Arc<artisan_api::InspectorSummary>>,
    pub loading: bool,
    pub error: Option<String>,
}

#[derive(Debug, Default)]
struct SummaryLoad {
    view: ThreadSummary,
    minimum_revision: artisan_api::Seq,
    dirty: bool,
    task: Option<AbortOnDropHandle<()>>,
    generation: u64,
}

struct ObservedSummary {
    thread: ThreadId,
    state: SummaryLoad,
}

struct RepositoryEntry {
    value: Option<artisan_api::ProjectRepository>,
    checked: std::time::Instant,
    failed: bool,
}

#[derive(Debug, Default)]
pub struct ProjectionLoad {
    pub loading: bool,
    pub error: Option<String>,
}

#[derive(Debug, Default)]
struct Projection {
    view: ProjectionLoad,
    _task: Option<AbortOnDropHandle<()>>,
    generation: u64,
}

impl Projection {
    fn loading(generation: u64, task: Option<AbortOnDropHandle<()>>) -> Self {
        Self {
            view: ProjectionLoad {
                loading: true,
                error: None,
            },
            _task: task,
            generation,
        }
    }
}

pub(super) enum HostMessage {
    Update(Update),
    Completion(Box<HostCompletion>),
}

impl HostMessage {
    fn completion(completion: HostCompletion) -> Self {
        Self::Completion(Box::new(completion))
    }
}

pub(super) enum HostCompletion {
    DecodedImage(
        artisan_api::BlobHash,
        u64,
        Result<Arc<aqua::images::ImageHandle>, String>,
    ),
    ProjectIcon(
        ProjectId,
        u64,
        Result<Option<artisan_api::BlobHash>, crate::core::client::CallError>,
    ),
    Image(
        artisan_api::BlobHash,
        u64,
        Result<Vec<u8>, crate::core::client::CallError>,
    ),
    Preference(
        u64,
        preferences::Choice,
        preferences::Preference,
        Result<(), crate::core::client::CallError>,
    ),
    WslProbe(
        u64,
        wsl::WslTarget,
        Result<Vec<crate::core::hosts::WslDistro>, crate::core::hosts::WslStateError>,
    ),
    WslStarted(String, Result<(), crate::core::hosts::StartWslForgeError>),
    RollbackDismissed(Result<(), crate::core::client::CallError>),
    ReadAck(
        ThreadId,
        artisan_api::Seq,
        u64,
        Result<Thread, crate::core::client::CallError>,
    ),
    ThreadPage {
        archived: bool,
        direction: ThreadPageDirection,
        started_seq: artisan_api::Seq,
        generation: u64,
        result: Result<artisan_api::ThreadsPage, crate::core::client::CallError>,
    },
    ThreadLookup(
        ThreadId,
        u64,
        Result<Thread, crate::core::client::CallError>,
    ),
    Summary(
        ThreadId,
        u64,
        artisan_api::Seq,
        Result<artisan_api::InspectorSummary, crate::core::client::CallError>,
    ),
    Prompt(
        u64,
        Result<artisan_api::GlobalPrompt, crate::core::client::CallError>,
    ),
    Catalog(
        u64,
        Result<Vec<artisan_api::CatalogSkill>, crate::core::client::CallError>,
    ),
    Repository(
        ProjectId,
        u64,
        Result<Option<artisan_api::ProjectRepository>, crate::core::client::CallError>,
    ),
    ClaudeAiCaptured(
        claude_ai_browser::Ticket,
        Result<artisan_api::ClaudeAiSession, claude_ai_browser::RenewError>,
    ),
    ClaudeAiHanded(
        claude_ai_browser::Ticket,
        Result<(), crate::core::client::CallError>,
    ),
}

#[derive(Clone)]
struct HostEffects {
    queue: crate::core::store::Effects,
    id: HostId,
    instance: HostInstance,
}
impl HostEffects {
    fn push(&self, event: HostEvent) {
        self.queue.host(&self.id, self.instance, event);
    }
}

pub struct HostData {
    instance: HostInstance,
    source_authority: RefCell<Rc<()>>,
    pub(crate) reported_live: bool,
    pub id: HostId,
    pub name: String,
    pub client: ForgeClient,
    pub status: ConnectionStatus,
    pub forge_version: Option<String>,
    rollback_notice: RollbackNotice,
    pub projects: BTreeMap<ProjectId, Project>,
    pub threads: HashMap<ThreadId, Thread>,
    pub runs: HashMap<ThreadId, Run>,
    stopping: HashMap<ThreadId, stopping::Stopping>,
    pub settings: HostSettings,
    pub engines: BTreeMap<EngineId, EngineStatus>,
    account: Option<Account>,
    pub global_prompt: Option<Arc<artisan_api::GlobalPrompt>>,
    prompt_load: Projection,
    catalog_load: Projection,
    projection_generation: u64,
    pending_events: HostEffects,
    prompt_revision: u64,
    catalog_revision: u64,
    pub skill_catalog: Option<Arc<[artisan_api::CatalogSkill]>>,
    waiting: HashMap<ThreadId, BTreeSet<ItemId>>,
    thread_pages: pages::ThreadPages,
    preferences: preferences::Preferences,
    images: images::ImageCache,
    project_icons: project_icons::ProjectIcons,
    read_acks: BTreeMap<ThreadId, artisan_api::Seq>,
    read_observed: Option<artisan_api::Seq>,
    read_writer: Option<ReadWriter>,
    read_generation: u64,
    pub wsl: Option<WslStatus>,
    wsl_target: wsl::WslTarget,
    wsl_probe: Option<AbortOnDropHandle<()>>,
    wsl_start: Option<AbortOnDropHandle<()>>,
    wsl_awake: Option<tokio::process::Child>,
    wsl_generation: u64,
    live: HashMap<ThreadId, HashMap<LiveTarget, LiveSnapshot>>,
    live_streams: u64,
    suggestions: HashMap<ThreadId, String>,
    observed: watch::Sender<Option<ThreadId>>,
    summary: Option<ObservedSummary>,
    summary_generation: u64,
    retry: Arc<Notify>,
    delivery: Delivery<HostMessage>,
    connection: AbortOnDropHandle<()>,
    claude_ai: claude_ai_browser::Renewal<AbortOnDropHandle<()>>,
    repositories: HashMap<ProjectId, RepositoryEntry>,
    repository_reads: HashMap<ProjectId, (u64, AbortOnDropHandle<()>)>,
    repository_order: VecDeque<ProjectId>,
    repository_generation: u64,
    clock: crate::core::clock::Clock,
}

impl HostData {
    pub(crate) fn source_provider(&self) -> ScopedSourceProvider {
        ScopedSourceProvider {
            host: self.id.clone(),
            instance: self.instance,
            client: self.client.clone(),
            authority: Rc::downgrade(&self.source_authority.borrow()),
        }
    }
    pub(crate) fn revoke_source_providers(&self) {
        *self.source_authority.borrow_mut() = Rc::new(());
    }
    pub(crate) fn new(
        id: HostId,
        name: String,
        client: ForgeClient,
        wake: aqua::mailbox::Sender<()>,
        order: AdmissionOrder,
        effects: crate::core::store::Effects,
        clock: crate::core::clock::Clock,
    ) -> Self {
        let delivery = Delivery::with_order(
            NonZeroUsize::new(64).expect("invariant: the host delivery capacity is non-zero"),
            wake,
            order,
        );
        let retry = Arc::new(Notify::new());
        let (observed, observation) = watch::channel(None);
        let connection = connection::follow(
            &id,
            client.clone(),
            delivery.sender().map(HostMessage::Update),
            Arc::clone(&retry),
            observation,
        );
        let instance =
            HostInstance(NEXT_HOST_INSTANCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
        let pending_events = HostEffects {
            queue: effects,
            id: id.clone(),
            instance,
        };
        let settings = HostSettings::new(id.clone());
        Self {
            instance,
            source_authority: RefCell::new(Rc::new(())),
            reported_live: false,
            id,
            name,
            client,
            status: ConnectionStatus::Connecting,
            forge_version: None,
            rollback_notice: RollbackNotice::Hidden,
            projects: BTreeMap::new(),
            threads: HashMap::new(),
            runs: HashMap::new(),
            stopping: HashMap::new(),
            settings,
            engines: BTreeMap::new(),
            account: None,
            global_prompt: None,
            prompt_load: Projection::default(),
            catalog_load: Projection::default(),
            projection_generation: 0,
            pending_events,
            prompt_revision: 0,
            catalog_revision: 0,
            skill_catalog: None,
            waiting: HashMap::new(),
            thread_pages: pages::ThreadPages::default(),
            preferences: preferences::Preferences::default(),
            images: images::ImageCache::default(),
            project_icons: project_icons::ProjectIcons::default(),
            read_acks: BTreeMap::new(),
            read_observed: None,
            read_writer: None,
            read_generation: 0,
            wsl: None,
            wsl_target: wsl::WslTarget::None,
            wsl_probe: None,
            wsl_start: None,
            wsl_awake: None,
            wsl_generation: 0,
            live: HashMap::new(),
            live_streams: 0,
            suggestions: HashMap::new(),
            observed,
            summary: None,
            summary_generation: 0,
            retry,
            delivery,
            connection,
            claude_ai: claude_ai_browser::Renewal::default(),
            repositories: HashMap::new(),
            repository_reads: HashMap::new(),
            repository_order: VecDeque::new(),
            repository_generation: 0,
            clock,
        }
    }
    pub fn instance(&self) -> HostInstance {
        self.instance
    }

    pub fn is_working(&self, thread: &ThreadId) -> bool {
        self.runs
            .get(thread)
            .is_some_and(|run| run.status().is_live())
    }
    pub fn can_continue(&self, thread: &ThreadId) -> bool {
        self.runs
            .get(thread)
            .is_some_and(|run| run.status().is_stopped())
    }
    pub fn is_waiting(&self, thread: &ThreadId) -> bool {
        self.waiting
            .get(thread)
            .is_some_and(|items| !items.is_empty())
    }
    pub fn is_live(&self) -> bool {
        self.status == ConnectionStatus::Live
    }
    pub fn setting(&self, key: &str) -> Option<&serde_json::Value> {
        self.settings.get(key)
    }
    pub fn mcp_servers(&self) -> Result<&[artisan_api::McpServer], &McpServersError> {
        self.settings.mcp_servers()
    }
    pub(crate) fn close(self) {
        self.connection.abort();
        self.client.close_relay();
    }
    pub fn retry_now(&self) {
        self.retry.notify_one();
    }
    pub fn attention(&self) -> Attention {
        self.threads
            .values()
            .filter(|thread| {
                !thread.archived
                    && thread.parent_thread_id.is_none()
                    && self.projects.contains_key(&thread.project_id)
            })
            .map(|thread| self.thread_attention(thread))
            .fold(Attention::default(), std::ops::Add::add)
    }
    fn thread_attention(&self, thread: &Thread) -> Attention {
        let settled = thread.settled_seq > 0 && thread.settled_seq >= thread.activity_seq;
        if self.is_waiting(&thread.id) {
            Attention {
                waiting: 1,
                stopped: 0,
            }
        } else if !self.is_working(&thread.id)
            && thread.parked_until.is_none()
            && thread.is_unread()
            && !settled
        {
            Attention {
                waiting: 0,
                stopped: 1,
            }
        } else {
            Attention::default()
        }
    }
    pub fn observe_thread(&mut self, thread: Option<ThreadId>) {
        if *self.observed.borrow() != thread {
            self.live.retain(|id, _| Some(id) == thread.as_ref());
            self.observed.send_replace(thread);
            self.read_observed = None;
            self.reset_thread_summary();
        }
    }
    pub fn suggestion(&self, thread: &ThreadId) -> Option<&str> {
        self.suggestions.get(thread).map(String::as_str)
    }
    pub fn live_snapshot(&self, thread: &ThreadId, target: &LiveTarget) -> Option<LiveSnapshot> {
        self.live.get(thread)?.get(target).cloned()
    }
    pub fn live_targets(
        &self,
        thread: &ThreadId,
    ) -> impl Iterator<Item = (&LiveTarget, &LiveSnapshot)> {
        self.live.get(thread).into_iter().flatten()
    }
    pub fn stop_optimistically(&mut self, thread: &ThreadId) {
        let Some(run) = self.runs.get_mut(thread) else {
            return;
        };
        let Some(stopping) = stopping::Stopping::begin(run, artisan_api::now()) else {
            return;
        };
        self.stopping.insert(thread.clone(), stopping);
        self.pending_events.push(HostEvent::ThreadChanged {
            thread_id: thread.clone(),
        });
        self.pending_events.push(HostEvent::Changed);
    }

    pub fn stop_failed(&mut self, thread: &ThreadId) {
        let Some(stopping) = self.stopping.remove(thread) else {
            return;
        };
        if let Some(run) = self.runs.get_mut(thread) {
            stopping.revert(run);
        }
        self.pending_events.push(HostEvent::ThreadChanged {
            thread_id: thread.clone(),
        });
        self.pending_events.push(HostEvent::Changed);
    }

    fn remember_run(&mut self, run: Run) -> bool {
        let thread = run.thread_id.clone();
        if self.is_deleted(&thread) {
            return false;
        }
        let stale = self.runs.get(&thread).is_some_and(|known| {
            if known.id == run.id {
                known.seq > run.seq
            } else {
                known.started_at > run.started_at
            }
        });
        if stale {
            return false;
        }
        let run = match self.stopping.get(&thread).cloned() {
            Some(stopping) => match stopping.hold(run) {
                stopping::Held::Stopping(held) => held,
                stopping::Held::Settled(settled) => {
                    self.stopping.remove(&thread);
                    settled
                }
                stopping::Held::Other(other) => {
                    self.stopping.remove(&thread);
                    other
                }
            },
            None => run,
        };
        self.runs.insert(thread, run);
        true
    }
    pub fn invalidate_catalog(&mut self) {
        self.catalog_load = Projection::default();
        self.skill_catalog = None;
        self.catalog_revision = self.catalog_revision.saturating_add(1);
    }
    fn invalidate_prompt(&mut self) {
        self.prompt_load = Projection::default();
        self.global_prompt = None;
        self.prompt_revision = self.prompt_revision.saturating_add(1);
    }
    pub fn remember_prompt(&mut self, prompt: artisan_api::GlobalPrompt) -> bool {
        let expected = self
            .settings
            .get("global_prompt_version")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        if prompt.version < expected {
            return false;
        }
        self.prompt_load = Projection::default();
        self.global_prompt = Some(Arc::new(prompt));
        self.pending_events.push(HostEvent::Changed);
        true
    }
    pub fn acknowledge_read(&mut self, thread: ThreadId, observed_seq: artisan_api::Seq) {
        if self.observed.borrow().as_ref() != Some(&thread) {
            return;
        }
        let Some(summary) = self.threads.get(&thread) else {
            return;
        };
        let seq = observed_seq.min(summary.activity_seq);
        if self.read_observed.is_some_and(|previous| seq <= previous) {
            return;
        }
        if !self.read_acks.contains_key(&thread) && self.read_acks.len() >= 128 {
            return;
        }
        self.read_observed = Some(seq);
        if seq <= summary.read_seq && !summary.marked_unread {
            return;
        }
        self.read_acks
            .entry(thread)
            .and_modify(|pending| *pending = (*pending).max(seq))
            .or_insert(seq);
        self.pump_read_acks(None);
    }

    #[must_use]
    pub fn has_requests_in_flight(&self) -> bool {
        self.read_writer.is_some()
            || !self.read_acks.is_empty()
            || !self.repository_reads.is_empty()
            || self.project_icons_reading()
    }

    fn pump_read_acks(&mut self, after: Option<&ThreadId>) {
        if self.read_writer.is_some() {
            return;
        }
        let next = after
            .and_then(|after| {
                self.read_acks
                    .range::<ThreadId, _>((
                        std::ops::Bound::Excluded(after),
                        std::ops::Bound::Unbounded,
                    ))
                    .next()
            })
            .or_else(|| self.read_acks.first_key_value())
            .map(|(id, seq)| (id.clone(), *seq));
        let Some((id, seq)) = next else {
            return;
        };
        self.read_generation = self.read_generation.wrapping_add(1);
        let generation = self.read_generation;
        let client = self.client.clone();
        let sender = self.delivery.sender().map(HostMessage::completion);
        let queued = std::time::Instant::now();
        let thread = id.clone();
        let task = crate::core::runtime::runtime().spawn(async move {
            let started = queued.elapsed();
            let update = artisan_api::UpdateThread {
                read_seq: Some(seq),
                ..Default::default()
            };
            let result = client.update_thread(&id, &update).await;
            match &result {
                Ok(_) => {
                    crate::core::diagnostics::open_timing::rpc_done(started, queued.elapsed());
                }
                Err(error) => {
                    if read_ack_failure(error) == ReadAckFailure::Transient {
                        tokio::time::sleep(READ_ACK_RETRY).await;
                    }
                }
            }
            sender
                .offer(HostCompletion::ReadAck(id, seq, generation, result))
                .await;
        });
        self.read_writer = Some(ReadWriter {
            thread,
            _task: AbortOnDropHandle::new(task),
        });
    }

    fn cancel_read_ack(&mut self, thread: &ThreadId) {
        self.read_acks.remove(thread);
        if self
            .read_writer
            .as_ref()
            .is_some_and(|writer| &writer.thread == thread)
        {
            self.read_writer = None;
            self.read_generation = self.read_generation.wrapping_add(1);
            self.pump_read_acks(Some(thread));
        }
    }

    pub fn thread_summary(&self, thread: &ThreadId) -> Option<&ThreadSummary> {
        self.summary
            .as_ref()
            .filter(|summary| &summary.thread == thread)
            .map(|summary| &summary.state.view)
    }

    pub fn load_thread_summary(&mut self, thread: &ThreadId, retry: bool) {
        if !self.is_live() {
            return;
        }
        let Some(summary) = self
            .summary
            .as_mut()
            .filter(|summary| &summary.thread == thread)
        else {
            return;
        };
        let state = &mut summary.state;
        if state.view.loading
            || (!retry
                && (state.view.error.is_some() || (!state.dirty && state.view.value.is_some())))
        {
            return;
        }
        state.view.loading = true;
        state.view.error = None;
        let minimum = state.minimum_revision;
        let previous = state.view.value.clone();
        self.summary_generation = self.summary_generation.wrapping_add(1);
        let generation = self.summary_generation;
        state.generation = generation;
        let client = self.client.clone();
        let sender = self.delivery.sender().map(HostMessage::completion);
        let thread = thread.clone();
        let task_thread = thread.clone();
        let task = crate::core::runtime::runtime().spawn(async move {
            let result = client.inspector(&task_thread, previous.as_deref()).await;
            sender
                .offer(HostCompletion::Summary(
                    task_thread,
                    generation,
                    minimum,
                    result,
                ))
                .await;
        });
        state.task = Some(AbortOnDropHandle::new(task));
        self.pending_events
            .push(HostEvent::ThreadSummaryChanged { thread_id: thread });
    }

    fn reset_thread_summary(&mut self) {
        self.summary = self
            .observed
            .borrow()
            .clone()
            .map(|thread| ObservedSummary {
                thread,
                state: SummaryLoad {
                    dirty: true,
                    ..SummaryLoad::default()
                },
            });
        if let Some(thread) = self.summary.as_ref().map(|summary| summary.thread.clone()) {
            self.load_thread_summary(&thread, false);
        }
    }

    fn summary_connection_changed(&mut self) {
        let live = self.is_live();
        let Some(summary) = self.summary.as_mut() else {
            return;
        };
        summary.state.task = None;
        summary.state.view.loading = false;
        summary.state.dirty = true;
        if live {
            let thread = summary.thread.clone();
            self.load_thread_summary(&thread, true);
        }
    }

    fn invalidate_thread_summary(&mut self, event: &Event, seq: Option<artisan_api::Seq>) {
        use artisan_api::{ItemKind, NoticeKind, ToolKind};
        let relevant = match event {
            Event::Run { .. } | Event::ItemState { .. } => true,
            Event::Item { item } => matches!(
                item.kind,
                ItemKind::UserMessage { .. }
                    | ItemKind::Agent { .. }
                    | ItemKind::Plan { .. }
                    | ItemKind::Question { .. }
                    | ItemKind::Notice {
                        kind: NoticeKind::Goal { .. },
                        ..
                    }
                    | ItemKind::Tool {
                        tool: ToolKind::Shell { .. } | ToolKind::Edit { .. },
                        ..
                    }
            ),
            Event::Hello { .. }
            | Event::Project { .. }
            | Event::Thread { .. }
            | Event::ThreadRemoved { .. }
            | Event::Setting { .. }
            | Event::Live { .. }
            | Event::Suggestion { .. }
            | Event::Engine { .. }
            | Event::EngineInstall { .. }
            | Event::EditorCall { .. }
            | Event::Reset => false,
        };
        if !relevant {
            return;
        }
        let Some(thread) = event.thread_id() else {
            return;
        };
        let Some(summary) = self
            .summary
            .as_mut()
            .filter(|summary| &summary.thread == thread)
        else {
            return;
        };
        let state = &mut summary.state;
        state.minimum_revision = state.minimum_revision.max(seq.unwrap_or(0));
        if state
            .view
            .value
            .as_ref()
            .is_some_and(|value| value.revision >= state.minimum_revision)
        {
            return;
        }
        state.dirty = true;
        state.view.error = None;
        self.load_thread_summary(thread, false);
    }

    pub const fn prompt_load(&self) -> &ProjectionLoad {
        &self.prompt_load.view
    }
    pub const fn catalog_load(&self) -> &ProjectionLoad {
        &self.catalog_load.view
    }

    pub fn load_prompt(&mut self, retry: bool) {
        if !self.is_live()
            || self.prompt_load.view.loading
            || (!retry && (self.global_prompt.is_some() || self.prompt_load.view.error.is_some()))
        {
            return;
        }
        self.projection_generation = self.projection_generation.wrapping_add(1);
        let generation = self.projection_generation;
        let client = self.client.clone();
        let sender = self.delivery.sender().map(HostMessage::completion);
        let task = crate::core::runtime::runtime().spawn(async move {
            let result = client.global_prompt().await;
            sender
                .offer(HostCompletion::Prompt(generation, result))
                .await;
        });
        self.prompt_load = Projection::loading(generation, Some(AbortOnDropHandle::new(task)));
        self.pending_events.push(HostEvent::Changed);
    }

    pub fn load_skill_catalog(&mut self, retry: bool) {
        if !self.is_live()
            || self.catalog_load.view.loading
            || (!retry && (self.skill_catalog.is_some() || self.catalog_load.view.error.is_some()))
        {
            return;
        }
        self.projection_generation = self.projection_generation.wrapping_add(1);
        let generation = self.projection_generation;
        let client = self.client.clone();
        let sender = self.delivery.sender().map(HostMessage::completion);
        let task = crate::core::runtime::runtime().spawn(async move {
            let result = client.skill_catalog().await;
            sender
                .offer(HostCompletion::Catalog(generation, result))
                .await;
        });
        self.catalog_load = Projection::loading(generation, Some(AbortOnDropHandle::new(task)));
        self.pending_events.push(HostEvent::Changed);
    }

    pub fn repository(&self, project: &ProjectId) -> Option<&artisan_api::ProjectRepository> {
        self.repositories.get(project)?.value.as_ref()
    }

    #[must_use]
    pub fn repository_checked(&self, project: &ProjectId) -> bool {
        self.repositories.contains_key(project)
    }

    pub fn load_repository(&mut self, project: &ProjectId, refresh: bool) {
        if !self.projects.contains_key(project)
            || !self.is_live()
            || self.repository_reads.contains_key(project)
            || self.repository_reads.len() >= 4
        {
            return;
        }
        if self.repositories.contains_key(project) {
            self.repository_order.retain(|id| id != project);
            self.repository_order.push_back(project.clone());
        }
        if !refresh
            && self.repositories.get(project).is_some_and(|entry| {
                self.clock.now().saturating_duration_since(entry.checked)
                    < std::time::Duration::from_secs(if entry.failed { 5 } else { 2 })
            })
        {
            return;
        }
        self.repository_generation = self.repository_generation.wrapping_add(1);
        let generation = self.repository_generation;
        let client = self.client.clone();
        let sender = self.delivery.sender().map(HostMessage::completion);
        let project = project.clone();
        let key = project.clone();
        let task = crate::core::runtime::runtime().spawn(async move {
            let result = client.project_repository(&project).await;
            sender
                .offer(HostCompletion::Repository(project, generation, result))
                .await;
        });
        self.repository_reads
            .insert(key, (generation, AbortOnDropHandle::new(task)));
    }

    pub(crate) fn peek_order(&mut self) -> Option<u64> {
        self.delivery.peek_order()
    }
    pub(crate) fn admitted(&self) -> usize {
        self.delivery.admitted()
    }
    pub(crate) fn drain(&mut self) {
        let events = self.pending_events.clone();
        match self.delivery.drain_one() {
            Some(HostMessage::Update(update)) => self.apply_update(update, &events),
            Some(HostMessage::Completion(completion)) => {
                self.apply_completion(*completion, &events);
            }
            None => {}
        }
    }
    fn apply_update(&mut self, update: Update, events: &HostEffects) {
        match update {
            Update::Status(status) => {
                if self.status != status {
                    self.status = status;
                    if !self.is_live() {
                        self.prompt_load = Projection::default();
                        self.catalog_load = Projection::default();
                    }
                    self.summary_connection_changed();
                    self.wsl_connection_changed();
                    events.push(HostEvent::Changed);
                }
            }
            Update::State(state) => self.reset_to_state(*state, events),
            Update::Event(envelope, _permit) => {
                self.invalidate_thread_summary(&envelope.event, envelope.seq);
                self.apply_event(envelope.event, events);
            }
        }
    }
    fn reset_to_state(&mut self, state: artisan_api::State, events: &HostEffects) {
        self.repositories.clear();
        self.repository_reads.clear();
        self.repository_order.clear();
        let paths: HashMap<_, _> = state
            .projects
            .iter()
            .map(|project| (project.id.clone(), project.path.clone()))
            .collect();
        let projects = &self.projects;
        self.project_icons.reset(|id| {
            projects
                .get(id)
                .is_some_and(|old| paths.get(id) == Some(&old.path))
        });
        self.forge_version = Some(state.forge_version);
        self.rollback_notice = RollbackNotice::from_state(state.database_rollback);
        self.projects = state
            .projects
            .into_iter()
            .map(|project| (project.id.clone(), project))
            .collect();
        self.threads = state
            .threads
            .into_iter()
            .map(|thread| (thread.id.clone(), thread))
            .collect();
        self.reset_thread_pages(state.threads_next);
        self.runs.clear();
        for run in state.runs {
            self.remember_run(run);
        }
        self.settings = HostSettings::from_values(self.id.clone(), state.settings);
        self.invalidate_prompt();
        self.invalidate_catalog();
        self.learn_wsl_distro(
            state
                .account
                .as_ref()
                .and_then(|account| account.wsl_distro.as_deref()),
        );
        self.account = state.account;
        self.wsl_connection_changed();
        self.engines = state
            .engines
            .into_iter()
            .map(|status| (status.engine, status))
            .collect();
        self.renew_claude_ai();
        self.waiting.clear();
        for item in state.waiting {
            self.waiting
                .entry(item.thread_id.clone())
                .or_default()
                .insert(item.id);
        }
        self.live.clear();
        self.suggestions.clear();
        self.prune_threads();
        let observed = self.observed.borrow().clone();
        if let Some(thread) = observed {
            self.ensure_thread(&thread);
        }
        self.reset_thread_summary();
        events.push(HostEvent::Reset);
        events.push(HostEvent::Changed);
    }
    fn apply_completion(&mut self, completion: HostCompletion, events: &HostEffects) {
        match completion {
            HostCompletion::ClaudeAiCaptured(ticket, result) => {
                self.finish_claude_ai_capture(ticket, result);
            }
            HostCompletion::ClaudeAiHanded(ticket, result) => {
                self.finish_claude_ai_handover(ticket, result);
            }
            HostCompletion::DecodedImage(hash, generation, result) => {
                if self.finish_decoded_image(hash, generation, result) {
                    events.push(HostEvent::ImagesChanged);
                }
            }
            HostCompletion::ProjectIcon(project, generation, result) => {
                if self.finish_project_icon(project, generation, result) {
                    events.push(HostEvent::ImagesChanged);
                }
            }
            HostCompletion::Image(hash, generation, result) => {
                if self.finish_image_download(hash, generation, result) {
                    events.push(HostEvent::ImagesChanged);
                }
            }
            HostCompletion::Preference(generation, choice, preference, result) => {
                if self.finish_preference(generation, choice, preference, result) {
                    events.push(HostEvent::Changed);
                }
            }
            HostCompletion::WslProbe(generation, target, result) => {
                if self.finish_wsl_probe(generation, &target, result) {
                    events.push(HostEvent::Changed);
                }
            }
            HostCompletion::WslStarted(distro, result) => {
                self.finish_wsl_start(&distro, result);
                events.push(HostEvent::Changed);
            }
            HostCompletion::RollbackDismissed(result) => {
                self.finish_rollback_dismissal(result);
                events.push(HostEvent::Changed);
            }
            HostCompletion::ReadAck(id, seq, generation, result) => {
                self.finish_read_ack(&id, seq, generation, result, events);
            }
            HostCompletion::ThreadPage {
                archived,
                direction,
                started_seq,
                generation,
                result,
            } => {
                if self.finish_thread_page(archived, direction, started_seq, generation, result) {
                    events.push(HostEvent::Changed);
                }
            }
            HostCompletion::ThreadLookup(id, generation, result) => {
                self.finish_thread_lookup(id, generation, result);
            }
            HostCompletion::Summary(thread, generation, minimum, result) => {
                self.finish_summary(thread, generation, minimum, result, events);
            }
            HostCompletion::Prompt(generation, result) => {
                self.finish_prompt(generation, result, events);
            }
            HostCompletion::Catalog(generation, result) => {
                self.finish_catalog(generation, result, events);
            }
            HostCompletion::Repository(project, generation, result) => {
                if self.finish_repository(project, generation, result) {
                    events.push(HostEvent::Changed);
                }
            }
        }
    }
    fn finish_read_ack(
        &mut self,
        id: &ThreadId,
        seq: artisan_api::Seq,
        generation: u64,
        result: Result<Thread, crate::core::client::CallError>,
        events: &HostEffects,
    ) {
        if generation != self.read_generation {
            return;
        }
        self.read_writer = None;
        match result {
            Ok(thread) => {
                if self
                    .read_acks
                    .get(id)
                    .is_some_and(|pending| *pending <= seq)
                {
                    self.read_acks.remove(id);
                }
                if self.remember_thread(thread) {
                    events.push(HostEvent::ThreadChanged {
                        thread_id: id.clone(),
                    });
                }
            }
            Err(error) => match read_ack_failure(&error) {
                ReadAckFailure::Refused => {
                    tracing::warn!(
                        host = %self.id,
                        thread = %id,
                        seq,
                        error = %::diagnostics::chain(&error),
                        "Forge refused a read acknowledgement; dropping it"
                    );
                    self.read_acks.remove(id);
                }
                ReadAckFailure::Transient => {
                    tracing::debug!(
                        host = %self.id,
                        thread = %id,
                        seq,
                        error = %::diagnostics::chain(&error),
                        "read acknowledgement will retry"
                    );
                }
            },
        }
        self.pump_read_acks(Some(id));
    }
    fn finish_summary(
        &mut self,
        thread: ThreadId,
        generation: u64,
        minimum: artisan_api::Seq,
        result: Result<artisan_api::InspectorSummary, crate::core::client::CallError>,
        events: &HostEffects,
    ) {
        let Some(summary) = self
            .summary
            .as_mut()
            .filter(|summary| summary.thread == thread && summary.state.generation == generation)
        else {
            return;
        };
        let state = &mut summary.state;
        state.view.loading = false;
        state.view.error = None;
        state.task = None;
        match result {
            Ok(value)
                if value.revision < state.minimum_revision && state.minimum_revision > minimum =>
            {
                self.load_thread_summary(&thread, true);
                return;
            }
            Ok(value) if value.revision < state.minimum_revision => {
                state.view.error = Some(
                    "Forge returned thread details older than its latest update. Retry to refresh."
                        .into(),
                );
            }
            Ok(value) if summary_bytes(&value) > 16 * 1024 * 1024 => {
                state.view.error = Some("Thread details exceed the 16 MiB display limit.".into());
            }
            Ok(value) => {
                if state
                    .view
                    .value
                    .as_ref()
                    .is_none_or(|previous| value.revision >= previous.revision)
                {
                    state.minimum_revision = value.revision;
                    state.view.value = Some(Arc::new(value));
                    state.dirty = false;
                }
            }
            Err(error) => {
                state.view.error = Some(format!(
                    "Couldn't load thread details: {}",
                    ::diagnostics::chain(&error)
                ));
            }
        }
        events.push(HostEvent::ThreadSummaryChanged { thread_id: thread });
    }
    fn finish_prompt(
        &mut self,
        generation: u64,
        result: Result<artisan_api::GlobalPrompt, crate::core::client::CallError>,
        events: &HostEffects,
    ) {
        if self.prompt_load.generation != generation || !self.prompt_load.view.loading {
            return;
        }
        self.prompt_load = Projection::default();
        match result {
            Ok(prompt) => {
                let expected = self
                    .settings
                    .get("global_prompt_version")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                if prompt.version < expected {
                    self.prompt_load.view.error =
                        Some("Forge returned an older prompt version. Retry to refresh.".into());
                } else {
                    self.global_prompt = Some(Arc::new(prompt));
                }
            }
            Err(error) => {
                self.prompt_load.view.error = Some(format!(
                    "Couldn't read the global prompt: {}",
                    ::diagnostics::chain(&error)
                ));
            }
        }
        events.push(HostEvent::Changed);
    }
    fn finish_catalog(
        &mut self,
        generation: u64,
        result: Result<Vec<artisan_api::CatalogSkill>, crate::core::client::CallError>,
        events: &HostEffects,
    ) {
        if self.catalog_load.generation != generation || !self.catalog_load.view.loading {
            return;
        }
        self.catalog_load = Projection::default();
        match result {
            Ok(catalog) => self.skill_catalog = Some(Arc::from(catalog)),
            Err(error) => {
                self.catalog_load.view.error = Some(format!(
                    "Couldn't list skills: {}",
                    ::diagnostics::chain(&error)
                ));
            }
        }
        events.push(HostEvent::Changed);
    }
    fn finish_repository(
        &mut self,
        project: ProjectId,
        generation: u64,
        result: Result<Option<artisan_api::ProjectRepository>, crate::core::client::CallError>,
    ) -> bool {
        if self
            .repository_reads
            .get(&project)
            .is_none_or(|(current, _)| *current != generation)
        {
            return false;
        }
        self.repository_reads.remove(&project);
        let failed = result.is_err();
        let value = match result {
            Ok(value) => value,
            Err(error) => {
                tracing::debug!(%project, "repository read failed: {}", ::diagnostics::chain(&error));
                self.repositories
                    .get(&project)
                    .and_then(|entry| entry.value.clone())
            }
        };
        let changed = self
            .repositories
            .get(&project)
            .is_none_or(|entry| entry.value != value || entry.failed != failed);
        self.repositories.insert(
            project.clone(),
            RepositoryEntry {
                value,
                checked: self.clock.now(),
                failed,
            },
        );
        self.repository_order.retain(|id| id != &project);
        self.repository_order.push_back(project);
        while self.repositories.len() > 128 {
            if let Some(oldest) = self.repository_order.pop_front() {
                self.repositories.remove(&oldest);
            }
        }
        changed
    }
    fn apply_event(&mut self, event: Event, events: &HostEffects) {
        match event {
            Event::Hello { forge_version, .. } => self.forge_version = Some(forge_version),
            Event::Project { project } => self.apply_project(project, events),
            Event::Thread { thread } => {
                let thread_id = thread.id.clone();
                if self.remember_thread(thread) {
                    events.push(HostEvent::ThreadChanged { thread_id });
                }
            }
            Event::ThreadRemoved { thread_id } => {
                self.forget_thread(&thread_id);
                self.waiting.remove(&thread_id);
                self.live.remove(&thread_id);
                self.suggestions.remove(&thread_id);
                events.push(HostEvent::ThreadChanged { thread_id });
                events.push(HostEvent::Changed);
            }
            Event::Run { run } => {
                if !run.status().is_live() {
                    self.live.remove(&run.thread_id);
                }
                let thread_id = run.thread_id.clone();
                self.remember_run(run);
                if self.is_working(&thread_id) && !self.threads.contains_key(&thread_id) {
                    self.ensure_thread(&thread_id);
                }
                events.push(HostEvent::ThreadChanged { thread_id });
            }
            Event::Setting { key, value } => {
                if matches!(
                    key.as_str(),
                    "global_prompt" | "global_prompt_version" | "prompt_revision"
                ) {
                    self.invalidate_prompt();
                }
                if matches!(key.as_str(), "disabled_skills" | "skills_revision") {
                    self.invalidate_catalog();
                }
                self.settings.insert(&key, value);
                events.push(HostEvent::Changed);
            }
            Event::EngineInstall { engine, install } => {
                if let Some(status) = self.engines.get_mut(&engine)
                    && status.install != install
                {
                    status.install = install;
                    events.push(HostEvent::EngineInstall { engine });
                }
            }
            Event::Engine { status } => {
                if self
                    .engines
                    .get(&status.engine)
                    .is_none_or(|previous| previous.install != status.install)
                {
                    self.invalidate_prompt();
                    self.invalidate_catalog();
                }
                self.engines.insert(status.engine, status);
                self.renew_claude_ai();
                events.push(HostEvent::Changed);
            }
            Event::Item { item } => self.apply_item(item, events),
            Event::ItemState {
                id,
                thread_id,
                status,
            } => {
                let waiting = self.waiting.entry(thread_id.clone()).or_default();
                let changed = if status == Status::AwaitingUser {
                    waiting.insert(id)
                } else {
                    waiting.remove(&id)
                };
                if changed {
                    if self.is_waiting(&thread_id) && !self.threads.contains_key(&thread_id) {
                        self.ensure_thread(&thread_id);
                    }
                    events.push(HostEvent::ThreadChanged { thread_id });
                }
            }
            Event::Live {
                thread_id,
                target,
                text,
                append,
                coverage,
            } => self.apply_live(thread_id, target, &text, append, coverage, events),
            Event::Suggestion { thread_id, text } => {
                match text {
                    Some(text) => self.suggestions.insert(thread_id.clone(), text),
                    None => self.suggestions.remove(&thread_id),
                };
                events.push(HostEvent::ThreadChanged { thread_id });
            }
            Event::EditorCall { id, call } => events.push(HostEvent::EditorCall { id, call }),
            Event::Reset => {}
        }
    }
    fn apply_project(&mut self, project: Project, events: &HostEffects) {
        self.repositories.remove(&project.id);
        self.repository_reads.remove(&project.id);
        self.repository_order.retain(|id| id != &project.id);
        let moved = self
            .projects
            .get(&project.id)
            .is_none_or(|old| old.path != project.path);
        if project.removed || moved {
            self.project_icons.forget(&project.id);
        } else {
            self.project_icons.refresh(&project.id);
        }
        if project.removed {
            self.projects.remove(&project.id);
        } else {
            self.projects.insert(project.id.clone(), project);
        }
        events.push(HostEvent::Changed);
    }
    fn apply_item(&mut self, item: Item, events: &HostEffects) {
        let waiting = self.waiting.entry(item.thread_id.clone()).or_default();
        let changed = if item.status == Status::AwaitingUser {
            waiting.insert(item.id.clone())
        } else {
            waiting.remove(&item.id)
        };
        if let Some(live) = self.live.get_mut(&item.thread_id)
            && item.status.is_terminal()
        {
            for target in [
                LiveTarget::Text {
                    item_id: item.id.clone(),
                },
                LiveTarget::Shell {
                    item_id: item.id.clone(),
                },
                LiveTarget::Progress {
                    item_id: item.id.clone(),
                },
            ] {
                live.remove(&target);
            }
        }
        if changed {
            events.push(HostEvent::ThreadChanged {
                thread_id: item.thread_id.clone(),
            });
        }
        events.push(HostEvent::Item(Arc::new(item)));
    }
    fn apply_live(
        &mut self,
        thread_id: ThreadId,
        target: LiveTarget,
        text: &str,
        append: bool,
        coverage: LiveCoverage,
        events: &HostEffects,
    ) {
        let streams = self.live.entry(thread_id.clone()).or_default();
        match streams.get_mut(&target) {
            Some(stream) if append => {
                let end = stream.text.len_chars();
                Arc::make_mut(&mut stream.text).insert(end, text);
                stream.coverage = coverage;
            }
            _ => {
                self.live_streams += 1;
                streams.insert(
                    target.clone(),
                    LiveSnapshot {
                        epoch: self.live_streams,
                        text: Arc::new(Rope::from_str(text)),
                        coverage,
                    },
                );
            }
        }
        events.push(HostEvent::Live { thread_id, target });
    }
}

fn summary_bytes(summary: &artisan_api::InspectorSummary) -> usize {
    use crate::core::{CollectionBytes as _, ResidentBytes as _};
    fn question_heap(question: &artisan_api::Question) -> usize {
        question.id.capacity()
            + question.header.as_ref().map_or(0, String::capacity)
            + question.text.resident_bytes()
            + question.options.resident_bytes(|option| {
                option.id.capacity()
                    + option.label.capacity()
                    + option
                        .description
                        .as_ref()
                        .map_or(0, artisan_api::TextContent::resident_bytes)
            })
    }
    size_of_val(summary)
        + summary
            .plan
            .resident_bytes(|step| step.text.resident_bytes())
        + summary.questions.capacity() * size_of::<artisan_api::ThreadQuestionnaire>()
        + summary
            .questions
            .iter()
            .map(|questionnaire| {
                questionnaire.id.0.capacity()
                    + questionnaire.questions.resident_bytes(question_heap)
            })
            .sum::<usize>()
        + summary.terminals.capacity() * size_of::<artisan_api::InspectorTerminal>()
        + summary
            .terminals
            .iter()
            .map(|terminal| {
                terminal.id.0.capacity()
                    + terminal.command.resident_bytes()
                    + terminal.cwd.as_ref().map_or(0, String::capacity)
            })
            .sum::<usize>()
        + summary.outline.capacity() * size_of::<artisan_api::OutlineEntry>()
        + summary
            .outline
            .iter()
            .map(|entry| entry.id.0.capacity() + entry.label.capacity())
            .sum::<usize>()
        + summary.agents.capacity() * size_of::<artisan_api::InspectorAgent>()
        + summary
            .agents
            .iter()
            .map(|agent| {
                agent.id.0.capacity() + agent.label.capacity() + agent.description.resident_bytes()
            })
            .sum::<usize>()
        + summary.workflows.capacity() * size_of::<artisan_api::InspectorWorkflow>()
        + summary
            .workflows
            .iter()
            .map(|workflow| {
                workflow.id.0.capacity() + workflow.name.as_ref().map_or(0, String::capacity)
            })
            .sum::<usize>()
        + summary.goal.as_ref().map_or(0, |goal| {
            goal.objective.resident_bytes() + goal.summary.capacity()
        })
}

const READ_ACK_RETRY: std::time::Duration = std::time::Duration::from_secs(1);

struct ReadWriter {
    thread: ThreadId,
    _task: AbortOnDropHandle<()>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReadAckFailure {
    Refused,
    Transient,
}

fn read_ack_failure(error: &crate::core::client::CallError) -> ReadAckFailure {
    use artisan_api::{ErrorCode, Failure};
    match error.refused() {
        Some(
            Failure::Thread(_)
            | Failure::Run(_)
            | Failure::Draft(_)
            | Failure::Setting(_)
            | Failure::Project(_)
            | Failure::Mcp(_)
            | Failure::Agent(_)
            | Failure::Auth(_)
            | Failure::Resource(_)
            | Failure::Extension(_)
            | Failure::Engine(_)
            | Failure::Legacy {
                code:
                    ErrorCode::NotFound
                    | ErrorCode::BadRequest
                    | ErrorCode::Conflict
                    | ErrorCode::Unauthorized
                    | ErrorCode::VersionMismatch,
            },
        ) => ReadAckFailure::Refused,
        Some(
            Failure::Internal
            | Failure::Legacy {
                code: ErrorCode::Internal | ErrorCode::EngineUnavailable,
            },
        )
        | None => ReadAckFailure::Transient,
    }
}

impl std::fmt::Debug for HostData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostData")
            .field("id", &self.id)
            .field("instance", &self.instance)
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}
