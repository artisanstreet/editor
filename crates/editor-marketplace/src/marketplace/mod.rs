pub mod controller;
pub mod presentation;
pub mod view;

use artisan_api::{CatalogSkill, Listing, McpServer, SkillAction};
use editor_core::{ForgeClient, HostId, Problem, Screen, SettingsPage, client::CallError};
use presentation::{Item, ItemKey, Kind};
use std::time::{Duration, Instant};

const REMOTE_SEARCH_DELAY: Duration = Duration::from_millis(250);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Remote {
    #[default]
    Idle,
    Searching,
    Found(Vec<Listing>),
    Failed(Problem),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    Hero,
    Notices,
    Found(String),
    Group(String),
    SearchFailed(Problem),
    Empty(String),
    Item {
        item: Item,
        first: bool,
        last: bool,
        selected: bool,
    },
}

#[derive(Clone, Debug)]
pub struct SearchRequest {
    query: String,
    serial: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Operation {
    Listing(String),
    Skill { id: String, action: SkillAction },
}

#[derive(Debug, thiserror::Error)]
pub enum OperationError {
    #[error("install listing {listing}")]
    Install {
        listing: String,
        #[source]
        source: CallError,
    },
    #[error("{verb} skill {skill}", verb = action.as_str())]
    Skill {
        skill: String,
        action: SkillAction,
        #[source]
        source: CallError,
    },
}

#[derive(Clone, Debug)]
pub struct OperationRequest {
    operation: Operation,
    generation: u64,
}

#[derive(Clone, Debug)]
pub enum Action {
    Operation(Operation),
    Navigate(Screen),
}

#[derive(Clone, Copy, Debug)]
pub enum ListKey {
    Up,
    Down,
    Accept,
    Dismiss,
}

#[derive(Debug, Default)]
pub struct Marketplace {
    pub host: Option<HostId>,
    generation: u64,
    pub query: String,
    kind: Kind,
    selected: Option<ItemKey>,
    pub found_open: bool,
    focus_search: bool,
    pub items: Vec<Item>,
    pub listings: Remote,
    pub pending: Option<Operation>,
    pub notice: Option<Problem>,
    shown: bool,
    search_serial: u64,
    search_due: Option<Instant>,
}

impl Marketplace {
    pub fn set_shown(&mut self, shown: bool) {
        self.focus_search |= shown && !self.shown;
        self.shown = shown;
    }

    pub fn sync_host(
        &mut self,
        host: Option<HostId>,
        catalog: &[CatalogSkill],
        servers: &[McpServer],
    ) {
        if !self.shown {
            return;
        }
        if self.host != host {
            self.host = host;
            self.generation += 1;
            self.items.clear();
            self.pending = None;
            self.notice = None;
        }
        if self.host.is_some() {
            self.items = presentation::items(catalog, servers);
        }
    }

    pub fn set_query(&mut self, query: String, now: Instant) {
        if query == self.query {
            return;
        }
        self.query = query;
        self.selected = None;
        self.restart_search(now);
    }

    fn restart_search(&mut self, now: Instant) -> bool {
        self.search_serial = self.search_serial.wrapping_add(1);
        self.search_due = None;
        if self.host.is_some() && self.query.trim().chars().count() >= 2 {
            self.listings = Remote::Searching;
            self.search_due = Some(now + REMOTE_SEARCH_DELAY);
            true
        } else {
            self.listings = Remote::Idle;
            false
        }
    }

    fn retry_search(&mut self, now: Instant) -> bool {
        matches!(self.listings, Remote::Failed(_)) && self.restart_search(now)
    }

    pub fn cancel_search(&mut self) {
        self.search_serial = self.search_serial.wrapping_add(1);
        self.search_due = None;
        self.listings = Remote::Idle;
    }

    pub fn search_deadline(&self) -> Option<Instant> {
        self.search_due
    }

    pub fn take_search(&mut self, now: Instant) -> Option<SearchRequest> {
        self.search_due.is_some_and(|due| now >= due).then(|| {
            self.search_due = None;
            SearchRequest {
                query: self.query.trim().to_owned(),
                serial: self.search_serial,
            }
        })
    }

    async fn search(
        client: &ForgeClient,
        request: &SearchRequest,
    ) -> Result<Vec<Listing>, CallError> {
        client.marketplace_search(&request.query).await
    }

    pub fn finish_search(
        &mut self,
        request: &SearchRequest,
        result: Result<Vec<Listing>, CallError>,
    ) {
        if request.serial != self.search_serial || self.query.trim() != request.query {
            return;
        }
        self.listings = match result {
            Ok(listings) => Remote::Found(listings),
            Err(error) => {
                Remote::Failed(Problem::calling_forge("Couldn't search skills.sh.", &error))
            }
        };
    }

    pub fn set_kind(&mut self, kind: Kind) {
        self.kind = kind;
        self.selected = None;
    }

    pub fn results(&self) -> Vec<Item> {
        let mut items = if self.query.trim().is_empty() {
            self.items.clone()
        } else {
            presentation::search(&self.items, &self.query)
        };
        if let Remote::Found(listings) = &self.listings {
            items.extend(
                presentation::by_name(listings)
                    .into_iter()
                    .filter(|(listing, _)| {
                        !self.items.iter().any(
                            |item| matches!(item, Item::Skill(skill) if skill.name == listing.name),
                        )
                    })
                    .map(|(listing, others)| presentation::listing_item(listing, others)),
            );
        }
        items
            .into_iter()
            .filter(|item| self.kind.admits(item))
            .collect()
    }

    pub fn selected_index(&self) -> Option<usize> {
        self.resolve_selection(&self.results())
    }

    fn resolve_selection(&self, results: &[Item]) -> Option<usize> {
        if results.is_empty() {
            return None;
        }
        Some(
            self.selected
                .as_ref()
                .and_then(|key| results.iter().position(|item| &item.key() == key))
                .unwrap_or(0),
        )
    }

    fn select(&mut self, results: &[Item], index: Option<usize>) {
        self.selected = index.and_then(|index| results.get(index)).map(Item::key);
    }

    pub fn list_key(&mut self, key: ListKey, now: Instant) -> Option<Action> {
        let results = self.results();
        let current = self.resolve_selection(&results);
        match key {
            ListKey::Up => self.select(&results, current.map(|index| index.saturating_sub(1))),
            ListKey::Down => self.select(
                &results,
                current.map(|index| (index + 1).min(results.len() - 1)),
            ),
            ListKey::Accept => {
                return current
                    .and_then(|index| results.get(index))
                    .and_then(Self::act);
            }
            ListKey::Dismiss => self.set_query(String::new(), now),
        }
        None
    }

    fn act(item: &Item) -> Option<Action> {
        match item {
            Item::Skill(row) if !item.managed() => Some(Action::Operation(Operation::Skill {
                id: row.id.clone(),
                action: SkillAction::Adopt,
            })),
            Item::Server(_) => Some(Action::Navigate(Screen::Settings(SettingsPage::Extensions))),
            Item::Listing(row) => Some(Action::Operation(Operation::Listing(row.id.clone()))),
            Item::Skill(_) => None,
        }
    }

    pub fn begin_operation(&mut self, operation: Operation) -> Option<OperationRequest> {
        if self.pending.is_some() || self.host.is_none() {
            return None;
        }
        self.pending = Some(operation.clone());
        self.notice = None;
        Some(OperationRequest {
            operation,
            generation: self.generation,
        })
    }

    async fn execute(
        client: &ForgeClient,
        request: &OperationRequest,
    ) -> Result<Vec<CatalogSkill>, OperationError> {
        match &request.operation {
            Operation::Listing(id) => {
                client
                    .marketplace_install(id)
                    .await
                    .map_err(|source| OperationError::Install {
                        listing: id.clone(),
                        source,
                    })
            }
            Operation::Skill { id, action } => {
                client
                    .skill_action(id, *action)
                    .await
                    .map_err(|source| OperationError::Skill {
                        skill: id.clone(),
                        action: *action,
                        source,
                    })
            }
        }
    }

    pub fn finish_operation(
        &mut self,
        request: &OperationRequest,
        result: Result<Vec<CatalogSkill>, OperationError>,
    ) -> bool {
        if self.generation != request.generation {
            return false;
        }
        self.pending = None;
        match result {
            Ok(_) => true,
            Err(error) => {
                let verb = match &request.operation {
                    Operation::Listing(_)
                    | Operation::Skill {
                        action: SkillAction::Adopt,
                        ..
                    } => "install",
                    Operation::Skill { action, .. } => action.as_str(),
                };
                self.notice = Some(Problem::caused_by(
                    format!("Couldn't {verb} the skill."),
                    &error,
                ));
                false
            }
        }
    }
}

pub fn entries(page: &Marketplace) -> Vec<Entry> {
    let mut entries = vec![Entry::Hero, Entry::Notices];
    let results = page.results();
    let group = |entries: &mut Vec<Entry>, items: Vec<&Item>, selected: Option<usize>| {
        let count = items.len();
        entries.extend(
            items
                .into_iter()
                .enumerate()
                .map(|(index, item)| Entry::Item {
                    item: item.clone(),
                    first: index == 0,
                    last: index + 1 == count,
                    selected: selected == Some(index),
                }),
        );
    };
    if !page.query.trim().is_empty() {
        let selected = page.resolve_selection(&results);
        let (local, remote): (Vec<&Item>, Vec<&Item>) = results
            .iter()
            .partition(|item| !matches!(item, Item::Listing(_)));
        let local_count = local.len();
        if !local.is_empty() {
            entries.push(Entry::Group("On this Forge".to_owned()));
            group(&mut entries, local, selected);
        }
        match &page.listings {
            Remote::Searching => {
                entries.push(Entry::Group("Searching skills.sh\u{2026}".to_owned()));
            }
            Remote::Failed(error) => entries.push(Entry::SearchFailed(error.clone())),
            Remote::Found(_) if !remote.is_empty() => {
                entries.push(Entry::Group("skills.sh".to_owned()));
                group(
                    &mut entries,
                    remote,
                    selected.and_then(|selected| selected.checked_sub(local_count)),
                );
            }
            Remote::Found(_) | Remote::Idle => {}
        }
        if results.is_empty() && !matches!(page.listings, Remote::Searching) {
            entries.push(Entry::Empty(format!(
                "Nothing called \u{201c}{}\u{201d}.",
                page.query.trim()
            )));
        }
        return entries;
    }
    let (installed, found): (Vec<&Item>, Vec<&Item>) =
        results.iter().partition(|item| item.managed());
    if let Some(summary) =
        presentation::found_summary(&page.items).filter(|_| page.kind != Kind::Servers)
    {
        entries.push(Entry::Found(summary));
        if page.found_open {
            group(&mut entries, found, None);
        }
    }
    entries.push(Entry::Group(format!(
        "Installed \u{b7} {}",
        installed.len()
    )));
    if installed.is_empty() {
        entries.push(Entry::Empty(
            match page.kind {
                Kind::Servers => "No MCP servers yet.",
                Kind::All | Kind::Skills => "Nothing installed yet.",
            }
            .to_owned(),
        ));
    } else {
        group(&mut entries, installed, None);
    }
    entries
}
