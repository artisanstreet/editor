//! The open thread's one copy of its items: the loaded window keyed by
//! position, the live text, the viewport and per-item UI state.

use crate::viewport::{Effect, Fetch, Input, Jump, State, Viewport};
use artisan_api::{
    Item, ItemId, ItemKind, ItemsPage, ItemsQuery, LiveTarget, RunId, Seq, ThreadId,
};
use statig::blocking::{IntoStateMachineExt as _, StateMachine};
use std::collections::{BTreeMap, HashMap};

/// Items per page.
pub(crate) const PAGE: u32 = 100;
/// Items per page while reading back to the start of a turn: Forge's
/// largest. A turn with subagents runs to thousands of items.
pub(crate) const SEARCH_PAGE: u32 = 500;
/// The window grows to this many items before far ones are dropped…
/// An agentic turn runs to hundreds of items, so this is generous: dropping
/// the newest while the reader is up in history shows a gap below.
const WINDOW_MAX: usize = 4000;
/// …down to this many.
const WINDOW_KEEP: usize = 3000;

/// UI state of one item: user toggles, an answer draft. (Reveal progress
/// lives in the reply's row view.)
#[derive(Clone, Debug, Default, Hash)]
pub(crate) struct ItemUi {
    /// The user opened or closed the item's details.
    pub expanded: Option<bool>,
    /// Arrived while the thread was open: prose reveals from its first
    /// word rather than appearing whole, and rows enter.
    pub fresh: bool,
    /// The user opened or closed the activity chain this item starts.
    pub chain_open: Option<bool>,
    /// An answer is on its way to Forge.
    pub sending: bool,
    /// Why the last answer failed.
    pub error: Option<String>,
}

pub(crate) struct ThreadView {
    pub thread: ThreadId,
    /// The loaded window, by position.
    pub items: BTreeMap<Seq, Item>,
    positions: HashMap<ItemId, Seq>,
    /// Memory-only text of the live run.
    pub live: HashMap<LiveTarget, String>,
    pub viewport: StateMachine<Viewport>,
    pub ui: HashMap<ItemId, ItemUi>,
    /// The user's fold choice per run; `None` follows the derived default.
    pub work_open: HashMap<RunId, bool>,
}

impl ThreadView {
    /// A view of `thread` and the effects that load it.
    pub(crate) fn open(thread: ThreadId) -> (Self, Vec<Effect>) {
        let mut view = Self {
            thread,
            items: BTreeMap::new(),
            positions: HashMap::new(),
            live: HashMap::new(),
            viewport: Viewport::default().state_machine(),
            ui: HashMap::new(),
            work_open: HashMap::new(),
        };
        let effects = view.handle(&Input::Open);
        (view, effects)
    }

    pub(crate) fn handle(&mut self, input: &Input) -> Vec<Effect> {
        let mut effects = Vec::new();
        self.viewport.handle_with_context(input, &mut effects);
        effects
    }

    /// Forge reset the stream: drop everything and load the tail again.
    pub(crate) fn reset(&mut self) -> Vec<Effect> {
        self.items.clear();
        self.positions.clear();
        self.handle(&Input::Open)
    }

    pub(crate) fn window(&self) -> &Viewport {
        self.viewport.inner()
    }

    /// Items on screen: whole turns, from the first loaded turn start up to
    /// a gap, if one is open below.
    ///
    /// A page often ends partway through a long turn. Showing that fragment
    /// above whole turns means every older page lands inside a row already
    /// on screen: it grows upward and the list loses the reader's place (a
    /// jump per page). Held back until its start loads, older pages only
    /// ever add whole turns above what shows. A fragment that is all there
    /// is (the newest turn, longer than what loaded) shows at once while
    /// its start loads: hiding it left only the status line on screen.
    pub(crate) fn shown(&self) -> impl DoubleEndedIterator<Item = &Item> {
        let end = self.window().gap.unwrap_or(Seq::MAX);
        let start = self.turn_start().unwrap_or(0);
        self.items
            .range(..=end)
            .filter(move |(position, _)| **position >= start)
            .map(|(_, item)| item)
    }

    /// Where the shown items begin: the oldest loaded turn start (a user
    /// message, not a steer), or the very first item once nothing older
    /// exists. `None` while the loaded window holds no turn start yet.
    fn turn_start(&self) -> Option<Seq> {
        if self.window().exhausted {
            return Some(0);
        }
        self.items
            .values()
            .find(|item| {
                item.parent_id.is_none()
                    && matches!(item.kind, ItemKind::UserMessage { steer: false, .. })
            })
            .map(|item| item.position)
    }

    /// The loaded window holds part of a turn but not its start: older
    /// pages must load before anything shows.
    pub(crate) fn needs_turn_start(&self) -> bool {
        !self.items.is_empty() && self.turn_start().is_none()
    }

    pub(crate) fn item(&self, id: &ItemId) -> Option<&Item> {
        self.items.get(self.positions.get(id)?)
    }

    pub(crate) fn position_of(&self, id: &ItemId) -> Option<Seq> {
        self.positions.get(id).copied()
    }

    /// Inserts or replaces by id; an older copy never replaces a newer one.
    fn upsert(&mut self, item: Item) -> bool {
        if let Some(&position) = self.positions.get(&item.id) {
            match self.items.get(&position) {
                Some(known) if known.seq > item.seq || *known == item => return false,
                _ => {}
            }
            self.items.remove(&position);
        }
        self.positions.insert(item.id.clone(), item.position);
        self.items.insert(item.position, item);
        true
    }

    /// An item from the event stream. Returns whether the view changed.
    pub(crate) fn apply_item(&mut self, item: Item) -> bool {
        if item.thread_id != self.thread {
            return false;
        }
        let known = self.positions.contains_key(&item.id);
        let window = self.window();
        if !known && !window.exhausted && window.oldest.is_some_and(|oldest| item.position < oldest)
        {
            // Above the window: it arrives with its page.
            return false;
        }
        // New while the thread is open: prose streams in like the reply,
        // and rows enter rather than appear.
        let arrived = !known;
        let id = item.id.clone();
        if !self.upsert(item) {
            return false;
        }
        if arrived {
            self.ui.entry(id).or_default().fresh = true;
        }
        if self.viewport.state().is_following() {
            self.evict_older();
        }
        true
    }

    /// A page arrived. Returns follow-up effects (another page while
    /// jumping, scrolling).
    pub(crate) fn apply_page(&mut self, fetch: Fetch, page: ItemsPage) -> Vec<Effect> {
        let first = page.items.first().map(|item| item.position);
        let last = page.items.last().map(|item| item.position);
        if fetch == Fetch::Tail {
            // A fresh tail replaces whatever window was loaded before it.
            let keep_from = first.unwrap_or(Seq::MAX);
            self.items.retain(|position, _| *position >= keep_from);
            self.positions.retain(|_, position| *position >= keep_from);
        }
        for item in page.items {
            if item.thread_id == self.thread {
                self.upsert(item);
            }
        }
        let mut effects = self.handle(&Input::Loaded {
            fetch,
            first,
            last,
            more: page.more,
        });
        match fetch {
            Fetch::Older { .. } if !self.viewport.state().is_following() => self.evict_newer(),
            _ => self.evict_older(),
        }
        if let State::Jumping {
            target: Jump::To(position),
            ..
        } = self.viewport.state().clone()
        {
            let loaded = self.items.contains_key(&position);
            effects.extend(self.handle(&Input::Present { position, loaded }));
        }
        effects
    }

    pub(crate) fn page_failed(&mut self) -> Vec<Effect> {
        self.handle(&Input::LoadFailed)
    }

    /// Moves the view to an item, loading older pages until it is in.
    pub(crate) fn jump(&mut self, jump: Jump) -> Vec<Effect> {
        let mut effects = self.handle(&Input::Jump(jump));
        if let Jump::To(position) = jump {
            let loaded = self.items.contains_key(&position);
            effects.extend(self.handle(&Input::Present { position, loaded }));
        }
        effects
    }

    fn evict_older(&mut self) {
        if self.items.len() <= WINDOW_MAX {
            return;
        }
        while self.items.len() > WINDOW_KEEP {
            if let Some((_, item)) = self.items.pop_first() {
                self.positions.remove(&item.id);
                self.ui.remove(&item.id);
            }
        }
        if let Some(&oldest) = self.items.keys().next() {
            self.handle(&Input::EvictedOlder { oldest });
        }
    }

    fn evict_newer(&mut self) {
        if self.items.len() <= WINDOW_MAX {
            return;
        }
        while self.items.len() > WINDOW_KEEP {
            if let Some((_, item)) = self.items.pop_last() {
                self.positions.remove(&item.id);
                self.ui.remove(&item.id);
            }
        }
        if let Some(&last) = self.items.keys().next_back() {
            self.handle(&Input::EvictedNewer { last });
        }
    }

    /// Replaces the live text with the host's current copy.
    pub(crate) fn set_live<'a>(
        &mut self,
        targets: impl Iterator<Item = (&'a LiveTarget, &'a str)>,
    ) -> bool {
        let live: HashMap<LiveTarget, String> = targets
            .map(|(target, text)| (target.clone(), text.to_owned()))
            .collect();
        if live == self.live {
            return false;
        }
        self.live = live;
        true
    }
}

/// The query for a fetch.
pub(crate) fn query(fetch: Fetch, limit: u32) -> ItemsQuery {
    let (before, after) = match fetch {
        Fetch::Tail => (None, None),
        Fetch::Older { before } => (Some(before), None),
        Fetch::Newer { after } => (None, Some(after)),
    };
    ItemsQuery {
        before,
        after,
        limit: Some(limit),
    }
}
