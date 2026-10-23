//! Skills and MCP servers from the active Forge, behind one search.
//! This screen renders the shared HostData projections and sends user choices
//! to Forge, which owns installation and adoption.

#![forbid(unsafe_code)]

mod presentation;
mod render;
#[cfg(test)]
mod tests;

use artisan_api::{McpServer, SkillAction};
use editor_core::{Core, CoreEvent, HostEvent, HostId, Screen, SettingsPage};
use editor_ui::text_editor::{ListKey, TextEditor, TextEditorEvent};
use editor_ui::theme::{ActiveTheme as _, ArtisanTheme};
use gpui::{AppContext as _, Context, Entity, ListAlignment, ListState, Subscription, Task, px};
use presentation::{Item, Kind};
use std::time::Duration;

pub const MARKETPLACE_SCREEN_SELECTOR: &str = "route-marketplace";
pub const MARKETPLACE_SEARCH_SELECTOR: &str = "marketplace-search";

/// Typing pauses this long before skills.sh is asked.
const REMOTE_SEARCH_DELAY: Duration = Duration::from_millis(250);

/// Where the skills.sh search for the current query stands.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum Remote {
    /// No query, or one too short to send.
    #[default]
    Idle,
    Searching,
    Found(Vec<artisan_api::Listing>),
    Failed(String),
}

pub struct Marketplace {
    core: Entity<Core>,
    theme: ArtisanTheme,
    host: Option<HostId>,
    generation: u64,
    search: Entity<TextEditor>,
    query: String,
    kind: Kind,
    /// The highlighted result while searching; Enter acts on it.
    selected: usize,
    /// Whether the skills the engines have on their own are listed.
    found_open: bool,
    /// Focus the search on the next frame (the page just opened).
    focus_search: bool,
    items: Vec<Item>,
    /// skills.sh's answer for `query`, once it arrives.
    listings: Remote,
    /// The pending skills.sh search; replacing it cancels the old one.
    _listing_search: Option<Task<()>>,
    entries: Vec<render::Entry>,
    list: ListState,
    pending: Option<String>,
    notice: Option<String>,
    shown: bool,
    /// Follows the shown host's changes while the page is shown.
    host_events: Option<Subscription>,
    _core: Subscription,
    _search: [Subscription; 2],
}

impl Marketplace {
    pub fn new(core: Entity<Core>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&core, |this, core, event: &CoreEvent, cx| {
            let was_shown = this.shown;
            this.shown = matches!(core.read(cx).screen(), Screen::Marketplace);
            if this.shown && matches!(event, CoreEvent::ScreenChanged | CoreEvent::HostsChanged) {
                this.focus_search |= !was_shown;
                this.sync(cx);
                cx.notify();
            } else if !this.shown {
                this.host_events = None;
            }
        });
        let search = cx.new(|cx| {
            TextEditor::new("", "Search skills and MCP servers", cx)
                .single_line()
                .bare()
        });
        let edited = cx.subscribe(&search, |this, search, event: &TextEditorEvent, cx| {
            if *event == TextEditorEvent::Changed {
                let query = search.read(cx).text().to_owned();
                this.set_query(query, cx);
            }
        });
        let keys = cx.subscribe(&search, |this, _, key: &ListKey, cx| {
            this.list_key(*key, cx);
        });
        let shown = matches!(core.read(cx).screen(), Screen::Marketplace);
        let mut page = Self {
            core,
            theme: *cx.theme(),
            host: None,
            generation: 0,
            search,
            query: String::new(),
            kind: Kind::All,
            selected: 0,
            found_open: false,
            focus_search: shown,
            items: Vec::new(),
            listings: Remote::Idle,
            _listing_search: None,
            entries: Vec::new(),
            list: ListState::new(0, ListAlignment::Top, px(256.0)),
            pending: None,
            notice: None,
            shown,
            host_events: None,
            _core: subscription,
            _search: [edited, keys],
        };
        page.rebuild(true);
        if shown {
            page.sync(cx);
        }
        page
    }

    fn sync(&mut self, cx: &mut Context<Self>) {
        if !self.shown {
            return;
        }
        let host = self.core.read(cx).active_host().cloned();
        let id = host.as_ref().map(|host| host.read(cx).id.clone());
        if self.host != id {
            self.host = id;
            self.generation += 1;
            self.items.clear();
            self.pending = None;
            self.notice = None;
            self.host_events = None;
            self.rebuild(true);
        }
        let Some(host) = host else {
            return;
        };
        if self.host_events.is_none() {
            self.host_events = Some(cx.subscribe(&host, |this, _, event: &HostEvent, cx| {
                if matches!(event, HostEvent::Changed | HostEvent::Reset) {
                    this.sync(cx);
                    cx.notify();
                }
            }));
        }
        host.update(cx, |host, cx| {
            host.load_skill_catalog(false, cx);
            host.load_prompt(false, cx);
        });
        let host = host.read(cx);
        let servers: Vec<McpServer> = host
            .setting("mcp_servers")
            .and_then(|value| serde_json::from_value(value.clone()).ok())
            .unwrap_or_default();
        let items = presentation::items(
            host.skill_catalog
                .as_deref()
                .map(Vec::as_slice)
                .unwrap_or_default(),
            &servers,
        );
        let changed = items != self.items;
        self.items = items;
        self.rebuild(changed);
    }

    /// Lays the rows out again; `reset` when what they show changed, so the
    /// list measures them afresh.
    fn rebuild(&mut self, reset: bool) {
        let old = self.entries.len();
        self.entries = render::entries(self);
        if reset {
            self.list.reset(self.entries.len());
        } else {
            // Notices and the prompt's state resize rows without new items.
            self.list.splice(0..old, self.entries.len());
        }
    }

    /// What the page lists: everything on the host at rest; while
    /// searching, the host's matches, then skills.sh's skills the host
    /// doesn't have by that name.
    fn results(&self) -> Vec<Item> {
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

    /// Asks skills.sh for the query once typing pauses; an answer for an
    /// older query is dropped.
    #[expect(
        clippy::used_underscore_binding,
        reason = "the search task is only held so replacing it cancels the old one"
    )]
    fn search_remote(&mut self, cx: &Context<Self>) {
        let query = self.query.trim().to_owned();
        self._listing_search = None;
        let Some(host) = self.core.read(cx).active_host().cloned() else {
            self.listings = Remote::Idle;
            return;
        };
        if query.chars().count() < 2 {
            self.listings = Remote::Idle;
            return;
        }
        self.listings = Remote::Searching;
        let client = host.read(cx).client.clone();
        self._listing_search = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(REMOTE_SEARCH_DELAY).await;
            let answer = client.marketplace_search(&query).await;
            this.update(cx, |this, cx| {
                if this.query.trim() != query {
                    return;
                }
                this.listings = match answer {
                    Ok(listings) => Remote::Found(listings),
                    Err(error) => Remote::Failed(format!("Couldn't search skills.sh: {error:#}")),
                };
                this.rebuild(true);
                cx.notify();
            })
            .ok();
        }));
    }

    /// Fetches a listed skill into Artisan and every engine.
    fn install_listing(&mut self, id: String, cx: &mut Context<Self>) {
        if self.pending.is_some() {
            return;
        }
        let Some(host) = self.core.read(cx).active_host().cloned() else {
            return;
        };
        let generation = self.generation;
        let client = host.read(cx).client.clone();
        self.pending = Some(id.clone());
        self.notice = None;
        self.rebuild(false);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = client.marketplace_install(&id).await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.pending = None;
                match result {
                    Ok(_) => host.update(cx, |host, _| host.invalidate_catalog()),
                    Err(error) => {
                        this.notice = Some(format!("Couldn't install the skill: {error:#}"));
                    }
                }
                this.sync(cx);
                this.rebuild(false);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn set_query(&mut self, query: String, cx: &mut Context<Self>) {
        if query == self.query {
            return;
        }
        self.query = query;
        self.selected = 0;
        let searching = !self.query.trim().is_empty();
        self.search
            .update(cx, |search, _| search.set_list_open(searching));
        self.search_remote(cx);
        self.rebuild(true);
        cx.notify();
    }

    fn set_kind(&mut self, kind: Kind, cx: &mut Context<Self>) {
        self.kind = kind;
        self.selected = 0;
        self.rebuild(true);
        cx.notify();
    }

    fn clear_search(&mut self, cx: &mut Context<Self>) {
        self.search.update(cx, |search, cx| search.set_text("", cx));
        self.set_query(String::new(), cx);
    }

    fn list_key(&mut self, key: ListKey, cx: &mut Context<Self>) {
        let results = self.results();
        match key {
            ListKey::Up => self.selected = self.selected.saturating_sub(1),
            ListKey::Down => {
                self.selected = (self.selected + 1).min(results.len().saturating_sub(1));
            }
            ListKey::Accept => {
                if let Some(item) = results.get(self.selected) {
                    self.act(item, cx);
                }
                return;
            }
            ListKey::Dismiss => {
                self.clear_search(cx);
                return;
            }
        }
        self.rebuild(false);
        if let Some(index) = self
            .entries
            .iter()
            .position(|entry| matches!(entry, render::Entry::Item { selected: true, .. }))
        {
            self.list.scroll_to_reveal_item(index);
        }
        cx.notify();
    }

    /// Enter on a result: install a quarantined skill, or
    /// open an MCP server's settings. A managed skill keeps its switch.
    fn act(&mut self, item: &Item, cx: &mut Context<Self>) {
        match item {
            Item::Skill(row) if !item.managed() => {
                self.skill_action(row.id.clone(), SkillAction::Adopt, cx);
            }
            Item::Server(_) => self.navigate(Screen::Settings(SettingsPage::Extensions), cx),
            Item::Listing(row) => self.install_listing(row.id.clone(), cx),
            Item::Skill(_) => {}
        }
    }

    fn skill_action(&mut self, id: String, action: SkillAction, cx: &mut Context<Self>) {
        if self.pending.is_some() {
            return;
        }
        let Some(host) = self.core.read(cx).active_host().cloned() else {
            return;
        };
        let generation = self.generation;
        let client = host.read(cx).client.clone();
        self.pending = Some(id.clone());
        self.notice = None;
        self.rebuild(false);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = client.skill_action(&id, action).await;
            this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.pending = None;
                match result {
                    Ok(_) => host.update(cx, |host, _| {
                        host.invalidate_catalog();
                    }),
                    Err(error) => {
                        let verb = match action {
                            SkillAction::Adopt => "install",
                            other @ (SkillAction::Enable
                            | SkillAction::Disable
                            | SkillAction::Discard) => other.as_str(),
                        };
                        this.notice = Some(format!("Couldn't {verb} the skill: {error:#}"));
                    }
                }
                this.sync(cx);
                this.rebuild(false);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn navigate(&self, screen: Screen, cx: &mut Context<Self>) {
        self.core.update(cx, |core, cx| core.navigate(screen, cx));
    }
}
