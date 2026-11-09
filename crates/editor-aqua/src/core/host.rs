mod images;
mod pages;
pub use images::{BlobImage, BlobImages};
mod preferences;
mod project_icons;
mod wsl;
pub use pages::{ThreadPage, ThreadPageDirection};
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

#[derive(Clone)]
pub struct ScopedSourceProvider {
    pub(crate) host: HostId,
    pub(crate) instance: HostInstance,
    pub(crate) client: ForgeClient,
    pub(crate) authority: Weak<()>,
}

impl ScopedSourceProvider {
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
    ) -> Option<futures::future::BoxFuture<'static, anyhow::Result<artisan_api::SourceIndexRange>>>
    {
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
    ) -> Option<futures::future::BoxFuture<'static, anyhow::Result<artisan_api::ContentRange>>>
    {
        let _authority = self.authority.upgrade()?;
        let client = self.client.clone();
        Some(Box::pin(async move {
            client.content_range(&source, offset, limit).await
        }))
    }
}

static NEXT_HOST_INSTANCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

#[derive(Clone)]
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

impl HostEvent {
    pub fn thread(&self) -> Option<&ThreadId> {
        match self {
            HostEvent::ThreadSummaryChanged { thread_id }
            | HostEvent::ThreadChanged { thread_id }
            | HostEvent::Live { thread_id, .. } => Some(thread_id),
            HostEvent::Item(item) => Some(&item.thread_id),
            HostEvent::Reset
            | HostEvent::ImagesChanged
            | HostEvent::EngineInstall { .. }
            | HostEvent::Changed
            | HostEvent::WslDistro { .. }
            | HostEvent::EditorCall { .. } => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Attention {
    pub waiting: usize,
    pub stopped: usize,
}
impl std::ops::Add for Attention {
    type Output = Self;
    fn add(self, other: Self) -> Self {
        Self {
            waiting: self.waiting + other.waiting,
            stopped: self.stopped + other.stopped,
        }
    }
}

#[derive(Default)]
pub struct ThreadSummary {
    pub value: Option<Arc<artisan_api::InspectorSummary>>,
    pub loading: bool,
    pub error: Option<String>,
    minimum_revision: artisan_api::Seq,
    dirty: bool,
    task: Option<AbortOnDropHandle<()>>,
    generation: u64,
}

struct ObservedSummary {
    thread: ThreadId,
    state: ThreadSummary,
}

struct RepositoryEntry {
    value: Option<artisan_api::ProjectRepository>,
    checked: std::time::Instant,
    failed: bool,
}

#[derive(Default)]
pub struct ProjectionLoad {
    pub loading: bool,
    pub error: Option<String>,
    _task: Option<AbortOnDropHandle<()>>,
    generation: u64,
}

pub(super) enum HostMessage {
    Update(Update),
    Completion(HostCompletion),
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
        anyhow::Result<Option<artisan_api::BlobHash>>,
    ),
    Image(artisan_api::BlobHash, u64, anyhow::Result<Vec<u8>>),
    Preference(
        u64,
        preferences::Choice,
        preferences::Preference,
        anyhow::Result<()>,
    ),
    WslProbe(
        u64,
        wsl::WslTarget,
        anyhow::Result<Vec<crate::core::hosts::WslDistro>>,
    ),
    WslStarted(String, anyhow::Result<()>),
    ReadAck(ThreadId, artisan_api::Seq, u64, Thread),
    ThreadPage {
        archived: bool,
        direction: ThreadPageDirection,
        started_seq: artisan_api::Seq,
        generation: u64,
        result: anyhow::Result<artisan_api::ThreadsPage>,
    },
    ThreadLookup(ThreadId, u64, anyhow::Result<Thread>),
    Summary(
        ThreadId,
        u64,
        artisan_api::Seq,
        anyhow::Result<artisan_api::InspectorSummary>,
    ),
    Prompt(u64, anyhow::Result<artisan_api::GlobalPrompt>),
    Catalog(u64, anyhow::Result<Vec<artisan_api::CatalogSkill>>),
    Repository(
        ProjectId,
        u64,
        anyhow::Result<Option<artisan_api::ProjectRepository>>,
    ),
}

#[cfg(feature = "test-support")]
pub struct TestHostTransitions {
    sender: crate::core::delivery::DeliverySender<HostMessage>,
}
#[cfg(feature = "test-support")]
impl TestHostTransitions {
    pub async fn status(&self, status: ConnectionStatus) {
        assert!(
            self.sender
                .send(HostMessage::Update(Update::Status(status)))
                .await
                .is_ok()
        );
    }
    pub async fn event(&self, event: Event) {
        let permit = Arc::new(tokio::sync::Semaphore::new(1))
            .acquire_owned()
            .await
            .unwrap();
        let envelope = artisan_api::Envelope {
            seq: Some(1),
            live_revision: 0,
            event,
        };
        assert!(
            self.sender
                .send(HostMessage::Update(Update::Event(
                    Box::new(envelope),
                    permit,
                )))
                .await
                .is_ok()
        );
    }
    pub async fn prompt(&self, generation: u64, prompt: artisan_api::GlobalPrompt) {
        assert!(
            self.sender
                .send(HostMessage::Completion(HostCompletion::Prompt(
                    generation,
                    Ok(prompt),
                )))
                .await
                .is_ok()
        );
    }
    pub async fn catalog(&self, generation: u64, catalog: Vec<artisan_api::CatalogSkill>) {
        assert!(
            self.sender
                .send(HostMessage::Completion(HostCompletion::Catalog(
                    generation,
                    Ok(catalog),
                )))
                .await
                .is_ok()
        );
    }
    pub async fn image(
        &self,
        hash: artisan_api::BlobHash,
        generation: u64,
        image: Arc<aqua::images::ImageHandle>,
    ) {
        assert!(
            self.sender
                .send(HostMessage::Completion(HostCompletion::DecodedImage(
                    hash,
                    generation,
                    Ok(image),
                )))
                .await
                .is_ok()
        );
    }
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
    #[cfg(feature = "test-support")]
    fn clear(&self) {
        self.queue.clear_host(&self.id, self.instance);
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
    pub projects: BTreeMap<ProjectId, Project>,
    pub threads: HashMap<ThreadId, Thread>,
    pub runs: HashMap<ThreadId, Run>,
    pub settings: serde_json::Map<String, serde_json::Value>,
    pub engines: BTreeMap<EngineId, EngineStatus>,
    pub account: Option<Account>,
    pub global_prompt: Option<Arc<artisan_api::GlobalPrompt>>,
    prompt_load: ProjectionLoad,
    catalog_load: ProjectionLoad,
    projection_generation: u64,
    pending_events: HostEffects,
    pub prompt_revision: u64,
    pub catalog_revision: u64,
    pub skill_catalog: Option<Arc<Vec<artisan_api::CatalogSkill>>>,
    waiting: HashMap<ThreadId, BTreeSet<ItemId>>,
    thread_pages: pages::ThreadPages,
    preferences: preferences::Preferences,
    images: images::ImageCache,
    project_icons: project_icons::ProjectIcons,
    read_acks: BTreeMap<ThreadId, artisan_api::Seq>,
    read_observed: Option<artisan_api::Seq>,
    read_writer: Option<AbortOnDropHandle<()>>,
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
    _connection: AbortOnDropHandle<()>,
    claude_ai_renewed: Option<std::time::Instant>,
    repositories: HashMap<ProjectId, RepositoryEntry>,
    repository_reads: HashMap<ProjectId, (u64, AbortOnDropHandle<()>)>,
    repository_order: VecDeque<ProjectId>,
    repository_generation: u64,
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
    ) -> Self {
        let delivery = Delivery::with_order(NonZeroUsize::new(64).unwrap(), wake, order);
        let retry = Arc::new(Notify::new());
        let (observed, observation) = watch::channel(None);
        let connection = connection::follow(
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
        Self {
            instance,
            source_authority: RefCell::new(Rc::new(())),
            reported_live: false,
            id,
            name,
            client,
            status: ConnectionStatus::Connecting,
            forge_version: None,
            projects: BTreeMap::new(),
            threads: HashMap::new(),
            runs: HashMap::new(),
            settings: serde_json::Map::new(),
            engines: BTreeMap::new(),
            account: None,
            global_prompt: None,
            prompt_load: ProjectionLoad::default(),
            catalog_load: ProjectionLoad::default(),
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
            _connection: connection,
            claude_ai_renewed: None,
            repositories: HashMap::new(),
            repository_reads: HashMap::new(),
            repository_order: VecDeque::new(),
            repository_generation: 0,
        }
    }
    fn renew_claude_ai(&mut self) {
        const EVERY: std::time::Duration = std::time::Duration::from_mins(10);
        let needs = self.engines.get(&EngineId::Claude).is_some_and(|status| {
            matches!(
                status.claude_ai,
                Some(artisan_api::ClaudeAiState::NeedsRenewal { .. })
            )
        });
        if !needs
            || self
                .claude_ai_renewed
                .is_some_and(|at| at.elapsed() < EVERY)
            || !claude_ai_browser::can_renew()
        {
            return;
        }
        self.claude_ai_renewed = Some(std::time::Instant::now());
        let client = self.client.clone();
        let host = self.id.clone();
        crate::core::runtime::runtime().spawn(async move {
            let session = match claude_ai_browser::capture(claude_ai_browser::Mode::Renew).await {
                Ok(session) => session,
                Err(error) => { tracing::warn!(%host, "renewing the claude.ai session failed: {error:#}"); return; }
            };
            if let Err(error) = client.engine_action(EngineId::Claude, &artisan_api::EngineCommand::ClaudeAiSignIn { session }).await { tracing::warn!(%host, "the Forge did not take the renewed claude.ai session: {error:#}"); }
        });
    }

    #[cfg(feature = "test-support")]
    pub fn test_stop_connection(&mut self) {
        self._connection.abort();
        self.delivery.reset_for_tests();
        self.status = ConnectionStatus::Live;
        self.pending_events.clear();
    }
    #[cfg(feature = "test-support")]
    pub fn test_transition_sender(&self) -> TestHostTransitions {
        TestHostTransitions {
            sender: self.delivery.sender(),
        }
    }
    #[cfg(feature = "test-support")]
    pub fn test_begin_projections(&mut self) -> (u64, u64) {
        self.projection_generation += 1;
        let prompt = self.projection_generation;
        self.prompt_load = ProjectionLoad {
            loading: true,
            generation: prompt,
            ..ProjectionLoad::default()
        };
        self.projection_generation += 1;
        let catalog = self.projection_generation;
        self.catalog_load = ProjectionLoad {
            loading: true,
            generation: catalog,
            ..ProjectionLoad::default()
        };
        (prompt, catalog)
    }

    pub fn instance(&self) -> HostInstance {
        self.instance
    }

    pub fn is_working(&self, thread: &ThreadId) -> bool {
        self.runs
            .get(thread)
            .is_some_and(|run| run.status.is_live())
    }
    pub fn can_continue(&self, thread: &ThreadId) -> bool {
        self.runs
            .get(thread)
            .is_some_and(|run| run.status.is_stopped())
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
    pub fn retry_now(&self) {
        self.retry.notify_one();
    }
    pub fn attention(&self) -> Attention {
        let mut attention = Attention::default();
        for thread in self.threads.values() {
            if thread.archived
                || thread.parent_thread_id.is_some()
                || !self.projects.contains_key(&thread.project_id)
            {
                continue;
            }
            let settled = thread.settled_seq > 0 && thread.settled_seq >= thread.activity_seq;
            if self.is_waiting(&thread.id) {
                attention.waiting += 1;
            } else if !self.is_working(&thread.id)
                && thread.parked_until.is_none()
                && thread.is_unread()
                && !settled
            {
                attention.stopped += 1;
            }
        }
        attention
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
    pub fn live(&self, thread: &ThreadId, target: &LiveTarget) -> Option<&Rope> {
        Some(&self.live.get(thread)?.get(target)?.text)
    }
    pub fn live_snapshot(&self, thread: &ThreadId, target: &LiveTarget) -> Option<LiveSnapshot> {
        self.live.get(thread)?.get(target).cloned()
    }
    pub fn live_targets(
        &self,
        thread: &ThreadId,
    ) -> impl Iterator<Item = (&LiveTarget, &Arc<Rope>)> {
        self.live.get(thread).into_iter().flat_map(|targets| {
            targets
                .iter()
                .map(|(target, stream)| (target, &stream.text))
        })
    }
    pub fn threads_by_activity(&self, project: Option<&ProjectId>) -> Vec<&Thread> {
        let mut threads: Vec<_> = self
            .threads
            .values()
            .filter(|thread| project.is_none_or(|project| &thread.project_id == project))
            .collect();
        threads.sort_by(|a, b| {
            b.activity_seq
                .cmp(&a.activity_seq)
                .then_with(|| b.created_at.cmp(&a.created_at))
        });
        threads
    }
    fn remember_run(&mut self, run: Run) {
        match self.runs.get(&run.thread_id) {
            Some(known) if known.id != run.id && known.started_at > run.started_at => {}
            _ => {
                self.runs.insert(run.thread_id.clone(), run);
            }
        }
    }
    pub fn invalidate_catalog(&mut self) {
        self.catalog_load = ProjectionLoad::default();
        self.skill_catalog = None;
        self.catalog_revision = self.catalog_revision.saturating_add(1);
    }
    fn invalidate_prompt(&mut self) {
        self.prompt_load = ProjectionLoad::default();
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
        self.prompt_load = ProjectionLoad::default();
        self.global_prompt = Some(Arc::new(prompt));
        self.pending_events.push(HostEvent::Changed);
        true
    }
    pub fn remember_skill_catalog(&mut self, catalog: Vec<artisan_api::CatalogSkill>) {
        self.catalog_load = ProjectionLoad::default();
        self.skill_catalog = Some(Arc::new(catalog));
        self.pending_events.push(HostEvent::Changed);
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
        self.pump_read_acks();
    }

    fn pump_read_acks(&mut self) {
        if self.read_writer.is_some() {
            return;
        }
        let Some((id, seq)) = self
            .read_acks
            .first_key_value()
            .map(|(id, seq)| (id.clone(), *seq))
        else {
            return;
        };
        self.read_generation = self.read_generation.wrapping_add(1);
        let generation = self.read_generation;
        let client = self.client.clone();
        let sender = self.delivery.sender().map(HostMessage::Completion);
        let queued = std::time::Instant::now();
        let task = crate::core::runtime::runtime().spawn(async move {
            let started = queued.elapsed();
            loop {
                let update = artisan_api::UpdateThread {
                    read_seq: Some(seq),
                    ..Default::default()
                };
                match client.update_thread(&id, &update).await {
                    Ok(thread) => {
                        crate::conversation::open_timing::rpc_done(started, queued.elapsed());
                        let _ = sender
                            .send(HostCompletion::ReadAck(id, seq, generation, thread))
                            .await;
                        return;
                    }
                    Err(error) => {
                        tracing::debug!(%id, "read acknowledgement will retry: {error:#}");
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    }
                }
            }
        });
        self.read_writer = Some(AbortOnDropHandle::new(task));
    }

    pub fn thread_summary(&self, thread: &ThreadId) -> Option<&ThreadSummary> {
        self.summary
            .as_ref()
            .filter(|summary| &summary.thread == thread)
            .map(|summary| &summary.state)
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
        if state.loading
            || (!retry && (state.error.is_some() || (!state.dirty && state.value.is_some())))
        {
            return;
        }
        state.loading = true;
        state.error = None;
        let minimum = state.minimum_revision;
        self.summary_generation = self.summary_generation.wrapping_add(1);
        let generation = self.summary_generation;
        state.generation = generation;
        let client = self.client.clone();
        let sender = self.delivery.sender().map(HostMessage::Completion);
        let thread = thread.clone();
        let task_thread = thread.clone();
        let task = crate::core::runtime::runtime().spawn(async move {
            let result = client.inspector(&task_thread).await;
            let _ = sender
                .send(HostCompletion::Summary(
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
                state: ThreadSummary {
                    dirty: true,
                    ..ThreadSummary::default()
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
        summary.state.loading = false;
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
            _ => false,
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
            .value
            .as_ref()
            .is_some_and(|value| value.revision >= state.minimum_revision)
        {
            return;
        }
        state.dirty = true;
        state.error = None;
        self.load_thread_summary(thread, false);
    }

    pub fn prompt_load(&self) -> &ProjectionLoad {
        &self.prompt_load
    }
    pub fn catalog_load(&self) -> &ProjectionLoad {
        &self.catalog_load
    }

    pub fn load_prompt(&mut self, retry: bool) {
        if !self.is_live()
            || self.prompt_load.loading
            || (!retry && (self.global_prompt.is_some() || self.prompt_load.error.is_some()))
        {
            return;
        }
        self.projection_generation = self.projection_generation.wrapping_add(1);
        let generation = self.projection_generation;
        let client = self.client.clone();
        let sender = self.delivery.sender().map(HostMessage::Completion);
        let task = crate::core::runtime::runtime().spawn(async move {
            let result = client.global_prompt().await;
            let _ = sender
                .send(HostCompletion::Prompt(generation, result))
                .await;
        });
        self.prompt_load = ProjectionLoad {
            loading: true,
            error: None,
            _task: Some(AbortOnDropHandle::new(task)),
            generation,
        };
        self.pending_events.push(HostEvent::Changed);
    }

    pub fn load_skill_catalog(&mut self, retry: bool) {
        if !self.is_live()
            || self.catalog_load.loading
            || (!retry && (self.skill_catalog.is_some() || self.catalog_load.error.is_some()))
        {
            return;
        }
        self.projection_generation = self.projection_generation.wrapping_add(1);
        let generation = self.projection_generation;
        let client = self.client.clone();
        let sender = self.delivery.sender().map(HostMessage::Completion);
        let task = crate::core::runtime::runtime().spawn(async move {
            let result = client.skill_catalog().await;
            let _ = sender
                .send(HostCompletion::Catalog(generation, result))
                .await;
        });
        self.catalog_load = ProjectionLoad {
            loading: true,
            error: None,
            _task: Some(AbortOnDropHandle::new(task)),
            generation,
        };
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
                entry.checked.elapsed()
                    < std::time::Duration::from_secs(if entry.failed { 5 } else { 2 })
            })
        {
            return;
        }
        self.repository_generation = self.repository_generation.wrapping_add(1);
        let generation = self.repository_generation;
        let client = self.client.clone();
        let sender = self.delivery.sender().map(HostMessage::Completion);
        let project = project.clone();
        let key = project.clone();
        let task = crate::core::runtime::runtime().spawn(async move {
            let result = client.project_repository(&project).await;
            let _ = sender
                .send(HostCompletion::Repository(project, generation, result))
                .await;
        });
        self.repository_reads
            .insert(key, (generation, AbortOnDropHandle::new(task)));
    }

    pub(crate) fn peek_order(&mut self) -> Option<u64> {
        self.delivery.peek_order()
    }
    pub(crate) fn drain(&mut self) {
        let mut events = self.pending_events.clone();
        let (update, completion) = match self.delivery.drain_one() {
            Some(HostMessage::Update(update)) => (Some(update), None),
            Some(HostMessage::Completion(completion)) => (None, Some(completion)),
            None => (None, None),
        };
        for update in update.into_iter() {
            match update {
                Update::Status(status) => {
                    if self.status != status {
                        self.status = status;
                        if !self.is_live() {
                            self.prompt_load = ProjectionLoad::default();
                            self.catalog_load = ProjectionLoad::default();
                        }
                        self.summary_connection_changed();
                        self.wsl_connection_changed();
                        events.push(HostEvent::Changed);
                    }
                }
                Update::State(state) => {
                    self.repositories.clear();
                    self.repository_reads.clear();
                    self.repository_order.clear();
                    let state = *state;
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
                    self.settings = state.settings;
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
                Update::Event(envelope, _permit) => {
                    self.invalidate_thread_summary(&envelope.event, envelope.seq);
                    self.apply_event(envelope.event, &mut events);
                }
            }
        }
        for completion in completion.into_iter() {
            match completion {
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
                    self.finish_preference(generation, choice, preference, result);
                }
                HostCompletion::WslProbe(generation, target, result) => {
                    if self.finish_wsl_probe(generation, target, result) {
                        events.push(HostEvent::Changed);
                    }
                }
                HostCompletion::WslStarted(distro, result) => {
                    self.finish_wsl_start(distro, result);
                    events.push(HostEvent::Changed);
                }
                HostCompletion::ReadAck(id, seq, generation, thread) => {
                    if generation != self.read_generation {
                        continue;
                    }
                    self.read_writer = None;
                    if self
                        .read_acks
                        .get(&id)
                        .is_some_and(|pending| *pending <= seq)
                    {
                        self.read_acks.remove(&id);
                    }
                    if self
                        .threads
                        .get(&id)
                        .is_none_or(|old| old.seq <= thread.seq)
                    {
                        self.remember_thread(thread);
                        events.push(HostEvent::ThreadChanged { thread_id: id });
                    }
                    self.pump_read_acks();
                }
                HostCompletion::ThreadPage {
                    archived,
                    direction,
                    started_seq,
                    generation,
                    result,
                } => {
                    if self.finish_thread_page(archived, direction, started_seq, generation, result)
                    {
                        events.push(HostEvent::Changed);
                    }
                }
                HostCompletion::ThreadLookup(id, generation, result) => {
                    self.finish_thread_lookup(id, generation, result);
                }
                HostCompletion::Summary(thread, generation, minimum, result) => {
                    let Some(summary) = self.summary.as_mut().filter(|summary| {
                        summary.thread == thread && summary.state.generation == generation
                    }) else {
                        continue;
                    };
                    let state = &mut summary.state;
                    state.loading = false;
                    state.error = None;
                    state.task = None;
                    match result {
                        Ok(value) if value.revision < state.minimum_revision && state.minimum_revision > minimum => {
                            self.load_thread_summary(&thread, true);
                            continue;
                        }
                        Ok(value) if value.revision < state.minimum_revision => state.error = Some("Forge returned thread details older than its latest update. Retry to refresh.".into()),
                        Ok(value) if summary_bytes(&value) > 16 * 1024 * 1024 => state.error = Some("Thread details exceed the 16 MiB display limit.".into()),
                        Ok(value) => {
                            if state.value.as_ref().is_none_or(|previous| value.revision >= previous.revision) {
                                state.minimum_revision = value.revision;
                                state.value = Some(Arc::new(value));
                                state.dirty = false;
                            }
                        }
                        Err(error) => state.error = Some(format!("Couldn't load thread details: {error:#}")),
                    }
                    events.push(HostEvent::ThreadSummaryChanged { thread_id: thread });
                }
                HostCompletion::Prompt(generation, result) => {
                    if self.prompt_load.generation != generation || !self.prompt_load.loading {
                        continue;
                    }
                    self.prompt_load = ProjectionLoad::default();
                    match result {
                        Ok(prompt) => {
                            let expected = self
                                .settings
                                .get("global_prompt_version")
                                .and_then(serde_json::Value::as_u64)
                                .unwrap_or(0);
                            if prompt.version < expected {
                                self.prompt_load.error = Some(
                                    "Forge returned an older prompt version. Retry to refresh."
                                        .into(),
                                );
                            } else {
                                self.global_prompt = Some(Arc::new(prompt));
                            }
                        }
                        Err(error) => {
                            self.prompt_load.error =
                                Some(format!("Couldn't read the global prompt: {error:#}"))
                        }
                    }
                    events.push(HostEvent::Changed);
                }
                HostCompletion::Catalog(generation, result) => {
                    if self.catalog_load.generation != generation || !self.catalog_load.loading {
                        continue;
                    }
                    self.catalog_load = ProjectionLoad::default();
                    match result {
                        Ok(catalog) => self.skill_catalog = Some(Arc::new(catalog)),
                        Err(error) => {
                            self.catalog_load.error =
                                Some(format!("Couldn't list skills: {error:#}"))
                        }
                    }
                    events.push(HostEvent::Changed);
                }
                HostCompletion::Repository(project, generation, result) => {
                    if !self
                        .repository_reads
                        .get(&project)
                        .is_some_and(|(current, _)| *current == generation)
                    {
                        continue;
                    }
                    self.repository_reads.remove(&project);
                    let failed = result.is_err();
                    let value = match result {
                        Ok(value) => value,
                        Err(error) => {
                            tracing::debug!(%project, "repository read failed: {error:#}");
                            self.repositories
                                .get(&project)
                                .and_then(|entry| entry.value.clone())
                        }
                    };
                    self.repositories.insert(
                        project.clone(),
                        RepositoryEntry {
                            value,
                            checked: std::time::Instant::now(),
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
                    events.push(HostEvent::Changed);
                }
            }
        }
    }
    fn apply_event(&mut self, event: Event, events: &mut HostEffects) {
        match event {
            Event::Hello { forge_version, .. } => self.forge_version = Some(forge_version),
            Event::Project { project } => {
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
            Event::Thread { thread } => {
                let thread_id = thread.id.clone();
                self.remember_thread(thread);
                events.push(HostEvent::ThreadChanged { thread_id });
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
                if !run.status.is_live() {
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
                self.settings.insert(key, value);
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
            Event::Item { item } => {
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
            } => {
                let streams = self.live.entry(thread_id.clone()).or_default();
                match streams.get_mut(&target) {
                    Some(stream) if append => {
                        let end = stream.text.len_chars();
                        Arc::make_mut(&mut stream.text).insert(end, &text);
                        stream.coverage = coverage;
                    }
                    _ => {
                        self.live_streams += 1;
                        streams.insert(
                            target.clone(),
                            LiveSnapshot {
                                epoch: self.live_streams,
                                text: Arc::new(Rope::from_str(&text)),
                                coverage,
                            },
                        );
                    }
                }
                events.push(HostEvent::Live { thread_id, target });
            }
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
    std::mem::size_of_val(summary)
        + summary
            .plan
            .resident_bytes(|step| step.text.resident_bytes())
        + summary.questions.capacity() * std::mem::size_of::<artisan_api::ThreadQuestionnaire>()
        + summary
            .questions
            .iter()
            .map(|questionnaire| {
                questionnaire.id.0.capacity()
                    + questionnaire.questions.resident_bytes(question_heap)
            })
            .sum::<usize>()
        + summary.terminals.capacity() * std::mem::size_of::<artisan_api::InspectorTerminal>()
        + summary
            .terminals
            .iter()
            .map(|terminal| {
                terminal.id.0.capacity()
                    + terminal.command.resident_bytes()
                    + terminal.cwd.as_ref().map_or(0, String::capacity)
            })
            .sum::<usize>()
        + summary.outline.capacity() * std::mem::size_of::<artisan_api::OutlineEntry>()
        + summary
            .outline
            .iter()
            .map(|entry| entry.id.0.capacity() + entry.label.capacity())
            .sum::<usize>()
        + summary.agents.capacity() * std::mem::size_of::<artisan_api::InspectorAgent>()
        + summary
            .agents
            .iter()
            .map(|agent| {
                agent.id.0.capacity() + agent.label.capacity() + agent.description.resident_bytes()
            })
            .sum::<usize>()
        + summary.workflows.capacity() * std::mem::size_of::<artisan_api::InspectorWorkflow>()
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
