use std::{
    any::{Any, TypeId},
    collections::VecDeque,
    marker::PhantomData,
    mem::size_of,
    sync::{
        Arc, Mutex, MutexGuard, Weak,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::task::AbortHandle;

const MAX_ENTRIES: usize = 32;
const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_PENDING: usize = 8;
static NEXT_CACHE: AtomicU64 = AtomicU64::new(1);

type Output = Box<dyn Any + Send>;
type Release = Box<dyn FnOnce(Output) + Send>;
type Rejected = Box<dyn FnOnce() + Send>;

struct Waiter {
    id: u64,
    wake: aqua::mailbox::Wake,
}

fn enlist(waiters: &mut Vec<Waiter>, id: u64, wake: aqua::mailbox::Wake) {
    if !waiters.iter().any(|known| known.id == id) {
        waiters.push(Waiter { id, wake });
    }
}

enum Slot {
    Pending {
        waiters: Vec<Waiter>,
        cancel: Option<AbortHandle>,
    },
    Ready {
        output: Output,
        release: Option<Release>,
    },
    TooLarge,
}

struct Entry {
    loader: TypeId,
    owners: Vec<Arc<[u64]>>,
    source: Box<dyn Any + Send>,
    bytes: usize,
    generation: u64,
    slot: Slot,
}

impl Entry {
    fn resident_bytes(&self) -> usize {
        let waiters = match &self.slot {
            Slot::Pending { waiters, .. } => waiters.capacity(),
            Slot::Ready { .. } | Slot::TooLarge => 0,
        };
        self.bytes
            .saturating_add(
                self.owners
                    .capacity()
                    .saturating_mul(size_of::<Arc<[u64]>>()),
            )
            .saturating_add(waiters.saturating_mul(size_of::<Waiter>()))
            .saturating_add(self.owners.iter().map(|owner| membership(owner)).sum())
    }

    fn pending(&self) -> bool {
        matches!(self.slot, Slot::Pending { .. })
    }

    fn matches<L: ResourceLoader>(&self, source: &L::Source) -> bool {
        self.loader == TypeId::of::<L>() && self.source.downcast_ref::<L::Source>() == Some(source)
    }

    fn is_pending_ticket<L: ResourceLoader>(&self, ticket: &LoadTicket<L>) -> bool {
        self.generation == ticket.generation && self.pending() && self.matches::<L>(&ticket.source)
    }
}

impl Drop for Entry {
    fn drop(&mut self) {
        match &mut self.slot {
            Slot::Pending { waiters, cancel } => {
                if let Some(cancel) = cancel.take() {
                    cancel.abort();
                }
                for waiter in waiters.drain(..) {
                    (waiter.wake)();
                }
            }
            Slot::Ready { output, release } => {
                if let Some(release) = release.take() {
                    release(std::mem::replace(output, Box::new(())));
                }
            }
            Slot::TooLarge => {}
        }
    }
}

fn membership(owner: &[u64]) -> usize {
    owner
        .len()
        .saturating_mul(size_of::<u64>())
        .saturating_add(2 * size_of::<usize>())
}

struct State {
    closed: bool,
    generation: u64,
    entries: VecDeque<Entry>,
    admission: Vec<Waiter>,
}

impl State {
    fn resident_bytes(&self) -> usize {
        self.entries.iter().map(Entry::resident_bytes).sum()
    }

    fn pending(&self) -> usize {
        self.entries.iter().filter(|entry| entry.pending()).count()
    }

    fn pending_bytes(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.pending())
            .map(Entry::resident_bytes)
            .sum()
    }

    fn evict_until(
        &mut self,
        kept: Option<u64>,
        fits: impl Fn(&Self) -> bool,
    ) -> (Vec<Entry>, bool) {
        let mut removed = Vec::new();
        while !fits(self) {
            let Some(position) = self
                .entries
                .iter()
                .position(|entry| !entry.pending() && Some(entry.generation) != kept)
            else {
                return (removed, false);
            };
            removed.push(
                self.entries
                    .remove(position)
                    .expect("invariant: an evictable position is inside the entries"),
            );
        }
        (removed, true)
    }

    fn admitted(&mut self, freed: bool) -> Vec<Waiter> {
        if freed {
            std::mem::take(&mut self.admission)
        } else {
            Vec::new()
        }
    }
}

struct Settled {
    removed: Vec<Entry>,
    wake: Vec<Waiter>,
    release: Option<Rejected>,
}

impl Settled {
    fn run(self) {
        drop(self.removed);
        if let Some(release) = self.release {
            release();
        }
        for waiter in self.wake {
            (waiter.wake)();
        }
    }
}

pub struct ResourceCache {
    id: u64,
    state: Mutex<State>,
}

pub trait ResourceLoader: 'static {
    type Source: Clone + PartialEq + Send + 'static;
    type Output: Clone + Send + 'static;
}

pub(crate) enum Load<L: ResourceLoader> {
    Ready(L::Output),
    Pending,
    Start(LoadTicket<L>),
    TooLarge,
}

pub struct LoadTicket<L: ResourceLoader> {
    cache: Weak<ResourceCache>,
    source: L::Source,
    generation: u64,
    source_bytes: usize,
    marker: PhantomData<fn() -> L>,
}

impl Default for ResourceCache {
    fn default() -> Self {
        Self {
            id: NEXT_CACHE.fetch_add(1, Ordering::Relaxed),
            state: Mutex::new(State {
                closed: false,
                generation: 0,
                entries: VecDeque::new(),
                admission: Vec::new(),
            }),
        }
    }
}

impl ResourceCache {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    pub(crate) fn load<L: ResourceLoader>(
        self: &Arc<Self>,
        owner: &Arc<[u64]>,
        source: &L::Source,
        source_bytes: usize,
        waiter: u64,
        wake: aqua::mailbox::Wake,
    ) -> Load<L> {
        let mut state = self.state();
        if state.closed {
            return Load::Pending;
        }
        if let Some(position) = state
            .entries
            .iter()
            .position(|entry| entry.matches::<L>(source))
        {
            let mut entry = state
                .entries
                .remove(position)
                .expect("invariant: a found position is inside the entries");
            if !entry.owners.iter().any(|known| known == owner) {
                entry.owners.push(Arc::clone(owner));
            }
            let load = match &mut entry.slot {
                Slot::Pending { waiters, .. } => {
                    enlist(waiters, waiter, wake);
                    Load::Pending
                }
                Slot::Ready { output, .. } => Load::Ready(
                    output
                        .downcast_ref::<L::Output>()
                        .expect("invariant: an entry's output has its loader's type")
                        .clone(),
                ),
                Slot::TooLarge => Load::TooLarge,
            };
            let generation = entry.generation;
            state.entries.push_back(entry);
            let (removed, _) = state.evict_until(Some(generation), |state| {
                state.resident_bytes() <= MAX_BYTES
            });
            let wake = state.admitted(!removed.is_empty());
            drop(state);
            Settled {
                removed,
                wake,
                release: None,
            }
            .run();
            return load;
        }
        let admission = membership(owner)
            .saturating_add(size_of::<Arc<[u64]>>())
            .saturating_add(size_of::<Waiter>());
        let needed = source_bytes.saturating_add(admission);
        if needed > MAX_BYTES {
            return Load::TooLarge;
        }
        if state.pending() >= MAX_PENDING
            || state.pending_bytes().saturating_add(needed) > MAX_BYTES
        {
            enlist(&mut state.admission, waiter, wake);
            return Load::Pending;
        }
        let (removed, fits) = state.evict_until(None, |state| {
            state.entries.len() < MAX_ENTRIES
                && state.resident_bytes().saturating_add(needed) <= MAX_BYTES
        });
        assert!(
            fits,
            "invariant: pending entries leave room for {needed} bytes in cache {}",
            self.id
        );
        state.generation = state.generation.wrapping_add(1);
        let generation = state.generation;
        state.entries.push_back(Entry {
            loader: TypeId::of::<L>(),
            owners: vec![Arc::clone(owner)],
            source: Box::new(source.clone()),
            bytes: source_bytes,
            generation,
            slot: Slot::Pending {
                waiters: vec![Waiter { id: waiter, wake }],
                cancel: None,
            },
        });
        drop(state);
        drop(removed);
        Load::Start(LoadTicket {
            cache: Arc::downgrade(self),
            source: source.clone(),
            generation,
            source_bytes,
            marker: PhantomData,
        })
    }

    fn remove(&self, mut removed: impl FnMut(&mut Entry) -> bool) -> bool {
        let mut state = self.state();
        let mut kept = VecDeque::with_capacity(state.entries.len());
        let mut gone = Vec::new();
        for mut entry in state.entries.drain(..) {
            if removed(&mut entry) {
                gone.push(entry);
            } else {
                kept.push_back(entry);
            }
        }
        state.entries = kept;
        let any = !gone.is_empty();
        let wake = state.admitted(any);
        drop(state);
        Settled {
            removed: gone,
            wake,
            release: None,
        }
        .run();
        any
    }

    pub(crate) fn forget<L: ResourceLoader>(&self, source: &L::Source) {
        self.remove(|entry| entry.matches::<L>(source));
    }

    pub(crate) fn retain_owners(&self, owners: &[Vec<u64>]) {
        self.remove(|entry| {
            entry
                .owners
                .retain(|owner| owners.iter().any(|prefix| owner.starts_with(prefix)));
            entry.owners.is_empty()
        });
    }

    pub(crate) fn close(&self) {
        let mut state = self.state();
        state.closed = true;
        let entries = std::mem::take(&mut state.entries);
        let wake = std::mem::take(&mut state.admission);
        drop(state);
        Settled {
            removed: entries.into(),
            wake,
            release: None,
        }
        .run();
    }
}

impl<L: ResourceLoader> LoadTicket<L> {
    pub(crate) fn current(&self) -> bool {
        let Some(cache) = self.cache.upgrade() else {
            return false;
        };
        let state = cache.state();
        state
            .entries
            .iter()
            .any(|entry| entry.is_pending_ticket(self))
    }

    pub(crate) fn cancel(&self) -> bool {
        let Some(cache) = self.cache.upgrade() else {
            return false;
        };
        let mut found = false;
        cache.remove(|entry| {
            let matched = !found && entry.is_pending_ticket(self);
            found |= matched;
            matched
        })
    }

    pub(crate) fn attach(&self, abort: AbortHandle) -> bool {
        let Some(cache) = self.cache.upgrade() else {
            abort.abort();
            return false;
        };
        let mut state = cache.state();
        let slot = state
            .entries
            .iter_mut()
            .find(|entry| entry.is_pending_ticket(self))
            .map(|entry| &mut entry.slot);
        match slot {
            Some(Slot::Pending { cancel, .. }) if cancel.is_none() => {
                *cancel = Some(abort);
                true
            }
            Some(_) | None => {
                drop(state);
                abort.abort();
                false
            }
        }
    }

    pub(crate) fn publish(
        self,
        output: L::Output,
        output_bytes: usize,
        release: impl FnOnce(L::Output) + Send + 'static,
    ) -> bool {
        let Some(cache) = self.cache.upgrade() else {
            release(output);
            return false;
        };
        let mut state = cache.state();
        let Some(position) = state
            .entries
            .iter()
            .position(|entry| entry.is_pending_ticket(&self))
        else {
            drop(state);
            release(output);
            return false;
        };
        let mut entry = state
            .entries
            .remove(position)
            .expect("invariant: a found position is inside the entries");
        let Slot::Pending { waiters, .. } = std::mem::replace(&mut entry.slot, Slot::TooLarge)
        else {
            unreachable!(
                "invariant: generation {} matched a pending entry",
                self.generation
            );
        };
        entry.bytes = self.source_bytes.saturating_add(output_bytes);
        let retained = entry.resident_bytes() <= MAX_BYTES;
        let rejected = if retained {
            let generation = self.generation;
            entry.slot = Slot::Ready {
                output: Box::new(output),
                release: Some(Box::new(move |output: Output| {
                    release(*output.downcast::<L::Output>().unwrap_or_else(|_| {
                        unreachable!(
                            "invariant: entry {generation} stores its loader's output type"
                        )
                    }));
                })),
            };
            None
        } else {
            entry.bytes = self.source_bytes;
            let rejected: Rejected = Box::new(move || release(output));
            Some(rejected)
        };
        state.entries.push_back(entry);
        let (removed, _) = state.evict_until(Some(self.generation), |state| {
            state.entries.len() <= MAX_ENTRIES && state.resident_bytes() <= MAX_BYTES
        });
        let mut wake = waiters;
        wake.extend(state.admitted(true));
        drop(state);
        Settled {
            removed,
            wake,
            release: rejected,
        }
        .run();
        true
    }
}

#[cfg(test)]
#[path = "../../../tests/ui/media/resource_cache.rs"]
mod tests;
