//! What one Forge has told us. Updated only by its connection; every
//! section reads from here.

mod projections;
pub use projections::ProjectionLoad;
use ropey::Rope;
mod thread_pages;
mod thread_summary;
use crate::client::ForgeClient;
use crate::connection::{self, ConnectionStatus, Update};
use crate::hosts::HostId;
use artisan_api::{
    Account, EngineId, EngineStatus, Event, Item, ItemId, LiveTarget, Project, ProjectId,
    ProjectRepository, Run, Status, Thread, ThreadId,
};
use futures::StreamExt as _;
use gpui::{Context, EventEmitter, Task};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Weak};
use thread_pages::ThreadPages;
pub use thread_pages::{ThreadPage, ThreadPageDirection};
use thread_summary::ObservedSummary;
pub use thread_summary::ThreadSummary;
use tokio::sync::{Notify, watch};

#[derive(Clone)]
pub enum BlobImage {
    Loading,
    Ready(Arc<gpui::RenderImage>),
    Failed(Arc<str>),
}

// A lookup never retains result bytes. Mounted rows/scenes can keep an evicted
// immutable result alive; reusing it avoids HTTP/decode or failure retry loops.
enum RetainedImage {
    Ready(Weak<gpui::RenderImage>),
    Failed(Weak<str>),
}
impl RetainedImage {
    fn upgrade(&self) -> Option<BlobImage> {
        match self {
            Self::Ready(image) => image.upgrade().map(BlobImage::Ready),
            Self::Failed(error) => error.upgrade().map(BlobImage::Failed),
        }
    }
    fn alive(&self) -> bool {
        match self {
            Self::Ready(image) => image.strong_count() > 0,
            Self::Failed(error) => error.strong_count() > 0,
        }
    }
}

#[derive(Clone)]
pub struct LiveSnapshot {
    pub epoch: u64,
    pub text: Arc<Rope>,
    pub coverage: artisan_api::LiveCoverage,
}

struct RepositoryEntry {
    value: Option<ProjectRepository>,
    checked: std::time::Instant,
    failed: bool,
}
impl RepositoryEntry {
    fn fresh(&self) -> bool {
        let seconds = if self.failed { 5 } else { 2 };
        self.checked.elapsed() < std::time::Duration::from_secs(seconds)
    }
}

pub struct HostData {
    pub id: HostId,
    pub name: String,
    pub client: ForgeClient,
    pub status: ConnectionStatus,
    pub forge_version: Option<String>,
    pub projects: BTreeMap<ProjectId, Project>,
    pub threads: HashMap<ThreadId, Thread>,
    thread_pages: ThreadPages,
    summary: Option<ObservedSummary>,
    summary_generation: u64,
    /// The latest run of each thread.
    pub runs: HashMap<ThreadId, Run>,
    pub settings: serde_json::Map<String, serde_json::Value>,
    /// Lazy canonical projections; forms keep only their unsaved edits.
    prompt_load: ProjectionLoad,
    catalog_load: ProjectionLoad,
    pub global_prompt: Option<Arc<artisan_api::GlobalPrompt>>,
    pub prompt_revision: u64,
    pub catalog_revision: u64,
    pub skill_catalog: Option<Arc<Vec<artisan_api::CatalogSkill>>>,
    pub engines: BTreeMap<EngineId, EngineStatus>,
    /// The account Forge runs as.
    pub account: Option<Account>,
    /// What git said about each project, once asked (see
    /// [`Self::load_repository`]); `None` outside a repository.
    repositories: HashMap<ProjectId, RepositoryEntry>,
    /// Repository reads in flight.
    reading: HashMap<ProjectId, Task<()>>,
    repository_order: std::collections::VecDeque<ProjectId>,
    generation: u64,
    /// Items waiting on the user, by thread.
    waiting: HashMap<ThreadId, BTreeSet<ItemId>>,
    /// Memory-only text of live runs.
    live: HashMap<ThreadId, HashMap<LiveTarget, Arc<Rope>>>,
    live_epochs: HashMap<ThreadId, HashMap<LiveTarget, (u64, artisan_api::LiveCoverage)>>,
    live_epoch: u64,
    /// Cuts the connection's wait before its next attempt short.
    retry: Arc<Notify>,
    observed: watch::Sender<Option<ThreadId>>,
    read_pending: BTreeMap<ThreadId, artisan_api::Seq>,
    /// Evidence from this observation, independent of mutable unread preferences.
    read_observed: Option<artisan_api::Seq>,
    read_writer: Option<Task<()>>,
    images: HashMap<artisan_api::BlobHash, BlobImage>,
    image_tasks: HashMap<artisan_api::BlobHash, Task<()>>,
    retained_images: HashMap<artisan_api::BlobHash, RetainedImage>,
    image_order: std::collections::VecDeque<artisan_api::BlobHash>,
    _follow: Task<()>,
    _connection: tokio_util::task::AbortOnDropHandle<()>,
}

#[derive(Clone, Debug)]
pub enum HostEvent {
    /// Everything was replaced; open threads refetch their windows.
    Reset,
    ImagesChanged,
    EngineInstall {
        engine: EngineId,
    },
    ThreadSummaryChanged {
        thread_id: ThreadId,
    },
    /// Projects, threads, runs, settings or engines changed.
    Changed,
    /// Metadata or pending interactions changed for one thread.
    ThreadChanged {
        thread_id: ThreadId,
    },
    Item(Arc<Item>),
    Live {
        thread_id: ThreadId,
        target: LiveTarget,
    },
}

impl EventEmitter<HostEvent> for HostData {}

impl HostData {
    /// Starts following the Forge behind `client`.
    pub fn new(id: HostId, name: String, client: ForgeClient, cx: &mut Context<Self>) -> Self {
        cx.on_release(|host, cx| {
            for (_, image) in host.images.drain() {
                if let BlobImage::Ready(image) = image {
                    cx.defer(move |cx| {
                        cx.drop_image(image, None);
                        // Other visible owners can retain these pixels after this owner
                        // releases them. Invalidate cached scenes before they reuse a tile.
                        cx.refresh_windows();
                    });
                }
            }
        })
        .detach();
        let (sender, mut receiver) = futures::channel::mpsc::channel(64);
        let retry = Arc::new(Notify::new());
        let (observed, observation) = watch::channel(None);
        let connection =
            connection::follow(client.clone(), sender, Arc::clone(&retry), observation);
        let follow = cx.spawn(async move |this, cx| {
            while let Some(update) = receiver.next().await {
                if this.update(cx, |host, cx| host.apply(update, cx)).is_err() {
                    break;
                }
            }
        });
        Self {
            id,
            name,
            client,
            status: ConnectionStatus::Connecting,
            forge_version: None,
            projects: BTreeMap::new(),
            threads: HashMap::new(),
            thread_pages: ThreadPages::default(),
            summary: None,
            summary_generation: 0,
            runs: HashMap::new(),
            settings: serde_json::Map::new(),
            prompt_load: ProjectionLoad::default(),
            catalog_load: ProjectionLoad::default(),
            global_prompt: None,
            prompt_revision: 0,
            catalog_revision: 0,
            skill_catalog: None,
            engines: BTreeMap::new(),
            account: None,
            repositories: HashMap::new(),
            reading: HashMap::new(),
            repository_order: std::collections::VecDeque::new(),
            generation: 0,
            waiting: HashMap::new(),
            live: HashMap::new(),
            live_epochs: HashMap::new(),
            live_epoch: 0,
            retry,
            observed,
            read_pending: BTreeMap::new(),
            read_observed: None,
            read_writer: None,
            images: HashMap::new(),
            image_tasks: HashMap::new(),
            retained_images: HashMap::new(),
            image_order: std::collections::VecDeque::new(),
            _follow: follow,
            _connection: connection,
        }
    }

    pub fn image(&self, hash: &artisan_api::BlobHash) -> Option<BlobImage> {
        self.images.get(hash).cloned().or_else(|| {
            self.retained_images
                .get(hash)
                .and_then(RetainedImage::upgrade)
        })
    }

    pub fn load_image(&mut self, hash: artisan_api::BlobHash, retry: bool, cx: &mut Context<Self>) {
        self.retained_images.retain(|_, result| result.alive());
        if retry && matches!(self.image(&hash), Some(BlobImage::Failed(_))) {
            self.retained_images.remove(&hash);
            self.images.remove(&hash);
            self.image_order.retain(|old| old != &hash);
        }
        if matches!(self.images.get(&hash), Some(BlobImage::Ready(_))) {
            self.image_order.retain(|old| old != &hash);
            self.image_order.push_back(hash);
            return;
        }
        if self.images.contains_key(&hash)
            || self
                .retained_images
                .get(&hash)
                .and_then(RetainedImage::upgrade)
                .is_some()
            || self.image_tasks.len() >= 4
        {
            return;
        }
        self.images.insert(hash.clone(), BlobImage::Loading);
        let client = self.client.clone();
        let renderer = cx.svg_renderer().clone();
        let key = hash.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = match client.blob(&hash).await {
                Ok(bytes) => {
                    cx.background_executor()
                        .spawn(async move {
                            editor_ui::markdown_renderer::decode_image_bytes(&bytes, renderer)
                        })
                        .await
                }
                Err(error) => Err(format!("{error:#}")),
            };
            let _ = this.update(cx, |this, cx| {
                this.image_tasks.remove(&hash);
                this.retained_images.remove(&hash);
                this.images.insert(
                    hash.clone(),
                    match result {
                        Ok(image) => BlobImage::Ready(image),
                        Err(error) => BlobImage::Failed(error.into()),
                    },
                );
                this.image_order.push_back(hash);
                while this.images.len() > 32
                    || this
                        .images
                        .values()
                        .filter_map(|image| match image {
                            BlobImage::Ready(image) => {
                                Some(editor_ui::markdown_renderer::image_resident_bytes(image))
                            }
                            _ => None,
                        })
                        .sum::<usize>()
                        > 64 * 1024 * 1024
                {
                    let Some(oldest) = this.image_order.pop_front() else {
                        break;
                    };
                    match this.images.remove(&oldest) {
                        Some(BlobImage::Ready(image)) => {
                            this.retained_images
                                .insert(oldest, RetainedImage::Ready(Arc::downgrade(&image)));
                            cx.defer(move |cx| {
                                cx.drop_image(image, None);
                                // Retained scenes need fresh GPU tiles for these shared pixels.
                                cx.refresh_windows();
                            });
                        }
                        Some(BlobImage::Failed(error)) => {
                            this.retained_images
                                .insert(oldest, RetainedImage::Failed(Arc::downgrade(&error)));
                        }
                        _ => {}
                    }
                }
                this.retained_images.retain(|_, result| result.alive());
                cx.emit(HostEvent::ImagesChanged);
                cx.notify();
            });
        });
        self.image_tasks.insert(key, task);
    }

    /// Coalesce actual visible activity into one owned, retryable writer.
    pub fn acknowledge_read(
        &mut self,
        thread: ThreadId,
        observed_seq: artisan_api::Seq,
        cx: &mut Context<Self>,
    ) {
        if self.observed.borrow().as_ref() != Some(&thread) {
            return;
        }
        let Some(summary) = self.threads.get(&thread) else {
            return;
        };
        let seq = observed_seq.min(summary.activity_seq);
        // Painting the same content again must not undo an explicit Mark Unread.
        // Revisiting a thread starts a new observation; new activity may also advance it.
        if !new_read_evidence(self.read_observed, seq) {
            return;
        }
        // An offline connection can accumulate acknowledgements while navigating.
        // Keep admission bounded and retry from the next visible frame when it drains.
        if !self.read_pending.contains_key(&thread) && self.read_pending.len() >= 128 {
            return;
        }
        self.read_observed = Some(seq);
        if seq <= summary.read_seq && !summary.marked_unread {
            return;
        }
        self.read_pending
            .entry(thread)
            .and_modify(|pending| *pending = (*pending).max(seq))
            .or_insert(seq);
        if self.read_writer.is_some() {
            return;
        }
        let client = self.client.clone();
        self.read_writer = Some(cx.spawn(async move |this, cx| {
            loop {
                let next = this.update(cx, |host, _| {
                    host.read_pending
                        .first_key_value()
                        .map(|(id, seq)| (id.clone(), *seq))
                });
                let Ok(Some((id, seq))) = next else {
                    let _ = this.update(cx, |host, _| host.read_writer = None);
                    break;
                };
                let result = client
                    .update_thread(
                        &id,
                        &artisan_api::UpdateThread {
                            read_seq: Some(seq),
                            ..Default::default()
                        },
                    )
                    .await;
                match result {
                    Ok(thread) => {
                        if this
                            .update(cx, |host, cx| {
                                if host
                                    .read_pending
                                    .get(&id)
                                    .is_some_and(|pending| *pending <= seq)
                                {
                                    host.read_pending.remove(&id);
                                }
                                if host
                                    .threads
                                    .get(&id)
                                    .is_none_or(|old| old.seq <= thread.seq)
                                {
                                    host.remember_thread(thread);
                                    cx.emit(HostEvent::ThreadChanged {
                                        thread_id: id.clone(),
                                    });
                                    cx.notify();
                                }
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(error) => {
                        tracing::debug!(%id, "read acknowledgement will retry: {error:#}");
                        cx.background_executor()
                            .timer(std::time::Duration::from_secs(1))
                            .await;
                    }
                }
            }
        }));
    }

    fn apply(&mut self, update: Update, cx: &mut Context<Self>) {
        match update {
            Update::Status(status) => {
                if self.status == status {
                    return;
                }
                self.status = status;
                self.summary_connection_changed(self.is_live(), cx);
                if !self.is_live() {
                    self.cancel_projection_reads();
                }
                cx.emit(HostEvent::Changed);
            }
            Update::State(state) => {
                self.generation = self.generation.wrapping_add(1);
                self.reading.clear();
                self.repositories.clear();
                self.repository_order.clear();
                let state = *state;
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
                let observed = self.observed.borrow().clone();
                if let Some(id) = observed {
                    self.ensure_thread(&id, cx);
                }
                self.runs.clear();
                for run in state.runs {
                    self.remember_run(run);
                }
                self.settings = state.settings;
                self.invalidate_prompt();
                self.invalidate_catalog();
                self.account = state.account;
                self.engines = state
                    .engines
                    .into_iter()
                    .map(|status| (status.engine, status))
                    .collect();
                self.waiting.clear();
                for item in state.waiting {
                    self.waiting
                        .entry(item.thread_id.clone())
                        .or_default()
                        .insert(item.id.clone());
                }
                self.live.clear();
                self.live_epochs.clear();
                self.prune_threads();
                self.reset_thread_summary(cx);
                cx.emit(HostEvent::Reset);
                cx.emit(HostEvent::Changed);
            }
            Update::Event(envelope, _permit) => self.apply_event(envelope.event, envelope.seq, cx),
        }
        cx.notify();
    }

    fn apply_event(&mut self, event: Event, seq: Option<artisan_api::Seq>, cx: &mut Context<Self>) {
        self.invalidate_thread_summary(&event, seq, cx);
        match event {
            Event::Hello { forge_version, .. } => self.forge_version = Some(forge_version),
            Event::Project { project } => {
                // Metadata replacement cancels the old path's observation before publishing.
                self.reading.remove(&project.id);
                self.repositories.remove(&project.id);
                self.repository_order.retain(|id| id != &project.id);
                if project.removed {
                    self.projects.remove(&project.id);
                } else {
                    self.projects.insert(project.id.clone(), project);
                }
                cx.emit(HostEvent::Changed);
            }
            Event::Thread { thread } => {
                let thread_id = thread.id.clone();
                self.remember_thread(thread);
                cx.emit(HostEvent::ThreadChanged { thread_id });
            }
            Event::Run { run } => {
                if !run.status.is_live() {
                    self.live.remove(&run.thread_id);
                    self.live_epochs.remove(&run.thread_id);
                }
                let thread_id = run.thread_id.clone();
                self.remember_run(run);
                if self.is_working(&thread_id) && !self.threads.contains_key(&thread_id) {
                    self.ensure_thread(&thread_id, cx);
                }
                cx.emit(HostEvent::ThreadChanged { thread_id });
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
                cx.emit(HostEvent::Changed);
            }
            Event::EngineInstall { engine, install } => {
                if let Some(status) = self.engines.get_mut(&engine)
                    && status.install != install
                {
                    status.install = install;
                    cx.emit(HostEvent::EngineInstall { engine });
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
                cx.emit(HostEvent::Changed);
            }
            Event::Item { item } => {
                if item.status.is_terminal() {
                    if let Some(epochs) = self.live_epochs.get_mut(&item.thread_id) {
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
                            epochs.remove(&target);
                        }
                    }
                }
                let waiting = self.waiting.entry(item.thread_id.clone()).or_default();
                let changed = if item.status == Status::AwaitingUser {
                    waiting.insert(item.id.clone())
                } else {
                    waiting.remove(&item.id)
                };
                if let Some(live) = self.live.get_mut(&item.thread_id)
                    && item.status.is_terminal()
                {
                    live.remove(&LiveTarget::Text {
                        item_id: item.id.clone(),
                    });
                    live.remove(&LiveTarget::Shell {
                        item_id: item.id.clone(),
                    });
                    live.remove(&LiveTarget::Progress {
                        item_id: item.id.clone(),
                    });
                }
                if changed {
                    cx.emit(HostEvent::ThreadChanged {
                        thread_id: item.thread_id.clone(),
                    });
                }
                cx.emit(HostEvent::Item(Arc::new(item)));
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
                        self.ensure_thread(&thread_id, cx);
                    }
                    cx.emit(HostEvent::ThreadChanged { thread_id });
                }
            }
            Event::Live {
                thread_id,
                target,
                text,
                append,
                coverage,
            } => {
                if !append
                    || !self
                        .live
                        .get(&thread_id)
                        .is_some_and(|targets| targets.contains_key(&target))
                {
                    self.live_epoch = self
                        .live_epoch
                        .checked_add(1)
                        .expect("live epoch exhausted");
                    self.live_epochs
                        .entry(thread_id.clone())
                        .or_default()
                        .insert(target.clone(), (self.live_epoch, coverage));
                }
                if let Some(value) = self
                    .live_epochs
                    .get_mut(&thread_id)
                    .and_then(|targets| targets.get_mut(&target))
                {
                    value.1 = coverage;
                }
                let buffer = self
                    .live
                    .entry(thread_id.clone())
                    .or_default()
                    .entry(target.clone())
                    .or_default();
                if append {
                    let end = buffer.len_chars();
                    Arc::make_mut(buffer).insert(end, &text);
                } else {
                    *buffer = Arc::new(Rope::from_str(&text));
                }
                cx.emit(HostEvent::Live { thread_id, target });
            }
            Event::Reset => {}
        }
    }

    fn remember_run(&mut self, run: Run) {
        match self.runs.get(&run.thread_id) {
            Some(known) if known.id != run.id && known.started_at > run.started_at => {}
            _ => {
                self.runs.insert(run.thread_id.clone(), run);
            }
        }
    }

    #[must_use]
    pub fn is_working(&self, thread: &ThreadId) -> bool {
        self.runs
            .get(thread)
            .is_some_and(|run| run.status.is_live())
    }

    #[must_use]
    pub fn is_waiting(&self, thread: &ThreadId) -> bool {
        self.waiting
            .get(thread)
            .is_some_and(|items| !items.is_empty())
    }

    fn invalidate_prompt(&mut self) {
        self.prompt_load = ProjectionLoad::default();
        self.global_prompt = None;
        self.prompt_revision = self.prompt_revision.saturating_add(1);
    }
    pub fn invalidate_catalog(&mut self) {
        self.catalog_load = ProjectionLoad::default();
        self.skill_catalog = None;
        self.catalog_revision = self.catalog_revision.saturating_add(1);
    }

    pub fn remember_prompt(&mut self, prompt: artisan_api::GlobalPrompt, cx: &mut Context<Self>) {
        let expected = self
            .settings
            .get("global_prompt_version")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        if prompt.version < expected {
            return;
        }
        self.prompt_load = ProjectionLoad::default();
        self.global_prompt = Some(Arc::new(prompt));
        cx.emit(HostEvent::Changed);
        cx.notify();
    }

    pub fn remember_skill_catalog(
        &mut self,
        catalog: Vec<artisan_api::CatalogSkill>,
        cx: &mut Context<Self>,
    ) {
        self.catalog_load = ProjectionLoad::default();
        self.skill_catalog = Some(Arc::new(catalog));
        cx.emit(HostEvent::Changed);
        cx.notify();
    }

    /// A connection carries full live content only for its visible transcript.
    pub fn observe_thread(&mut self, thread: Option<ThreadId>, cx: &mut Context<Self>) {
        if *self.observed.borrow() == thread {
            return;
        }
        self.live.retain(|id, _| Some(id) == thread.as_ref());
        self.live_epochs.retain(|id, _| Some(id) == thread.as_ref());
        self.observed.send_replace(thread);
        self.read_observed = None;
        self.reset_thread_summary(cx);
    }

    /// Live text for a target, if the run is still producing it.
    #[must_use]
    pub fn live(&self, thread: &ThreadId, target: &LiveTarget) -> Option<&Rope> {
        self.live.get(thread)?.get(target).map(AsRef::as_ref)
    }

    pub fn live_snapshot(&self, thread: &ThreadId, target: &LiveTarget) -> Option<LiveSnapshot> {
        Some(LiveSnapshot {
            epoch: self.live_epochs.get(thread)?.get(target)?.0,
            coverage: self.live_epochs.get(thread)?.get(target)?.1,
            text: self.live.get(thread)?.get(target)?.clone(),
        })
    }

    /// Every live target of a thread (reasoning, shells, streaming text).
    pub fn live_targets(
        &self,
        thread: &ThreadId,
    ) -> impl Iterator<Item = (&LiveTarget, &Arc<Rope>)> {
        self.live
            .get(thread)
            .into_iter()
            .flat_map(|targets| targets.iter().map(|(target, text)| (target, text)))
    }

    /// Threads of a project (or all), newest activity first.
    #[must_use]
    pub fn threads_by_activity(&self, project: Option<&ProjectId>) -> Vec<&Thread> {
        let mut threads: Vec<&Thread> = self
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

    #[must_use]
    pub fn setting(&self, key: &str) -> Option<&serde_json::Value> {
        self.settings.get(key)
    }

    /// What git said about a project; `None` until read or outside a
    /// repository.
    #[must_use]
    pub fn repository(&self, project: &ProjectId) -> Option<&ProjectRepository> {
        self.repositories.get(project)?.value.as_ref()
    }

    /// Coalesces visible repository reads for two seconds. Failures retain the
    /// last successful observation and back off for five seconds.
    pub fn load_repository(&mut self, project: &ProjectId, refresh: bool, cx: &mut Context<Self>) {
        let Some(project_seq) = self.projects.get(project).map(|project| project.seq) else {
            return;
        };
        if !self.is_live() || self.reading.contains_key(project) || self.reading.len() >= 4 {
            return;
        }
        if self.repositories.contains_key(project) {
            self.repository_order.retain(|id| id != project);
            self.repository_order.push_back(project.clone());
        }
        if !refresh
            && self
                .repositories
                .get(project)
                .is_some_and(RepositoryEntry::fresh)
        {
            return;
        }
        let client = self.client.clone();
        let project = project.clone();
        let key = project.clone();
        let generation = self.generation;
        let task = cx.spawn(async move |this, cx| {
            let result = client.project_repository(&project).await;
            let _ = this.update(cx, |host, cx| {
                host.finish_repository(project, generation, project_seq, result, cx);
            });
        });
        self.reading.insert(key, task);
    }

    fn finish_repository(
        &mut self,
        project: ProjectId,
        generation: u64,
        project_seq: artisan_api::Seq,
        result: anyhow::Result<Option<ProjectRepository>>,
        cx: &mut Context<Self>,
    ) {
        if self.generation != generation
            || self
                .projects
                .get(&project)
                .is_none_or(|current| current.seq != project_seq)
        {
            return;
        }
        self.reading.remove(&project);
        let failed = result.is_err();
        let value = match result {
            Ok(repository) => repository,
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
        cx.emit(HostEvent::Changed);
        cx.notify();
    }

    /// Retries an offline connection now rather than after its backoff.
    pub fn retry_now(&self) {
        self.retry.notify_one();
    }

    #[must_use]
    pub fn is_live(&self) -> bool {
        self.status == ConnectionStatus::Live
    }
}

/// Only new visible evidence can clear an explicit unread mark during an observation.
fn new_read_evidence(previous: Option<artisan_api::Seq>, next: artisan_api::Seq) -> bool {
    previous.is_none_or(|previous| next > previous)
}

#[cfg(test)]
mod read_evidence_tests {
    use super::new_read_evidence;

    #[test]
    fn repaint_cannot_clear_manual_unread_but_new_activity_and_revisit_can() {
        assert!(new_read_evidence(None, 10));
        // The unread preference may change after this paint, without new content.
        assert!(!new_read_evidence(Some(10), 10));
        assert!(!new_read_evidence(Some(10), 9));
        assert!(new_read_evidence(Some(10), 11));
        // Changing observations resets the evidence, allowing an intentional revisit.
        assert!(new_read_evidence(None, 10));
    }
}

#[cfg(test)]
mod live_rope_tests {
    use super::*;
    #[test]
    fn append_preserves_snapshot_and_shares_unchanged_prefix_storage() {
        let mut live = Arc::new(Rope::from_str(
            &"prefix🙂
"
            .repeat(8192),
        ));
        let snapshot = live.clone();
        let before = snapshot.len_bytes();
        let prefix = snapshot.chunks().next().unwrap().as_ptr();
        let end = live.len_chars();
        Arc::make_mut(&mut live).insert(end, "new delta🙂");
        assert_eq!(snapshot.len_bytes(), before);
        assert_eq!(live.len_bytes(), before + "new delta🙂".len());
        assert_eq!(
            prefix,
            live.chunks().next().unwrap().as_ptr(),
            "a delta cannot copy the unchanged prefix"
        );
        assert_eq!(
            live.slice(live.len_chars() - 10..).to_string(),
            "new delta🙂"
        );
    }
}

#[cfg(test)]
mod install_delta_client_tests {
    use super::*;
    use gpui::{AppContext, TestAppContext};
    #[gpui::test]
    fn progress_replay_preserves_catalog_snapshots_and_projection_revisions(
        cx: &mut TestAppContext,
    ) {
        let identity = artisan_api::tls::Identity::generate("install-delta-test").unwrap();
        let fingerprint =
            artisan_api::tls::fingerprint(identity.certificate_der().unwrap().as_ref());
        let client = ForgeClient::new("127.0.0.1:1", &fingerprint, &identity).unwrap();
        let host = cx.new(|cx| {
            let mut host = HostData::new(
                HostId("install-delta-test".into()),
                "Test".into(),
                client,
                cx,
            );
            host._connection.abort();
            host._follow = Task::ready(());
            host.status = ConnectionStatus::Live;
            host
        });
        host.update(cx,|host,cx| {
            let engine=EngineId::Codex;
            let status:EngineStatus=serde_json::from_value(serde_json::json!({
                "engine":"codex","install":{"state":"downloading","version":"2.0.0","received":0,"total":1000},
                "sign_in":{"state":"signed_in","account":"fixture@example.invalid"},
                "models":[{"id":"fixture","name":"Fixture","default_effort":null,"context_window":200000}],
                "default_model":"fixture","usage":null,
                "versions":[{"version":"2.0.0","installed":false,"current":false}]
            })).unwrap();
            host.apply_event(Event::Engine {status},None,cx);
            let models=host.engines[&engine].models.clone();let versions=host.engines[&engine].versions.clone();
            let skills=Arc::new(Vec::new());host.skill_catalog=Some(skills.clone());
            let prompt=Arc::new(artisan_api::GlobalPrompt {version:1,text:"saved prompt".into(),candidates:vec![],engines:vec![]});
            host.global_prompt=Some(prompt.clone());
            let revisions=(host.prompt_revision,host.catalog_revision);
            let install=artisan_api::InstallState::Downloading {version:"2.0.0".into(),received:512,total:Some(1000)};
            let wire=serde_json::to_string(&Event::EngineInstall {engine,install:install.clone()}).unwrap();
            host.apply_event(serde_json::from_str(&wire).unwrap(),None,cx);
            assert_eq!(host.engines[&engine].install,install);
            assert!(Arc::ptr_eq(&models,&host.engines[&engine].models));
            assert!(Arc::ptr_eq(&versions,&host.engines[&engine].versions));
            assert!(Arc::ptr_eq(&skills,host.skill_catalog.as_ref().unwrap()));
            assert!(Arc::ptr_eq(&prompt,host.global_prompt.as_ref().unwrap()));
            assert_eq!((host.prompt_revision,host.catalog_revision),revisions);
            assert_eq!(host.engines[&engine].sign_in,artisan_api::SignInState::SignedIn {account:Some("fixture@example.invalid".into())});
        });
    }
}

#[cfg(test)]
mod repository_tests {
    use super::*;
    use gpui::{AppContext, Entity, TestAppContext};
    use std::time::{Duration, Instant};

    fn host(cx: &mut TestAppContext) -> Entity<HostData> {
        let identity = artisan_api::tls::Identity::generate("repository-test").unwrap();
        let fingerprint =
            artisan_api::tls::fingerprint(identity.certificate_der().unwrap().as_ref());
        let client = ForgeClient::new("127.0.0.1:1", &fingerprint, &identity).unwrap();
        cx.new(|cx| {
            let mut host =
                HostData::new(HostId("repository-test".into()), "Test".into(), client, cx);
            host._connection.abort();
            host._follow = Task::ready(());
            host.status = ConnectionStatus::Live;
            host
        })
    }
    fn project(seq: artisan_api::Seq) -> Project {
        Project {
            id: ProjectId::from("test-project"),
            path: "/project".into(),
            name: "Project".into(),
            seq,
            created_at: 0,
            removed: false,
        }
    }
    fn repository(branch: &str) -> ProjectRepository {
        ProjectRepository {
            branch: Some(branch.into()),
            host: None,
            web_url: None,
            worktree: None,
        }
    }
    #[gpui::test]
    fn metadata_change_cancels_repository_read_and_rejects_its_late_result(
        cx: &mut TestAppContext,
    ) {
        let host = host(cx);
        host.update(cx, |host, cx| {
            let old = project(1);
            let id = old.id.clone();
            host.apply_event(Event::Project { project: old }, Some(1), cx);
            host.finish_repository(
                id.clone(),
                host.generation,
                1,
                Ok(Some(repository("old"))),
                cx,
            );
            host.load_repository(&id, true, cx);
            assert!(host.reading.contains_key(&id));
            host.apply_event(
                Event::Project {
                    project: project(2),
                },
                Some(2),
                cx,
            );
            assert!(!host.reading.contains_key(&id));
            assert!(host.repository(&id).is_none());
            host.finish_repository(
                id.clone(),
                host.generation,
                1,
                Ok(Some(repository("late-old"))),
                cx,
            );
            assert!(host.repository(&id).is_none());
            host.finish_repository(
                id.clone(),
                host.generation,
                2,
                Ok(Some(repository("current"))),
                cx,
            );
            assert_eq!(
                host.repository(&id).unwrap().branch.as_deref(),
                Some("current")
            );
        });
    }
    #[gpui::test]
    fn expired_repository_reads_coalesce_and_failure_preserves_last_observation(
        cx: &mut TestAppContext,
    ) {
        let host = host(cx);
        host.update(cx, |host, cx| {
            let project = project(1);
            let id = project.id.clone();
            host.apply_event(Event::Project { project }, Some(1), cx);
            host.finish_repository(
                id.clone(),
                host.generation,
                1,
                Ok(Some(repository("main"))),
                cx,
            );
            host.load_repository(&id, false, cx);
            assert!(
                host.reading.is_empty(),
                "fresh observation avoids another read"
            );
            host.repositories.get_mut(&id).unwrap().checked =
                Instant::now() - Duration::from_secs(3);
            host.load_repository(&id, false, cx);
            host.load_repository(&id, false, cx);
            assert_eq!(host.reading.len(), 1);
            host.finish_repository(
                id.clone(),
                host.generation,
                1,
                Err(anyhow::anyhow!("refused")),
                cx,
            );
            assert_eq!(
                host.repository(&id).unwrap().branch.as_deref(),
                Some("main")
            );
            host.load_repository(&id, false, cx);
            assert!(host.reading.is_empty(), "failed refresh has a backoff");
            assert_eq!(
                host.repository_order.len(),
                1,
                "refresh does not accumulate LRU entries"
            );
        });
    }
}

#[cfg(test)]
mod image_tests;
