//! What one Forge has told us: projects, threads, runs, settings, engines,
//! items waiting on the user and live text. Updated only by its connection;
//! every section reads from here. Caches built from Forge reads (images,
//! repositories, thread pages, the observed thread's summary) and the
//! read-acknowledgement writer are separate pieces in their own modules,
//! each owning its tasks.

mod images;
mod projections;
mod read_acks;
mod repositories;
mod thread_pages;
mod thread_summary;

pub use images::{BlobImage, ImageDecoder};
pub use projections::ProjectionLoad;
pub use thread_pages::{ThreadPage, ThreadPageDirection};
pub use thread_summary::ThreadSummary;

use crate::client::ForgeClient;
use crate::connection::{self, ConnectionStatus, Update};
use crate::hosts::HostId;
use artisan_api::{
    Account, EngineId, EngineStatus, Event, Item, ItemId, LiveCoverage, LiveTarget, Project,
    ProjectId, Run, Status, Thread, ThreadId,
};
use futures::StreamExt as _;
use gpui::{Context, EventEmitter, Task};
use images::ImageCache;
use read_acks::ReadAcks;
use repositories::RepositoryCache;
use ropey::Rope;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use thread_pages::ThreadPages;
use thread_summary::ObservedSummary;
use tokio::sync::{Notify, watch};

/// One live text stream as last seen.
#[derive(Clone)]
pub struct LiveSnapshot {
    /// Which stream this is: a target whose text was replaced rather than
    /// appended to gets a new one.
    pub epoch: u64,
    pub text: Arc<Rope>,
    pub coverage: LiveCoverage,
}

pub struct HostData {
    pub id: HostId,
    pub name: String,
    pub client: ForgeClient,
    pub status: ConnectionStatus,
    pub forge_version: Option<String>,
    pub projects: BTreeMap<ProjectId, Project>,
    pub threads: HashMap<ThreadId, Thread>,
    /// The latest run of each thread.
    pub runs: HashMap<ThreadId, Run>,
    pub settings: serde_json::Map<String, serde_json::Value>,
    pub engines: BTreeMap<EngineId, EngineStatus>,
    /// The account Forge runs as.
    pub account: Option<Account>,
    /// Lazy canonical projections; forms keep only their unsaved edits.
    prompt_load: ProjectionLoad,
    catalog_load: ProjectionLoad,
    pub global_prompt: Option<Arc<artisan_api::GlobalPrompt>>,
    pub prompt_revision: u64,
    pub catalog_revision: u64,
    pub skill_catalog: Option<Arc<Vec<artisan_api::CatalogSkill>>>,
    /// Items waiting on the user, by thread.
    waiting: HashMap<ThreadId, BTreeSet<ItemId>>,
    /// Memory-only text of live runs, for the observed thread.
    live: HashMap<ThreadId, HashMap<LiveTarget, LiveSnapshot>>,
    /// The last stream number handed out.
    live_streams: u64,
    /// The thread on screen: its live text and summary are followed.
    observed: watch::Sender<Option<ThreadId>>,
    summary: Option<ObservedSummary>,
    thread_pages: ThreadPages,
    images: ImageCache,
    repositories: RepositoryCache,
    read_acks: ReadAcks,
    /// Cuts the connection's wait before its next attempt short.
    retry: Arc<Notify>,
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
        cx.on_release(|host, cx| host.images.release(cx)).detach();
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
            runs: HashMap::new(),
            settings: serde_json::Map::new(),
            engines: BTreeMap::new(),
            account: None,
            prompt_load: ProjectionLoad::default(),
            catalog_load: ProjectionLoad::default(),
            global_prompt: None,
            prompt_revision: 0,
            catalog_revision: 0,
            skill_catalog: None,
            waiting: HashMap::new(),
            live: HashMap::new(),
            live_streams: 0,
            observed,
            summary: None,
            thread_pages: ThreadPages::default(),
            images: ImageCache::default(),
            repositories: RepositoryCache::default(),
            read_acks: ReadAcks::default(),
            retry,
            _follow: follow,
            _connection: connection,
        }
    }

    pub fn image(&self, hash: &artisan_api::BlobHash) -> Option<BlobImage> {
        self.images.get(hash)
    }

    /// Loads a blob image unless it is kept, loading or still shown; only
    /// `retry` loads a failed one again.
    pub fn load_image(&mut self, hash: artisan_api::BlobHash, retry: bool, cx: &mut Context<Self>) {
        self.images.load(hash, retry, &self.client, cx);
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
                // Everything read from the old state goes, with its reads.
                self.repositories.clear();
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
                // New metadata cancels what was read about the old path.
                self.repositories.forget(&project.id);
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
        self.observed.send_replace(thread);
        self.read_acks.observation_changed();
        self.reset_thread_summary(cx);
    }

    /// Live text for a target, if the run is still producing it.
    #[must_use]
    pub fn live(&self, thread: &ThreadId, target: &LiveTarget) -> Option<&Rope> {
        Some(&self.live.get(thread)?.get(target)?.text)
    }

    pub fn live_snapshot(&self, thread: &ThreadId, target: &LiveTarget) -> Option<LiveSnapshot> {
        self.live.get(thread)?.get(target).cloned()
    }

    /// Every live target of a thread (reasoning, shells, streaming text).
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

    /// Retries an offline connection now rather than after its backoff.
    pub fn retry_now(&self) {
        self.retry.notify_one();
    }

    #[must_use]
    pub fn is_live(&self) -> bool {
        self.status == ConnectionStatus::Live
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
mod image_tests;
