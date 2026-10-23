//! The open thread's one copy of its items: the loaded window keyed by
//! position, the live text, the viewport and per-item UI state.

use crate::viewport::{Effect, Fetch, Input, Jump, State, Viewport};
use artisan_api::{
    Item, ItemId, ItemsPage, ItemsQuery, LiveTarget, Run, RunId, Seq, ThreadId, WorkSpan,
};
use editor_core::ResidentBytes as _;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// Items per page of an agent's view, and of work loading into an open
/// section.
pub const PAGE: u32 = 100;
/// Rows per transcript page: a few screens of what the reader sees.
pub const ROWS_PAGE: u32 = 40;
const WINDOW_MAX: usize = 800;
const WINDOW_KEEP: usize = 600;
const WINDOW_MAX_BYTES: usize = 8 * 1024 * 1024;
const WINDOW_KEEP_BYTES: usize = 6 * 1024 * 1024;

/// How long after arriving an item still animates in.
const FRESH_FOR: std::time::Duration = std::time::Duration::from_secs(2);

/// UI state of one Item: disclosures and pending answers.
#[derive(Clone, Debug, Default, Hash)]
pub struct ItemUi {
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

pub struct ThreadView {
    pub thread: ThreadId,
    pub context: HashMap<ItemId, Arc<Item>>,
    /// The loaded window, by position.
    pub items: BTreeMap<Seq, Arc<Item>>,
    /// Work inside the window that is not loaded, by its first position:
    /// a closed section draws from this alone, an open one loads it newest
    /// first.
    pub work: BTreeMap<Seq, WorkSpan>,
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
            work: BTreeMap::new(),
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
        self.work.clear();
        self.positions.clear();
        self.resident_bytes = 0;
        self.live.clear();
        self.ui.clear();
        self.work_open.clear();
        self.runs.clear();
        self.handle(&Input::Open)
    }

    /// Keeps a run the transcript maps items through. The host holds only
    /// the latest run, so a live run must be kept here before a restart
    /// continuation replaces it, or its items lose their turn.
    pub(crate) fn remember_run(&mut self, run: Run) {
        if run.thread_id == self.thread
            && self.runs.get(&run.id).is_none_or(|old| old.seq <= run.seq)
        {
            self.runs.insert(run.id.clone(), run);
        }
    }

    pub(crate) const fn window(&self) -> &Viewport {
        &self.viewport
    }

    /// Every loaded item through the lower gap, including partial turns.
    pub(crate) fn shown(&self) -> impl DoubleEndedIterator<Item = &Item> {
        let end = self.window().gap.unwrap_or(Seq::MAX);
        self.items.range(..=end).map(|(_, item)| item.as_ref())
    }

    /// The unloaded work through the lower gap.
    pub(crate) fn shown_work(&self) -> impl DoubleEndedIterator<Item = &WorkSpan> {
        let end = self.window().gap.unwrap_or(Seq::MAX);
        self.work.range(..=end).map(|(_, span)| span)
    }

    /// The unloaded span holding `position`.
    fn unloaded_at(&self, position: Seq) -> Option<&WorkSpan> {
        self.work
            .range(..=position)
            .next_back()
            .map(|(_, span)| span)
            .filter(|span| position <= span.last)
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
        if !known && item.folds_into_work() && self.unloaded_at(item.position).is_some() {
            // Work not loaded: it arrives with its span when that opens.
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
        self.trim_window(None);
        true
    }

    /// A page arrived. Returns follow-up effects (another page while
    /// jumping, scrolling).
    pub(crate) fn apply_page(&mut self, fetch: Fetch, page: ItemsPage) -> Vec<Effect> {
        // A page for a request since replaced (a jump went elsewhere) would
        // land beside a window it doesn't border.
        if self.viewport.requested != Some(fetch) {
            return Vec::new();
        }
        for run in page.runs {
            self.remember_run(run);
        }
        let first = page
            .items
            .first()
            .map(|item| item.position)
            .into_iter()
            .chain(page.work.first().map(|span| span.first))
            .min();
        let last = page
            .items
            .last()
            .map(|item| item.position)
            .into_iter()
            .chain(page.work.last().map(|span| span.last))
            .max();
        if matches!(fetch, Fetch::Tail | Fetch::At { .. }) {
            // A fresh tail, or the page a jump landed on, replaces whatever
            // window was loaded before it: open work in it loads again as
            // it shows. Only what arrived live past a tail stays.
            let kept = |position: Seq| {
                matches!(fetch, Fetch::Tail) && last.is_some_and(|last| position > last)
            };
            self.items.retain(|position, _| kept(*position));
            self.work
                .retain(|_, span| kept(span.first) && kept(span.last));
            self.positions.retain(|_, position| kept(*position));
            self.resident_bytes = self.items.values().map(|item| item.resident_bytes()).sum();
            self.ui.retain(|id, _| self.positions.contains_key(id));
        }
        for span in page.work {
            self.work.insert(span.first, span);
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
        // Never trim the side a page just filled: it would load again.
        self.trim_window(match fetch {
            Fetch::Older { .. } => Some(false),
            Fetch::Newer { .. } => Some(true),
            Fetch::Tail | Fetch::At { .. } => None,
        });
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

    /// Work of the unloaded span at `first` arrived, newest first: what
    /// is still unloaded ends before it. Returns whether the view changed.
    pub(crate) fn apply_work(&mut self, first: Seq, page: ItemsPage) -> bool {
        let Some(span) = self.work.get_mut(&first) else {
            return false;
        };
        let oldest = page.items.first().map(|item| item.position);
        match oldest.filter(|_| page.more) {
            Some(oldest) => {
                span.last = oldest.saturating_sub(1);
                span.count = span
                    .count
                    .saturating_sub(u32::try_from(page.items.len()).unwrap_or(u32::MAX))
                    .max(1);
            }
            None => {
                self.work.remove(&first);
            }
        }
        for run in page.runs {
            self.remember_run(run);
        }
        for item in page.items {
            if item.thread_id == self.thread {
                self.upsert(Arc::new(item));
            }
        }
        self.trim_window(Some(false));
        true
    }

    pub(crate) fn page_failed(&mut self) -> Vec<Effect> {
        self.handle(&Input::LoadFailed)
    }

    /// Moves the view to an item, loading the page it is on when it isn't in.
    pub(crate) fn jump(&mut self, jump: Jump) -> Vec<Effect> {
        let mut effects = self.handle(&Input::Jump(jump));
        if let Jump::To(position) = jump {
            let loaded = self.items.contains_key(&position);
            effects.extend(self.handle(&Input::Present { position, loaded }));
        }
        effects
    }

    /// Bounds the window around what the reader sees. The span on screen
    /// is never evicted, and the budget always leaves room for a page or two
    /// beside it: one folded run can hold more items than the whole budget,
    /// and trimming into it (or the page below it) would drop the rows the
    /// reader is looking at, then reload and drop them again.
    /// `evict_older` limits trimming to one side.
    fn trim_window(&mut self, evict_older: Option<bool>) {
        let shown = match self.viewport.state() {
            State::Reading { anchor, through } => Some(*anchor..=(*through).max(*anchor)),
            State::Jumping {
                target: Jump::To(position),
                ..
            } => Some(*position..=*position),
            State::Following { .. } | State::Jumping { .. } => None,
        };
        let (shown_len, shown_bytes) = shown.clone().map_or((0, 0), |shown| {
            self.items
                .range(shown)
                .fold((0, 0), |(len, bytes), (_, item)| {
                    (len + 1, bytes + item.resident_bytes())
                })
        });
        let page = PAGE as usize;
        let slack = WINDOW_MAX_BYTES - WINDOW_KEEP_BYTES;
        let (max, keep) = (
            WINDOW_MAX.max(shown_len + 2 * page),
            WINDOW_KEEP.max(shown_len + page),
        );
        let (max_bytes, keep_bytes) = (
            WINDOW_MAX_BYTES.max(shown_bytes + slack),
            WINDOW_KEEP_BYTES.max(shown_bytes + slack / 2),
        );
        if self.items.len() <= max && self.resident_bytes <= max_bytes {
            return;
        }
        let mut before = shown.as_ref().map_or(self.items.len(), |shown| {
            self.items.range(..*shown.start()).count()
        });
        let mut after = shown.as_ref().map_or(0, |shown| {
            self.items
                .range((
                    std::ops::Bound::Excluded(*shown.end()),
                    std::ops::Bound::Unbounded,
                ))
                .count()
        });
        let (mut older, mut newer) = (false, false);
        // Keep a single oversized Item accessible; its body has its own rendering budget.
        if let Some(evict_older) = evict_older {
            // Only one side may go.
            if evict_older {
                after = 0;
            } else {
                before = 0;
            }
        }
        while self.items.len() > 1
            && (before + after) > 0
            && (self.items.len() > keep || self.resident_bytes > keep_bytes)
        {
            let take_older = before > after;
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
                before -= 1;
            } else {
                newer = true;
                after -= 1;
            }
        }
        if older && let Some(&oldest) = self.items.keys().next() {
            self.work.retain(|_, span| span.first > oldest);
            self.handle(&Input::EvictedOlder { oldest });
        }
        if newer && let Some(&last) = self.items.keys().next_back() {
            self.work.retain(|_, span| span.last < last);
            self.handle(&Input::EvictedNewer { last });
        }
        let runs: std::collections::HashSet<_> = self
            .items
            .values()
            .filter_map(|item| item.run_id.as_ref())
            .chain(self.work.values().filter_map(|span| span.run_id.as_ref()))
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
            .map(|(target, text)| (target.clone(), Arc::clone(text)))
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

/// The query for the next stretch of the unloaded span `span`: its newest
/// page.
pub const fn work_query(span: &WorkSpan) -> ItemsQuery {
    ItemsQuery {
        before: Some(span.last.saturating_add(1)),
        after: Some(span.first.saturating_sub(1)),
        limit: Some(PAGE),
    }
}

/// The query for a fetch.
pub const fn query(fetch: Fetch, limit: u32) -> ItemsQuery {
    let (before, after) = match fetch {
        Fetch::Tail => (None, None),
        Fetch::Older { before } => (Some(before), None),
        Fetch::Newer { after } => (None, Some(after)),
        Fetch::At { position } => (None, Some(position.saturating_sub(1))),
    };
    ItemsQuery {
        before,
        after,
        limit: Some(limit),
    }
}
