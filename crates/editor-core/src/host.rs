//! What one Forge has told us. Updated only by its connection; every
//! section reads from here.

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
use std::sync::Arc;
use tokio::sync::Notify;

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
    /// What git said about each project, once asked (see
    /// [`Self::load_repository`]); `None` outside a repository.
    repositories: HashMap<ProjectId, Option<ProjectRepository>>,
    /// Repository reads in flight.
    reading: BTreeSet<ProjectId>,
    /// Items waiting on the user, by thread.
    waiting: HashMap<ThreadId, BTreeSet<ItemId>>,
    /// Memory-only text of live runs.
    live: HashMap<ThreadId, HashMap<LiveTarget, String>>,
    /// Cuts the connection's wait before its next attempt short.
    retry: Arc<Notify>,
    _follow: Task<()>,
}

#[derive(Clone, Debug)]
pub enum HostEvent {
    /// Everything was replaced; open threads refetch their windows.
    Reset,
    /// Projects, threads, runs, settings or engines changed.
    Changed,
    Item(Box<Item>),
    Live {
        thread_id: ThreadId,
        target: LiveTarget,
    },
}

impl EventEmitter<HostEvent> for HostData {}

impl HostData {
    /// Starts following the Forge behind `client`.
    pub fn new(id: HostId, name: String, client: ForgeClient, cx: &mut Context<Self>) -> Self {
        let (sender, mut receiver) = futures::channel::mpsc::unbounded();
        let retry = Arc::new(Notify::new());
        connection::follow(client.clone(), sender, Arc::clone(&retry));
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
            repositories: HashMap::new(),
            reading: BTreeSet::new(),
            waiting: HashMap::new(),
            live: HashMap::new(),
            retry,
            _follow: follow,
        }
    }

    fn apply(&mut self, update: Update, cx: &mut Context<Self>) {
        match update {
            Update::Status(status) => {
                self.status = status;
                cx.emit(HostEvent::Changed);
            }
            Update::State(state) => {
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
                self.runs.clear();
                for run in state.runs {
                    self.remember_run(run);
                }
                self.settings = state.settings;
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
                cx.emit(HostEvent::Reset);
                cx.emit(HostEvent::Changed);
            }
            Update::Event(envelope) => self.apply_event(envelope.event, cx),
        }
        cx.notify();
    }

    fn apply_event(&mut self, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::Hello { forge_version, .. } => self.forge_version = Some(forge_version),
            Event::Project { project } => {
                if project.removed {
                    self.projects.remove(&project.id);
                } else {
                    self.projects.insert(project.id.clone(), project);
                }
                cx.emit(HostEvent::Changed);
            }
            Event::Thread { thread } => {
                self.threads.insert(thread.id.clone(), thread);
                cx.emit(HostEvent::Changed);
            }
            Event::Run { run } => {
                if !run.status.is_live() {
                    self.live.remove(&run.thread_id);
                }
                self.remember_run(run);
                cx.emit(HostEvent::Changed);
            }
            Event::Setting { key, value } => {
                self.settings.insert(key, value);
                cx.emit(HostEvent::Changed);
            }
            Event::Engine { status } => {
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
                    cx.emit(HostEvent::Changed);
                }
                cx.emit(HostEvent::Item(Box::new(item)));
            }
            Event::Live {
                thread_id,
                target,
                text,
                append,
            } => {
                let buffer = self
                    .live
                    .entry(thread_id.clone())
                    .or_default()
                    .entry(target.clone())
                    .or_default();
                if append {
                    buffer.push_str(&text);
                } else {
                    *buffer = text;
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

    /// Live text for a target, if the run is still producing it.
    #[must_use]
    pub fn live(&self, thread: &ThreadId, target: &LiveTarget) -> Option<&str> {
        self.live.get(thread)?.get(target).map(String::as_str)
    }

    /// Every live target of a thread (reasoning, shells, streaming text).
    pub fn live_targets(&self, thread: &ThreadId) -> impl Iterator<Item = (&LiveTarget, &str)> {
        self.live
            .get(thread)
            .into_iter()
            .flat_map(|targets| targets.iter().map(|(target, text)| (target, text.as_str())))
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
        self.repositories.get(project)?.as_ref()
    }

    /// Asks Forge for a project's branch and remote unless it already
    /// answered; `refresh` asks again (the branch may have moved).
    pub fn load_repository(&mut self, project: &ProjectId, refresh: bool, cx: &mut Context<Self>) {
        if self.reading.contains(project) || (!refresh && self.repositories.contains_key(project)) {
            return;
        }
        self.reading.insert(project.clone());
        let client = self.client.clone();
        let project = project.clone();
        cx.spawn(async move |this, cx| {
            let read = client.project_repository(&project).await;
            this.update(cx, |host, cx| {
                host.reading.remove(&project);
                let repository = match read {
                    Ok(repository) => repository,
                    Err(error) => {
                        tracing::debug!(%project, "no project repository: {error:#}");
                        None
                    }
                };
                let changed = host.repositories.get(&project) != Some(&repository);
                host.repositories.insert(project, repository);
                if changed {
                    cx.emit(HostEvent::Changed);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
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
