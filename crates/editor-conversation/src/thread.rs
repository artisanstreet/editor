//! The open thread's one copy of its items: the loaded window keyed by
//! position, the live text, the viewport and per-item UI state.

use crate::viewport::{Effect, Fetch, Input, Jump, State, Viewport};
use artisan_api::{Item, ItemId, ItemsPage, ItemsQuery, LiveTarget, Run, RunId, Seq, ThreadId};
use editor_core::ResidentBytes as _;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// Items per page.
pub(crate) const PAGE: u32 = 100;
const WINDOW_MAX: usize = 800;
const WINDOW_KEEP: usize = 600;
const WINDOW_MAX_BYTES: usize = 8 * 1024 * 1024;
const WINDOW_KEEP_BYTES: usize = 6 * 1024 * 1024;

/// How long after arriving an item still animates in.
const FRESH_FOR: std::time::Duration = std::time::Duration::from_secs(2);

/// UI state of one Item: disclosures and pending answers.
#[derive(Clone, Debug, Default, Hash)]
pub(crate) struct ItemUi {
    /// The user opened or closed the item's details.
    pub expanded: Option<bool>,
    /// When it arrived while the reader was watching the open thread:
    /// compact rows may fade in. Only for a moment: shown any later (an agent's view
    /// opened, a chain unfolded) it is history and appears at once.
    pub arrived: Option<std::time::Instant>,
    /// The user opened or closed the activity chain this item starts.
    pub chain_open: Option<bool>,
    /// An answer is on its way to Forge.
    pub sending: bool,

    /// Why the last answer failed.
    pub error: Option<String>,
}

impl ItemUi {
    /// It arrived while the reader watched, a moment ago: it animates in.
    pub(crate) fn fresh(&self) -> bool {
        self.arrived
            .is_some_and(|arrived| arrived.elapsed() < FRESH_FOR)
    }
}

pub(crate) struct ThreadView {
    pub thread: ThreadId,
    pub context: HashMap<ItemId, Arc<Item>>,
    /// The loaded window, by position.
    pub items: BTreeMap<Seq, Arc<Item>>,
    positions: HashMap<ItemId, Seq>,
    resident_bytes: usize,
    /// Memory-only text of the live run.
    pub live: HashMap<LiveTarget, Arc<ropey::Rope>>,
    pub viewport: Viewport,
    pub ui: HashMap<ItemId, ItemUi>,
    /// The user's fold choice per run; `None` follows the derived default.
    pub work_open: HashMap<RunId, bool>,
    pub runs: HashMap<RunId, Run>,
}

impl ThreadView {
    /// A view of `thread` and the effects that load it.
    pub(crate) fn open(thread: ThreadId) -> (Self, Vec<Effect>) {
        let mut view = Self {
            thread,
            context: HashMap::new(),
            items: BTreeMap::new(),
            positions: HashMap::new(),
            resident_bytes: 0,
            live: HashMap::new(),
            viewport: Viewport::default(),
            ui: HashMap::new(),
            work_open: HashMap::new(),
            runs: HashMap::new(),
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
        self.resident_bytes = 0;
        self.live.clear();
        self.ui.clear();
        self.work_open.clear();
        self.runs.clear();
        self.handle(&Input::Open)
    }

    pub(crate) fn window(&self) -> &Viewport {
        &self.viewport
    }

    /// Every loaded item through the lower gap, including partial turns.
    pub(crate) fn shown(&self) -> impl DoubleEndedIterator<Item = &Item> {
        let end = self.window().gap.unwrap_or(Seq::MAX);
        self.items.range(..=end).map(|(_, item)| item.as_ref())
    }

    pub(crate) fn item(&self, id: &ItemId) -> Option<&Item> {
        self.positions
            .get(id)
            .and_then(|position| self.items.get(position))
            .or_else(|| self.context.get(id))
            .map(Arc::as_ref)
    }

    pub(crate) fn shared_item(&self, id: &ItemId) -> Option<Arc<Item>> {
        self.positions
            .get(id)
            .and_then(|position| self.items.get(position))
            .or_else(|| self.context.get(id))
            .cloned()
    }

    pub(crate) fn position_of(&self, id: &ItemId) -> Option<Seq> {
        self.positions.get(id).copied()
    }

    /// Inserts or replaces by id; an older copy never replaces a newer one.
    fn upsert(&mut self, item: Arc<Item>) -> bool {
        if let Some(&position) = self.positions.get(&item.id) {
            match self.items.get(&position) {
                Some(known) if known.seq > item.seq || *known == item => return false,
                _ => {}
            }
            if let Some(old) = self.items.remove(&position) {
                self.resident_bytes -= old.resident_bytes();
            }
        }
        self.resident_bytes += item.resident_bytes();
        self.positions.insert(item.id.clone(), item.position);
        self.items.insert(item.position, item);
        true
    }

    /// An item from the event stream. Returns whether the view changed.
    /// An item from the live stream. `watched`: the reader sees the thread
    /// (its window active, its transcript landed), so a new item animates.
    pub(crate) fn apply_item(&mut self, item: impl Into<Arc<Item>>, watched: bool) -> bool {
        let item = item.into();
        if item.thread_id != self.thread {
            return false;
        }
        if self.context.contains_key(&item.id) {
            if self
                .context
                .get(&item.id)
                .is_some_and(|known| known.seq >= item.seq)
            {
                return false;
            }
            self.context.insert(item.id.clone(), item);
            return true;
        }
        let known = self.positions.contains_key(&item.id);
        let window = self.window();
        if !known && !window.exhausted && window.oldest.is_some_and(|oldest| item.position < oldest)
        {
            // Above the window: it arrives with its page.
            return false;
        }
        // New while the reader watches: prose streams in like the reply,
        // and rows enter rather than appear.
        let arrived = !known && watched;
        let id = item.id.clone();
        if !self.upsert(item) {
            return false;
        }
        if arrived {
            self.ui.entry(id).or_default().arrived = Some(std::time::Instant::now());
        }
        self.trim_window();
        true
    }

    /// A page arrived. Returns follow-up effects (another page while
    /// jumping, scrolling).
    pub(crate) fn apply_page(&mut self, fetch: Fetch, page: ItemsPage) -> Vec<Effect> {
        for run in page.runs {
            if run.thread_id == self.thread
                && self.runs.get(&run.id).is_none_or(|old| old.seq <= run.seq)
            {
                self.runs.insert(run.id.clone(), run);
            }
        }
        let first = page.items.first().map(|item| item.position);
        let last = page.items.last().map(|item| item.position);
        if fetch == Fetch::Tail {
            // A fresh tail replaces whatever window was loaded before it.
            let keep_from = first.unwrap_or(Seq::MAX);
            self.items.retain(|position, _| *position >= keep_from);
            self.positions.retain(|_, position| *position >= keep_from);
            self.resident_bytes = self.items.values().map(|item| item.resident_bytes()).sum();
            self.ui.retain(|id, _| self.positions.contains_key(id));
        }
        for item in page.items {
            if item.thread_id == self.thread {
                self.upsert(Arc::new(item));
            }
        }
        let mut effects = self.handle(&Input::Loaded {
            fetch,
            first,
            last,
            more: page.more,
        });
        self.trim_window();
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

    fn trim_window(&mut self) {
        if self.items.len() <= WINDOW_MAX && self.resident_bytes <= WINDOW_MAX_BYTES {
            return;
        }
        let anchor = match self.viewport.state() {
            State::Reading { anchor } => Some(*anchor),
            State::Jumping {
                target: Jump::To(position),
                ..
            } => Some(*position),
            _ => None,
        };
        let mut before = anchor.map_or(self.items.len(), |position| {
            self.items.range(..position).count()
        });
        let (mut older, mut newer) = (false, false);
        // Keep a single oversized Item accessible; its body has its own rendering budget.
        while self.items.len() > 1
            && (self.items.len() > WINDOW_KEEP || self.resident_bytes > WINDOW_KEEP_BYTES)
        {
            let take_older = anchor.is_none() || before > self.items.len() / 2;
            let removed = if take_older {
                self.items.pop_first()
            } else {
                self.items.pop_last()
            };
            let Some((_, item)) = removed else { break };
            self.resident_bytes -= item.resident_bytes();
            self.positions.remove(&item.id);
            self.ui.remove(&item.id);
            if take_older {
                older = true;
                before = before.saturating_sub(1);
            } else {
                newer = true;
            }
        }
        if older && let Some(&oldest) = self.items.keys().next() {
            self.handle(&Input::EvictedOlder { oldest });
        }
        if newer && let Some(&last) = self.items.keys().next_back() {
            self.handle(&Input::EvictedNewer { last });
        }
        let runs: std::collections::HashSet<_> = self
            .items
            .values()
            .filter_map(|item| item.run_id.as_ref())
            .collect();
        self.work_open.retain(|run, _| runs.contains(run));
        self.runs.retain(|run, _| runs.contains(run));
    }

    /// A live frame replaces only its named target; other snapshots keep their identity.
    pub(crate) fn update_live(
        &mut self,
        target: &LiveTarget,
        text: Option<Arc<ropey::Rope>>,
    ) -> bool {
        match (self.live.get(target), &text) {
            (Some(old), Some(next)) if Arc::ptr_eq(old, next) => return false,
            (None, None) => return false,
            _ => {}
        }
        if let Some(text) = text {
            self.live.insert(target.clone(), text);
        } else {
            self.live.remove(target);
        }
        true
    }

    /// Replaces the live text with the host's current copy.
    pub(crate) fn set_live<'a>(
        &mut self,
        targets: impl Iterator<Item = (&'a LiveTarget, &'a Arc<ropey::Rope>)>,
    ) -> bool {
        let live: HashMap<LiveTarget, Arc<ropey::Rope>> = targets
            .map(|(target, text)| (target.clone(), text.clone()))
            .collect();
        if live.len() == self.live.len()
            && live.iter().all(|(target, text)| {
                self.live
                    .get(target)
                    .is_some_and(|old| Arc::ptr_eq(old, text))
            })
        {
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
