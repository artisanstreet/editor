use super::viewport::{Effect, Fetch, Input, Jump, State, Viewport};
use artisan_api::{
    Item, ItemId, ItemsPage, ItemsQuery, LiveTarget, Run, RunId, Seq, ThreadId, WorkSpan,
};
use editor_core::ResidentBytes as _;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

pub const PAGE: u32 = 100;
pub const ROWS_PAGE: u32 = 40;
const WINDOW_MAX: usize = 800;
const WINDOW_KEEP: usize = 600;
const WINDOW_MAX_BYTES: usize = 8 * 1024 * 1024;
const WINDOW_KEEP_BYTES: usize = 6 * 1024 * 1024;

const FRESH_FOR: std::time::Duration = std::time::Duration::from_secs(2);

#[derive(Clone, Debug, Default, Hash)]
pub struct ItemUi {
    pub expanded: Option<bool>,
    pub arrived: Option<std::time::Instant>,
    pub chain_open: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AnswerRequest(pub(crate) u64);

impl AnswerRequest {
    #[cfg(feature = "test-support")]
    #[must_use]
    pub const fn for_tests(request: u64) -> Self {
        Self(request)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum AnswerState {
    Sending(AnswerRequest),
    Failed(String),
}

impl AnswerState {
    #[must_use]
    pub const fn sending(&self) -> bool {
        matches!(self, Self::Sending(_))
    }

    #[must_use]
    pub fn error(&self) -> Option<&str> {
        match self {
            Self::Sending(_) => None,
            Self::Failed(error) => Some(error),
        }
    }
}

impl ItemUi {
    pub(crate) fn fresh(&self, now: std::time::Instant) -> bool {
        self.arrived
            .is_some_and(|arrived| now.saturating_duration_since(arrived) < FRESH_FOR)
    }
}

#[derive(Clone)]
pub struct ThreadView {
    pub thread: ThreadId,
    pub context: HashMap<ItemId, Arc<Item>>,
    pub items: BTreeMap<Seq, Arc<Item>>,
    pub work: BTreeMap<Seq, WorkSpan>,
    positions: HashMap<ItemId, Seq>,
    resident_bytes: usize,
    pub live: HashMap<LiveTarget, Arc<ropey::Rope>>,
    pub viewport: Viewport,
    pub ui: HashMap<ItemId, ItemUi>,
    pub answers: HashMap<ItemId, AnswerState>,
    pub work_open: HashMap<RunId, bool>,
    pub runs: HashMap<RunId, Run>,
}

impl ThreadView {
    pub fn open(thread: ThreadId) -> (Self, Vec<Effect>) {
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
            answers: HashMap::new(),
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

    pub(crate) fn reset(&mut self) -> Vec<Effect> {
        self.items.clear();
        self.work.clear();
        self.positions.clear();
        self.resident_bytes = 0;
        self.live.clear();
        self.ui.clear();
        self.answers.clear();
        self.work_open.clear();
        self.runs.clear();
        self.handle(&Input::Open)
    }

    pub(crate) fn abandon_answers(&mut self) {
        self.answers.retain(|_, state| !state.sending());
    }

    pub(crate) fn resume(&mut self) -> Vec<Effect> {
        self.handle(&Input::Resume)
    }

    pub const fn resident_bytes(&self) -> usize {
        self.resident_bytes
    }

    pub fn parked_bytes(&self) -> usize {
        let items = self.items.len() + self.context.len();
        self.resident_bytes
            + self
                .context
                .values()
                .map(|item| item.resident_bytes())
                .sum::<usize>()
            + self
                .live
                .values()
                .map(|rope| rope.capacity())
                .sum::<usize>()
            + items * size_of::<Item>()
            + self.items.len() * size_of::<(Seq, Arc<Item>)>()
            + self.context.capacity() * size_of::<(ItemId, Arc<Item>)>()
            + self.work.len() * size_of::<(Seq, WorkSpan)>()
            + self.positions.capacity() * size_of::<(ItemId, Seq)>()
            + self.ui.capacity() * size_of::<(ItemId, ItemUi)>()
            + self.answers.capacity() * size_of::<(ItemId, AnswerState)>()
            + self.work_open.capacity() * size_of::<(RunId, bool)>()
            + self.runs.capacity() * size_of::<(RunId, Run)>()
    }

    pub(crate) fn remember_run(&mut self, run: Run) {
        if run.thread_id == self.thread
            && self.runs.get(&run.id).is_none_or(|old| old.seq <= run.seq)
        {
            self.runs.insert(run.id.clone(), run);
        }
    }

    pub const fn window(&self) -> &Viewport {
        &self.viewport
    }

    pub(crate) fn shown(&self) -> impl DoubleEndedIterator<Item = &Item> {
        let end = self.window().gap.unwrap_or(Seq::MAX);
        self.items.range(..=end).map(|(_, item)| item.as_ref())
    }

    pub(crate) fn shown_work(&self) -> impl DoubleEndedIterator<Item = &WorkSpan> {
        let end = self.window().gap.unwrap_or(Seq::MAX);
        self.work.range(..=end).map(|(_, span)| span)
    }

    fn unloaded_at(&self, position: Seq) -> Option<&WorkSpan> {
        self.work
            .range(..=position)
            .next_back()
            .map(|(_, span)| span)
            .filter(|span| position <= span.last)
    }

    pub fn item(&self, id: &ItemId) -> Option<&Item> {
        self.positions
            .get(id)
            .and_then(|position| self.items.get(position))
            .or_else(|| self.context.get(id))
            .map(Arc::as_ref)
    }

    pub fn shared_item(&self, id: &ItemId) -> Option<Arc<Item>> {
        self.positions
            .get(id)
            .and_then(|position| self.items.get(position))
            .or_else(|| self.context.get(id))
            .cloned()
    }

    pub(crate) fn position_of(&self, id: &ItemId) -> Option<Seq> {
        self.positions.get(id).copied()
    }

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

    pub fn apply_item(
        &mut self,
        item: impl Into<Arc<Item>>,
        watched_at: Option<std::time::Instant>,
    ) -> bool {
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
            return false;
        }
        if !known && item.folds_into_work() && self.unloaded_at(item.position).is_some() {
            return false;
        }
        let arrived = watched_at.filter(|_| !known);
        let id = item.id.clone();
        if !self.upsert(item) {
            return false;
        }
        if let Some(at) = arrived {
            self.ui.entry(id).or_default().arrived = Some(at);
        }
        self.trim_window(None);
        true
    }

    pub fn apply_page(&mut self, fetch: Fetch, page: ItemsPage) -> Vec<Effect> {
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
        let newest = self
            .items
            .last_key_value()
            .map(|(position, _)| *position)
            .into_iter()
            .chain(self.work.last_key_value().map(|(_, span)| span.last))
            .max();
        let fetch = match fetch {
            Fetch::Refresh if !refreshes(first, newest, page.more) => Fetch::Tail,
            fetch => fetch,
        };
        let newer: Vec<Arc<Item>> = page
            .items
            .iter()
            .filter_map(|item| {
                let known = self.items.get(self.positions.get(&item.id)?)?;
                (known.seq > item.seq).then(|| Arc::clone(known))
            })
            .collect();
        if fetch == Fetch::Refresh {
            let kept = |position: Seq| {
                first.is_none_or(|first| position < first)
                    || last.is_some_and(|last| position > last)
            };
            self.items.retain(|position, _| kept(*position));
            self.work
                .retain(|_, span| kept(span.first) && kept(span.last));
            self.positions.retain(|_, position| kept(*position));
            self.resident_bytes = self.items.values().map(|item| item.resident_bytes()).sum();
        }
        if matches!(fetch, Fetch::Tail | Fetch::At { .. }) {
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
        for item in newer {
            self.upsert(item);
        }
        let mut effects = self.handle(&Input::Loaded {
            fetch,
            first,
            last,
            more: page.more,
        });
        self.trim_window(match fetch {
            Fetch::Older { .. } => Some(false),
            Fetch::Newer { .. } => Some(true),
            Fetch::Tail | Fetch::Refresh | Fetch::At { .. } => None,
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

    pub(crate) fn jump(&mut self, jump: Jump) -> Vec<Effect> {
        let mut effects = self.handle(&Input::Jump(jump));
        if let Jump::To(position) = jump {
            let loaded = self.items.contains_key(&position);
            effects.extend(self.handle(&Input::Present { position, loaded }));
        }
        effects
    }

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
            self.items.range(shown.clone()).fold(
                (self.work.range(shown).count(), 0),
                |(len, bytes), (_, item)| (len + 1, bytes + item.resident_bytes()),
            )
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
        if self.entries() <= max && self.resident_bytes <= max_bytes {
            return;
        }
        let mut before = shown.as_ref().map_or_else(
            || self.entries(),
            |shown| {
                self.items.range(..*shown.start()).count()
                    + self.work.range(..*shown.start()).count()
            },
        );
        let mut after = shown.as_ref().map_or(0, |shown| {
            let later = (
                std::ops::Bound::Excluded(*shown.end()),
                std::ops::Bound::Unbounded,
            );
            self.items.range(later).count() + self.work.range(later).count()
        });
        let (mut older, mut newer) = (false, false);
        if let Some(evict_older) = evict_older {
            if evict_older {
                after = 0;
            } else {
                before = 0;
            }
        }
        while self.entries() > 1
            && (before + after) > 0
            && (self.entries() > keep || self.resident_bytes > keep_bytes)
        {
            let take_older = before > after;
            if !self.evict(take_older) {
                break;
            }
            if take_older {
                older = true;
                before -= 1;
            } else {
                newer = true;
                after -= 1;
            }
        }
        let first_item = self.items.keys().next().copied();
        let first_span = self.work.keys().next().copied();
        if older && let Some(oldest) = first_item.into_iter().chain(first_span).min() {
            self.handle(&Input::EvictedOlder { oldest });
        }
        let last_item = self.items.keys().next_back().copied();
        let last_span = self.work.values().next_back().map(|span| span.last);
        if newer && let Some(last) = last_item.into_iter().chain(last_span).max() {
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

    fn entries(&self) -> usize {
        self.items.len() + self.work.len()
    }

    fn evict(&mut self, older: bool) -> bool {
        let (item, span) = if older {
            (
                self.items.keys().next().copied(),
                self.work.keys().next().copied(),
            )
        } else {
            (
                self.items.keys().next_back().copied(),
                self.work.keys().next_back().copied(),
            )
        };
        let from_items = match (item, span) {
            (Some(item), Some(span)) => (item < span) == older,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => return false,
        };
        if !from_items {
            if older {
                self.work.pop_first();
            } else {
                self.work.pop_last();
            }
            return true;
        }
        let removed = if older {
            self.items.pop_first()
        } else {
            self.items.pop_last()
        };
        if let Some((_, item)) = removed {
            self.resident_bytes -= item.resident_bytes();
            self.positions.remove(&item.id);
            self.ui.remove(&item.id);
        }
        true
    }

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

#[must_use]
pub fn refreshes(first: Option<Seq>, newest: Option<Seq>, more: bool) -> bool {
    !more || matches!((first, newest), (Some(first), Some(newest)) if first <= newest)
}

pub const fn work_query(span: &WorkSpan) -> ItemsQuery {
    ItemsQuery {
        before: Some(span.last.saturating_add(1)),
        after: Some(span.first.saturating_sub(1)),
        limit: Some(PAGE),
    }
}

pub const fn query(fetch: Fetch, limit: u32) -> ItemsQuery {
    let (before, after) = match fetch {
        Fetch::Tail | Fetch::Refresh => (None, None),
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

#[cfg(test)]
#[path = "../../tests/conversation/thread.rs"]
mod tests;
