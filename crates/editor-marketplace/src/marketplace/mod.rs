pub mod controller;
pub mod presentation;
pub mod view;

use artisan_api::{CatalogSkill, Listing, McpServer, SkillAction};
use editor_core::{
    CatalogRead, ForgeClient, HostId, Problem, Screen, SettingsPage, client::CallError,
};
use presentation::{Item, ItemKey, Kind};
use std::collections::HashMap;
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

impl Operation {
    #[must_use]
    fn targets(&self, item: &Item) -> bool {
        item.key() == self.target()
    }

    fn target(&self) -> ItemKey {
        match self {
            Self::Listing(id) => ItemKey::Listing(id.clone()),
            Self::Skill { id, .. } => ItemKey::Skill(id.clone()),
        }
    }

    const fn toggles(&self) -> bool {
        matches!(
            self,
            Self::Skill {
                action: SkillAction::Enable | SkillAction::Disable,
                ..
            }
        )
    }

    fn id(&self) -> &str {
        match self {
            Self::Listing(id) | Self::Skill { id, .. } => id,
        }
    }

    fn verb(&self) -> &'static str {
        match self {
            Self::Listing(_)
            | Self::Skill {
                action: SkillAction::Adopt,
                ..
            } => "install",
            Self::Skill { action, .. } => action.as_str(),
        }
    }

    fn overlay(&self, item: &mut Item) {
        if !self.targets(item) {
            return;
        }
        if let (
            Self::Skill {
                action: action @ (SkillAction::Enable | SkillAction::Disable),
                ..
            },
            Item::Skill(row),
        ) = (self, item)
        {
            row.enabled = *action == SkillAction::Enable;
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Overlap {
    #[default]
    Alone,
    Overlapped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reread {
    Due,
    Retried,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SendId(u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Queued,
    Sending(SendId),
    Settling { after: CatalogRead, reread: Reread },
}

impl Stage {
    const fn active(self) -> bool {
        matches!(self, Self::Queued | Self::Sending(_))
    }

    const fn sending(self) -> bool {
        matches!(self, Self::Sending(_))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Pending {
    operation: Operation,
    name: String,
    stage: Stage,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Notice {
    target: ItemKey,
    problem: Problem,
}

#[derive(Debug, Default)]
struct Writes {
    generation: u64,
    pending: Vec<Pending>,
    overlap: Overlap,
    notices: Vec<Notice>,
}

impl Writes {
    fn sending(&self) -> impl Iterator<Item = &Pending> {
        self.pending
            .iter()
            .filter(|pending| pending.stage.sending())
    }

    fn notify(&mut self, target: ItemKey, problem: Problem) {
        self.notices.retain(|notice| notice.target != target);
        self.notices.push(Notice { target, problem });
    }
}

#[derive(Debug)]
pub enum Reply {
    Adopted,
    Unconfirmed { after: CatalogRead },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogAfter {
    Kept,
    Reload,
}

#[derive(Debug)]
pub enum Settled {
    Stale,
    Done {
        next: Option<OperationRequest>,
        catalog: CatalogAfter,
    },
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
    host: HostId,
    operation: Operation,
    generation: u64,
    send: SendId,
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
    host: Option<HostId>,
    generation: u64,
    pub query: String,
    kind: Kind,
    selected: Option<ItemKey>,
    pub found_open: bool,
    focus_search: bool,
    pub items: Vec<Item>,
    pub listings: Remote,
    writes: HashMap<HostId, Writes>,
    sends: u64,
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
        fresh: Option<CatalogRead>,
        servers: &[McpServer],
    ) {
        if !self.shown {
            return;
        }
        if self.host != host {
            self.host = host;
            self.items.clear();
        }
        let Some(writes) = self
            .host
            .as_ref()
            .and_then(|host| self.writes.get_mut(host))
        else {
            if self.host.is_some() {
                self.items = presentation::items(catalog, servers);
            }
            return;
        };
        writes.pending.retain(|pending| match pending.stage {
            Stage::Settling { after, .. } => fresh.is_none_or(|read| read <= after),
            Stage::Queued | Stage::Sending(_) => true,
        });
        self.items = presentation::items(catalog, servers);
        for pending in &writes.pending {
            for item in &mut self.items {
                pending.operation.overlay(item);
            }
        }
    }

    fn reread_failed_catalog(&mut self) -> bool {
        let Some(writes) = self
            .host
            .as_ref()
            .and_then(|host| self.writes.get_mut(host))
        else {
            return false;
        };
        let mut due = false;
        for pending in &mut writes.pending {
            if let Stage::Settling { reread, .. } = &mut pending.stage
                && *reread == Reread::Due
            {
                *reread = Reread::Retried;
                due = true;
            }
        }
        due
    }

    pub fn replace_host(&mut self, host: &HostId) {
        let Some(old) = self.writes.remove(host) else {
            return;
        };
        let writes = writes_entry(&mut self.writes, &mut self.generation, host);
        writes.notices = old.notices;
        for pending in old.pending {
            let problem = match pending.stage {
                Stage::Queued => Problem::new(format!(
                    "Couldn't {} {}: the Forge connection was replaced before the change was sent.",
                    pending.operation.verb(),
                    pending.name
                )),
                Stage::Sending(_) => Problem::new(format!(
                    "The change to {} may not have been saved: the Forge connection was replaced while it was sent.",
                    pending.name
                )),
                Stage::Settling { .. } => continue,
            };
            writes.notify(pending.operation.target(), problem);
        }
    }

    fn forget_host(&mut self, host: &HostId) {
        self.writes.remove(host);
    }

    fn holds_writes(&self, host: &HostId) -> bool {
        self.writes.contains_key(host)
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

    fn cancel_search(&mut self) {
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

    fn shown_writes(&self) -> Option<&Writes> {
        self.writes.get(self.host.as_ref()?)
    }

    pub fn pending(&self) -> impl Iterator<Item = &Operation> {
        self.shown_writes()
            .into_iter()
            .flat_map(|writes| &writes.pending)
            .filter(|pending| pending.stage.active())
            .map(|pending| &pending.operation)
    }

    pub fn notices(&self) -> impl Iterator<Item = &Problem> {
        self.shown_writes()
            .into_iter()
            .flat_map(|writes| &writes.notices)
            .map(|notice| &notice.problem)
    }

    pub fn begin_operation(&mut self, operation: Operation) -> Option<OperationRequest> {
        let host = self.host.clone()?;
        let target = operation.target();
        let name = self
            .results()
            .iter()
            .find(|item| operation.targets(item))
            .map_or_else(|| operation.id().to_owned(), |item| item.name().to_owned());
        let send = self.next_send();
        let writes = writes_entry(&mut self.writes, &mut self.generation, &host);
        if writes
            .pending
            .iter()
            .any(|pending| pending.stage.active() && pending.operation.target() == target)
        {
            return None;
        }
        writes.notices.retain(|notice| notice.target != target);
        for item in &mut self.items {
            operation.overlay(item);
        }
        let waits =
            operation.toggles() && writes.sending().any(|pending| pending.operation.toggles());
        if waits {
            writes.pending.push(Pending {
                operation,
                name,
                stage: Stage::Queued,
            });
            return None;
        }
        Some(Self::start(&host, writes, operation, name, send))
    }

    fn next_send(&mut self) -> SendId {
        self.sends += 1;
        SendId(self.sends)
    }

    fn start(
        host: &HostId,
        writes: &mut Writes,
        operation: Operation,
        name: String,
        send: SendId,
    ) -> OperationRequest {
        if writes.sending().next().is_some() {
            writes.overlap = Overlap::Overlapped;
        }
        let target = operation.target();
        match writes
            .pending
            .iter_mut()
            .find(|pending| pending.stage == Stage::Queued && pending.operation.target() == target)
        {
            Some(pending) => pending.stage = Stage::Sending(send),
            None => writes.pending.push(Pending {
                operation: operation.clone(),
                name,
                stage: Stage::Sending(send),
            }),
        }
        OperationRequest {
            host: host.clone(),
            operation,
            generation: writes.generation,
            send,
        }
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
        result: Result<Reply, OperationError>,
        reads: CatalogRead,
    ) -> Settled {
        let send = self.next_send();
        let Some(writes) = self
            .writes
            .get_mut(&request.host)
            .filter(|writes| writes.generation == request.generation)
        else {
            return Settled::Stale;
        };
        let Some(at) = writes
            .pending
            .iter()
            .position(|pending| pending.stage == Stage::Sending(request.send))
        else {
            return Settled::Stale;
        };
        let target = request.operation.target();
        let name = writes.pending.remove(at).name;
        let mut reread = false;
        match result {
            Ok(reply) => {
                writes.pending.retain(|pending| {
                    !(matches!(pending.stage, Stage::Settling { .. })
                        && pending.operation.target() == target)
                });
                let others_sending = writes.sending().next().is_some();
                let settle = match reply {
                    Reply::Unconfirmed { after } => {
                        reread = true;
                        Some(after)
                    }
                    Reply::Adopted => others_sending.then_some(reads),
                };
                if let Some(after) = settle.filter(|_| request.operation.toggles()) {
                    writes.pending.push(Pending {
                        operation: request.operation.clone(),
                        name,
                        stage: Stage::Settling {
                            after,
                            reread: Reread::Due,
                        },
                    });
                }
            }
            Err(error) => writes.notify(
                target,
                Problem::caused_by(
                    format!("Couldn't {} {name}.", request.operation.verb()),
                    &error,
                ),
            ),
        }
        let queued = request
            .operation
            .toggles()
            .then(|| {
                writes
                    .pending
                    .iter()
                    .find(|pending| pending.stage == Stage::Queued)
                    .map(|pending| (pending.operation.clone(), pending.name.clone()))
            })
            .flatten();
        let next = queued
            .map(|(operation, name)| Self::start(&request.host, writes, operation, name, send));
        let settled_overlap = writes.sending().next().is_none()
            && std::mem::take(&mut writes.overlap) == Overlap::Overlapped;
        let catalog = if reread || settled_overlap {
            CatalogAfter::Reload
        } else {
            CatalogAfter::Kept
        };
        Settled::Done { next, catalog }
    }
}

fn writes_entry<'a>(
    writes: &'a mut HashMap<HostId, Writes>,
    generation: &mut u64,
    host: &HostId,
) -> &'a mut Writes {
    writes.entry(host.clone()).or_insert_with(|| {
        *generation += 1;
        Writes {
            generation: *generation,
            ..Writes::default()
        }
    })
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
