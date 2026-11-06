pub mod controller;
pub mod presentation;
pub mod view;

use crate::core::{ForgeClient, HostId, Screen, SettingsPage};
use artisan_api::{CatalogSkill, Listing, McpServer, SkillAction};
use presentation::{Item, Kind};
use std::time::{Duration, Instant};

pub const MARKETPLACE_SCREEN_SELECTOR: &str = "route-marketplace";
pub const MARKETPLACE_SEARCH_SELECTOR: &str = "marketplace-search";
pub const REMOTE_SEARCH_DELAY: Duration = Duration::from_millis(250);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Remote {
    #[default]
    Idle,
    Searching,
    Found(Vec<Listing>),
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    Hero,
    Notices,
    Found(String),
    Group(String),
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
    pub query: String,
    serial: u64,
}

#[derive(Clone, Debug)]
pub enum Operation {
    Listing(String),
    Skill { id: String, action: SkillAction },
}

#[derive(Clone, Debug)]
pub struct OperationRequest {
    pub operation: Operation,
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

#[derive(Default)]
pub struct Marketplace {
    pub host: Option<HostId>,
    pub generation: u64,
    pub query: String,
    pub kind: Kind,
    pub selected: usize,
    pub found_open: bool,
    pub focus_search: bool,
    pub items: Vec<Item>,
    pub listings: Remote,
    pub pending: Option<String>,
    pub notice: Option<String>,
    pub shown: bool,
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
        self.selected = 0;
        self.search_serial = self.search_serial.wrapping_add(1);
        self.search_due = None;
        if self.host.is_some() && self.query.trim().chars().count() >= 2 {
            self.listings = Remote::Searching;
            self.search_due = Some(now + REMOTE_SEARCH_DELAY);
        } else {
            self.listings = Remote::Idle;
        }
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
        if self.search_due.is_some_and(|due| now >= due) {
            self.search_due = None;
            Some(SearchRequest {
                query: self.query.trim().to_owned(),
                serial: self.search_serial,
            })
        } else {
            None
        }
    }

    pub async fn search(
        client: &ForgeClient,
        request: &SearchRequest,
    ) -> anyhow::Result<Vec<Listing>> {
        client.marketplace_search(&request.query).await
    }

    pub fn finish_search(&mut self, request: &SearchRequest, result: anyhow::Result<Vec<Listing>>) {
        if request.serial != self.search_serial || self.query.trim() != request.query {
            return;
        }
        self.listings = match result {
            Ok(listings) => Remote::Found(listings),
            Err(error) => Remote::Failed(format!("Couldn't search skills.sh: {error:#}")),
        };
    }

    pub fn set_kind(&mut self, kind: Kind) {
        self.kind = kind;
        self.selected = 0;
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
                        !self.items.iter().any(|item| item.name() == listing.name)
                    })
                    .map(|(listing, others)| presentation::listing_item(listing, others)),
            );
        }
        items
            .into_iter()
            .filter(|item| self.kind.admits(item))
            .collect()
    }

    pub fn list_key(&mut self, key: ListKey, now: Instant) -> Option<Action> {
        let results = self.results();
        match key {
            ListKey::Up => self.selected = self.selected.saturating_sub(1),
            ListKey::Down => {
                self.selected = (self.selected + 1).min(results.len().saturating_sub(1))
            }
            ListKey::Accept => return results.get(self.selected).and_then(Self::act),
            ListKey::Dismiss => self.set_query(String::new(), now),
        }
        None
    }

    pub fn act(item: &Item) -> Option<Action> {
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
        let id = match &operation {
            Operation::Listing(id) | Operation::Skill { id, .. } => id,
        };
        self.pending = Some(id.clone());
        self.notice = None;
        Some(OperationRequest {
            operation,
            generation: self.generation,
        })
    }

    pub async fn execute(
        client: &ForgeClient,
        request: &OperationRequest,
    ) -> anyhow::Result<Vec<CatalogSkill>> {
        match &request.operation {
            Operation::Listing(id) => client.marketplace_install(id).await,
            Operation::Skill { id, action } => client.skill_action(id, *action).await,
        }
    }

    pub fn finish_operation(
        &mut self,
        request: &OperationRequest,
        result: anyhow::Result<Vec<CatalogSkill>>,
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
                self.notice = Some(format!("Couldn't {verb} the skill: {error:#}"));
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
        let selected = page.selected.min(results.len().saturating_sub(1));
        let (local, remote): (Vec<&Item>, Vec<&Item>) = results
            .iter()
            .partition(|item| !matches!(item, Item::Listing(_)));
        let local_count = local.len();
        if !local.is_empty() {
            entries.push(Entry::Group("On this host".to_owned()));
            group(&mut entries, local, Some(selected));
        }
        match &page.listings {
            Remote::Searching => {
                entries.push(Entry::Group("Searching skills.sh\u{2026}".to_owned()));
            }
            Remote::Failed(error) => entries.push(Entry::Group(error.clone())),
            Remote::Found(_) if !remote.is_empty() => {
                entries.push(Entry::Group("skills.sh".to_owned()));
                group(&mut entries, remote, selected.checked_sub(local_count));
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
