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

type Recency = (Timestamp, HostId, ThreadId);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub label: String,
    pub detail: String,
    pub screen: Screen,
}

pub struct HostSnapshot<'a> {
    pub id: &'a HostId,
    pub name: &'a str,
    pub instance: HostInstance,
    pub threads: &'a HashMap<ThreadId, Thread>,
    pub projects: &'a BTreeMap<ProjectId, Project>,
}

pub struct SearchHost {
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
    result: Result<Vec<Thread>, ()>,
}

struct SearchResult {
    host: HostId,
    instance: HostInstance,
    name: String,
    result: Result<Vec<Thread>, ()>,
}

pub struct Palette {
    pub query: String,
    pub highlighted: usize,
    pub entries: Vec<Entry>,
    pub search_error: Option<String>,
    search: Option<Vec<SearchResult>>,
    generation: u64,
    request: Option<AbortOnDropHandle<()>>,
    delivery: crate::core::delivery::Delivery<SearchUpdate>,
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

impl Default for Palette {
    fn default() -> Self {
        Self {
            query: String::new(),
            highlighted: 0,
            entries: Vec::new(),
            search_error: None,
            search: None,
            generation: 0,
            request: None,
            delivery: crate::core::delivery::Delivery::new(NonZeroUsize::new(64).unwrap()),
        }
    }
}

impl Palette {
    pub fn wake_mailbox(&self) -> Option<&aqua::mailbox::Mailbox<()>> {
        self.delivery.wake_mailbox()
    }

    pub fn start_search(&mut self, query: String, hosts: Vec<SearchHost>) {
        self.query = query;
        self.generation = self.generation.wrapping_add(1);
        self.request = None;
        self.search = None;
        self.search_error = None;
        let query = self.query.trim().to_owned();
        if query.is_empty() {
            return;
        }
        if hosts.is_empty() {
            self.search_error = Some("Connect to a Forge to search older threads.".into());
            return;
        }
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
                        result: result.map(|page| page.threads).map_err(|_| ()),
                    };
                    if sender.send(update).await.is_err() {
                        return;
                    }
                }
            }),
        ));
    }

    pub fn drain(&mut self, core: &crate::core::Core) -> bool {
        let mut changed = false;
        for update in self.delivery.drain() {
            if update.generation != self.generation
                || core
                    .host_if_instance(&update.host, update.instance)
                    .is_none()
            {
                continue;
            }
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

    pub fn refresh(&mut self, active: Option<&HostId>, hosts: &[HostSnapshot<'_>]) {
        let selected = self
            .entries
            .get(self.highlighted)
            .map(|entry| entry.screen.clone());
        let query = self.query.trim().to_lowercase();
        let mut entries = Vec::new();
        if let Some(host) = active {
            entries.push(Entry {
                label: "New thread".into(),
                detail: "Ctrl+N".into(),
                screen: Screen::NewThread {
                    host: host.clone(),
                    project: None,
                },
            });
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
            entries.push(Entry {
                label: label.into(),
                detail: "Settings".into(),
                screen: Screen::Settings(page),
            });
        }
        for host in hosts {
            if Some(host.id) != active {
                entries.push(Entry {
                    label: format!("Switch to {}", host.name),
                    detail: "Host".into(),
                    screen: Screen::NewThread {
                        host: host.id.clone(),
                        project: None,
                    },
                });
            }
        }
        entries.retain(|entry| {
            query.is_empty()
                || entry.label.to_lowercase().contains(&query)
                || entry.detail.to_lowercase().contains(&query)
        });
        if let Some(search) = &mut self.search {
            search.retain(|result| {
                hosts
                    .iter()
                    .any(|host| host.id == &result.host && host.instance == result.instance)
            });
            let mut results = Vec::new();
            let mut failures = 0;
            for result in search {
                match &result.result {
                    Ok(threads) => {
                        for thread in threads {
                            insert_thread(&mut results, thread, &result.host, &result.name, SHOWN);
                        }
                    }
                    Err(()) => failures += 1,
                }
            }
            self.search_error =
                (failures > 0).then(|| format!("Search could not reach {failures} Forge(s)."));
            let additions: Vec<_> = results
                .into_iter()
                .map(|(_, entry)| entry)
                .filter(|candidate| !entries.iter().any(|entry| entry.screen == candidate.screen))
                .collect();
            entries.extend(additions);
            entries.truncate(SHOWN);
        }
        let room = SHOWN.saturating_sub(entries.len());
        let mut recent = Vec::with_capacity(room + 1);
        if room > 0 {
            for host in hosts {
                for thread in host
                    .threads
                    .values()
                    .filter(|thread| !thread.archived && thread.parent_thread_id.is_none())
                {
                    if entries.iter().any(|entry| matches!(&entry.screen, Screen::Thread { host: id, thread: thread_id } if id == host.id && thread_id == &thread.id)) { continue; }
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
                    insert_thread(&mut recent, thread, host.id, detail, room);
                }
            }
        }
        entries.extend(recent.into_iter().map(|(_, entry)| entry));
        entries.truncate(SHOWN);
        self.highlighted = selected
            .and_then(|screen| entries.iter().position(|entry| entry.screen == screen))
            .unwrap_or(0);
        self.entries = entries;
    }

    pub fn up(&mut self) {
        self.highlighted = self.highlighted.saturating_sub(1);
    }
    pub fn down(&mut self) {
        self.highlighted = (self.highlighted + 1).min(self.entries.len().saturating_sub(1));
    }
    pub fn selected(&self) -> Option<&Screen> {
        self.entries
            .get(self.highlighted)
            .map(|entry| &entry.screen)
    }
}
pub mod view;
