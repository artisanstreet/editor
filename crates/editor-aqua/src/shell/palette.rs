use crate::core::{ForgeClient, HostId, HostInstance, Screen, SettingsPage, SettingsSection};
use artisan_api::{Project, ProjectId, Thread, ThreadId, ThreadsQuery, Timestamp};
use futures::StreamExt as _;
use std::{
    collections::{BTreeMap, HashMap},
    num::NonZeroUsize,
    time::Duration,
};
use tokio_util::task::AbortOnDropHandle;

const SHOWN: usize = 12;
const THREAD_FLOOR: usize = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchProblem {
    NoForge,
    Unreachable(usize),
}

type Recency = (Timestamp, HostId, ThreadId);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    label: String,
    detail: String,
    screen: Screen,
}

#[derive(Debug)]
pub struct HostSnapshot<'a> {
    pub(crate) id: &'a HostId,
    pub(crate) name: &'a str,
    pub(crate) instance: HostInstance,
    pub(crate) threads: &'a HashMap<ThreadId, Thread>,
    pub(crate) projects: &'a BTreeMap<ProjectId, Project>,
}

pub(crate) struct SearchHost {
    pub id: HostId,
    pub name: String,
    pub client: ForgeClient,
    pub instance: HostInstance,
}

struct SearchUpdate {
    generation: u64,
    host: HostId,
    instance: HostInstance,
    name: String,
    result: Result<Vec<Thread>, crate::core::client::CallError>,
}

struct SearchResult {
    host: HostId,
    instance: HostInstance,
    name: String,
    result: Result<Vec<Thread>, crate::core::client::CallError>,
}

pub struct Palette {
    pub query: String,
    highlighted: usize,
    entries: Vec<Entry>,
    search_error: Option<SearchProblem>,
    search: Option<Vec<SearchResult>>,
    pending: usize,
    generation: u64,
    request: Option<AbortOnDropHandle<()>>,
    delivery: crate::core::delivery::Delivery<SearchUpdate>,
}

impl std::fmt::Debug for Palette {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("Palette").finish_non_exhaustive()
    }
}

fn insert_thread(
    entries: &mut Vec<(Recency, Entry)>,
    thread: &Thread,
    host: &HostId,
    detail: &str,
    limit: usize,
) {
    let key = (thread.activity_at, host.clone(), thread.id.clone());
    let index = entries.partition_point(|(existing, _)| existing > &key);
    if index >= limit {
        return;
    }
    entries.insert(
        index,
        (
            key,
            Entry {
                label: thread
                    .title
                    .clone()
                    .unwrap_or_else(|| "Untitled thread".into()),
                detail: detail.to_owned(),
                screen: Screen::Thread {
                    host: host.clone(),
                    thread: thread.id.clone(),
                },
            },
        ),
    );
    entries.truncate(limit);
}

fn allocate(commands: Vec<Entry>, threads: Vec<Entry>) -> Vec<Entry> {
    let thread_room = threads
        .len()
        .min(SHOWN.saturating_sub(commands.len()).max(THREAD_FLOOR));
    commands
        .into_iter()
        .take(SHOWN - thread_room)
        .chain(threads.into_iter().take(thread_room))
        .collect()
}

const SEARCH_MAILBOX: NonZeroUsize = NonZeroUsize::new(64).expect("invariant: 64 is nonzero");

impl Default for Palette {
    fn default() -> Self {
        Self {
            query: String::new(),
            highlighted: 0,
            entries: Vec::new(),
            search_error: None,
            search: None,
            pending: 0,
            generation: 0,
            request: None,
            delivery: crate::core::delivery::Delivery::new(SEARCH_MAILBOX),
        }
    }
}

impl Palette {
    pub(crate) fn wake_mailbox(&self) -> Option<&aqua::mailbox::Mailbox<()>> {
        self.delivery.wake_mailbox()
    }

    pub(crate) fn start_search(&mut self, query: String, hosts: Vec<SearchHost>) {
        self.query = query;
        self.generation = self.generation.wrapping_add(1);
        self.request = None;
        self.search = None;
        self.search_error = None;
        self.pending = 0;
        let query = self.query.trim().to_owned();
        if query.is_empty() {
            return;
        }
        if hosts.is_empty() {
            self.search_error = Some(SearchProblem::NoForge);
            return;
        }
        self.pending = hosts.len();
        let generation = self.generation;
        let sender = self.delivery.sender();
        self.request = Some(AbortOnDropHandle::new(
            crate::core::runtime::runtime().spawn(async move {
                tokio::time::sleep(Duration::from_millis(150)).await;
                let mut responses = futures::stream::iter(hosts.into_iter().map(|host| {
                    let query = query.clone();
                    async move {
                        let result = host
                            .client
                            .threads(&ThreadsQuery {
                                query: Some(query),
                                limit: Some(12),
                                ..Default::default()
                            })
                            .await;
                        (host.id, host.instance, host.name, result)
                    }
                }))
                .buffer_unordered(4);
                while let Some((host, instance, name, result)) = responses.next().await {
                    let update = SearchUpdate {
                        generation,
                        host,
                        instance,
                        name,
                        result: result.map(|page| page.threads),
                    };
                    if sender.send(update).await.is_err() {
                        return;
                    }
                }
            }),
        ));
    }

    pub(crate) fn drain(&mut self, core: &crate::core::Core) -> bool {
        let mut changed = false;
        for update in self.delivery.drain() {
            if update.generation != self.generation
                || core
                    .host_if_instance(&update.host, update.instance)
                    .is_none()
            {
                continue;
            }
            if let Err(error) = &update.result {
                tracing::warn!(
                    host = %update.host,
                    error = %::diagnostics::chain(error),
                    "thread search failed on a Forge"
                );
            }
            self.pending = self.pending.saturating_sub(1);
            let search = self.search.get_or_insert_with(Vec::new);
            search.push(SearchResult {
                host: update.host,
                instance: update.instance,
                name: update.name,
                result: update.result,
            });
            changed = true;
        }
        changed
    }

    #[must_use]
    const fn searching(&self) -> bool {
        self.pending > 0
    }

    pub fn refresh(&mut self, active: Option<&HostId>, hosts: &[HostSnapshot<'_>]) {
        let selected = self
            .entries
            .get(self.highlighted)
            .map(|entry| entry.screen.clone());
        let query = self.query.trim().to_lowercase();
        let mut commands = Vec::new();
        if let Some(host) = active {
            commands.push(Entry {
                label: "New thread".into(),
                detail: "Ctrl+N".into(),
                screen: Screen::NewThread {
                    host: host.clone(),
                    project: None,
                },
            });
            commands.push(Entry {
                label: "Marketplace".into(),
                detail: "Skills and MCP servers".into(),
                screen: Screen::Marketplace,
            });
            commands.push(Entry {
                label: "Usage".into(),
                detail: "Engine limits and spend".into(),
                screen: Screen::Usage,
            });
        }
        for host in hosts {
            if Some(host.id) != active {
                commands.push(Entry {
                    label: format!("Switch to {}", host.name),
                    detail: "Host".into(),
                    screen: Screen::NewThread {
                        host: host.id.clone(),
                        project: None,
                    },
                });
            }
        }
        for (label, page) in [
            ("Settings", SettingsPage::General),
            (
                "Appearance",
                SettingsPage::Section(SettingsSection::Appearance),
            ),
            (
                "Text Streaming",
                SettingsPage::Section(SettingsSection::Streaming),
            ),
            (
                "Notifications",
                SettingsPage::Section(SettingsSection::Notifications),
            ),
            (
                "Performance",
                SettingsPage::Section(SettingsSection::Performance),
            ),
            (
                "Recovery and continuation",
                SettingsPage::Section(SettingsSection::Automation),
            ),
            (
                "claude.ai mode",
                SettingsPage::Section(SettingsSection::ClaudeAi),
            ),
            ("Forges", SettingsPage::Hosts),
            ("Account", SettingsPage::Account),
            ("About", SettingsPage::About),
            ("Threads and archive", SettingsPage::Threads),
            ("Global prompt", SettingsPage::GlobalPrompt),
            ("Skills and MCP servers", SettingsPage::Extensions),
        ] {
            commands.push(Entry {
                label: label.into(),
                detail: "Settings".into(),
                screen: Screen::Settings(page),
            });
        }
        commands.retain(|entry| {
            query.is_empty()
                || entry.label.to_lowercase().contains(&query)
                || entry.detail.to_lowercase().contains(&query)
        });
        let mut threads = Vec::new();
        if let Some(search) = &mut self.search {
            search.retain(|result| {
                hosts
                    .iter()
                    .any(|host| host.id == &result.host && host.instance == result.instance)
            });
            let mut found = Vec::new();
            let mut failures = 0;
            for result in search {
                match &result.result {
                    Ok(found_threads) => {
                        for thread in found_threads {
                            insert_thread(&mut found, thread, &result.host, &result.name, SHOWN);
                        }
                    }
                    Err(_) => failures += 1,
                }
            }
            self.search_error = (failures > 0).then_some(SearchProblem::Unreachable(failures));
            threads.extend(found.into_iter().map(|(_, entry)| entry));
        }
        let mut recent = Vec::new();
        for host in hosts {
            for thread in host
                .threads
                .values()
                .filter(|thread| !thread.archived && thread.parent_thread_id.is_none())
            {
                let label = thread.title.as_deref().unwrap_or("Untitled thread");
                let detail = host
                    .projects
                    .get(&thread.project_id)
                    .map_or("", |project| project.name.as_str());
                if !query.is_empty()
                    && !label.to_lowercase().contains(&query)
                    && !detail.to_lowercase().contains(&query)
                {
                    continue;
                }
                insert_thread(&mut recent, thread, host.id, detail, SHOWN);
            }
        }
        for (_, entry) in recent {
            if !threads
                .iter()
                .any(|known: &Entry| known.screen == entry.screen)
            {
                threads.push(entry);
            }
        }
        let entries = allocate(commands, threads);
        self.highlighted = selected
            .and_then(|screen| entries.iter().position(|entry| entry.screen == screen))
            .unwrap_or(0);
        self.entries = entries;
    }

    fn up(&mut self) {
        self.highlighted = self.highlighted.saturating_sub(1);
    }
    fn down(&mut self) {
        self.highlighted = (self.highlighted + 1).min(self.entries.len().saturating_sub(1));
    }
    pub fn selected(&self) -> Option<&Screen> {
        self.entries
            .get(self.highlighted)
            .map(|entry| &entry.screen)
    }
}
pub mod view;
