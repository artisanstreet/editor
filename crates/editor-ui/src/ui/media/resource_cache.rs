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

type Release = Box<dyn FnOnce() + Send>;

struct Waiter {
    id: u64,
    wake: aqua::mailbox::Wake,
}

struct Entry {
    loader: TypeId,
    owners: Vec<Arc<[u64]>>,
    waiters: Vec<Waiter>,
    source: Box<dyn Any + Send>,
    output: Option<Box<dyn Any + Send>>,
    bytes: usize,
    generation: u64,
    cancel: Option<AbortHandle>,
    release: Option<Release>,
}

impl Entry {
    fn resident_bytes(&self) -> usize {
        self.bytes
            .saturating_add(
                self.owners
                    .capacity()
                    .saturating_mul(size_of::<Arc<[u64]>>()),
            )
            .saturating_add(self.waiters.capacity().saturating_mul(size_of::<Waiter>()))
            .saturating_add(
                self.owners
                    .iter()
                    .map(|owner| {
                        owner
                            .len()
                            .saturating_mul(size_of::<u64>())
                            .saturating_add(2 * size_of::<usize>())
                    })
                    .sum::<usize>(),
            )
    }

    fn dispose(self) {
        drop(self);
    }
}

impl Drop for Entry {
    fn drop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.abort();
        }
        if let Some(release) = self.release.take() {
            release();
        }
    }
}

struct State {
    closed: bool,
    generation: u64,
    entries: VecDeque<Entry>,
}

pub struct ResourceCache {
    id: u64,
    state: Mutex<State>,
}

pub trait ResourceLoader: 'static {
    type Source: Clone + PartialEq + Send + 'static;
    type Output: Clone + Send + 'static;
}

pub enum Load<L: ResourceLoader> {
    Ready(L::Output),
    Pending,
    Start(LoadTicket<L>),
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
            }),
        }
    }
}

impl ResourceCache {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    pub fn load<L: ResourceLoader>(
        self: &Arc<Self>,
        owner: &Arc<[u64]>,
        source: &L::Source,
        source_bytes: usize,
        waiter: u64,
        wake: aqua::mailbox::Wake,
    ) -> Load<L> {
        let loader = TypeId::of::<L>();
        let mut state = self.state();
        if state.closed {
            return Load::Pending;
        }
        let position = state.entries.iter().position(|entry| {
            entry.loader == loader && entry.source.downcast_ref::<L::Source>() == Some(source)
        });
        if let Some(position) = position {
            let mut entry = state.entries.remove(position).unwrap();
            if !entry.owners.iter().any(|known| known == owner) {
                entry.owners.push(Arc::clone(owner));
            }
            if entry.output.is_none() && !entry.waiters.iter().any(|known| known.id == waiter) {
                entry.waiters.push(Waiter { id: waiter, wake });
            }
            let output = entry
                .output
                .as_ref()
                .and_then(|value| value.downcast_ref::<L::Output>())
                .cloned();
            let generation = entry.generation;
            state.entries.push_back(entry);
            let mut removed = Vec::new();
            while state
                .entries
                .iter()
                .map(Entry::resident_bytes)
                .sum::<usize>()
                > MAX_BYTES
            {
                let Some(entry) = state.entries.pop_front() else {
                    break;
                };
                removed.push(entry);
            }
            let retained = state
                .entries
                .iter()
                .any(|entry| entry.generation == generation);
            drop(state);
            for entry in removed {
                entry.dispose();
            }
            return if retained {
                output.map_or(Load::Pending, Load::Ready)
            } else {
                Load::Pending
            };
        }
        if state
            .entries
            .iter()
            .filter(|entry| entry.output.is_none())
            .count()
            >= MAX_PENDING
        {
            return Load::Pending;
        }
        let membership = size_of::<Arc<[u64]>>()
            .saturating_add(owner.len().saturating_mul(size_of::<u64>()))
            .saturating_add(2 * size_of::<usize>())
            .saturating_add(size_of::<Waiter>());
        if source_bytes.saturating_add(membership) > MAX_BYTES {
            return Load::Pending;
        }
        let mut removed: Vec<Entry> = Vec::new();
        while state.entries.len() >= MAX_ENTRIES
            || state
                .entries
                .iter()
                .map(Entry::resident_bytes)
                .sum::<usize>()
                .saturating_add(source_bytes)
                .saturating_add(membership)
                > MAX_BYTES
        {
            let Some(position) = state
                .entries
                .iter()
                .position(|entry| entry.output.is_some())
            else {
                drop(state);
                for entry in removed {
                    entry.dispose();
                }
                return Load::Pending;
            };
            removed.push(state.entries.remove(position).unwrap());
        }
        state.generation = state.generation.wrapping_add(1);
        let generation = state.generation;
        state.entries.push_back(Entry {
            loader,
            owners: vec![Arc::clone(owner)],
            waiters: vec![Waiter { id: waiter, wake }],
            source: Box::new(source.clone()),
            output: None,
            bytes: source_bytes,
            generation,
            cancel: None,
            release: None,
        });
        drop(state);
        for entry in removed {
            entry.dispose();
        }
        Load::Start(LoadTicket {
            cache: Arc::downgrade(self),
            source: source.clone(),
            generation,
            source_bytes,
            marker: PhantomData,
        })
    }

    pub fn forget<L: ResourceLoader>(&self, source: &L::Source) {
        let mut state = self.state();
        let position = state.entries.iter().position(|entry| {
            entry.loader == TypeId::of::<L>()
                && entry.source.downcast_ref::<L::Source>() == Some(source)
        });
        let removed = position.and_then(|index| state.entries.remove(index));
        drop(state);
        if let Some(entry) = removed {
            entry.dispose();
        }
    }

    pub fn retain_owners(&self, owners: &[Vec<u64>]) {
        let mut state = self.state();
        let mut kept = VecDeque::new();
        let mut removed = Vec::new();
        for mut entry in state.entries.drain(..) {
            entry
                .owners
                .retain(|owner| owners.iter().any(|prefix| owner.starts_with(prefix)));
            if entry.owners.is_empty() {
                removed.push(entry);
            } else {
                kept.push_back(entry);
            }
        }
        state.entries = kept;
        drop(state);
        for entry in removed {
            entry.dispose();
        }
    }

    pub fn clear(&self) {
        let mut state = self.state();
        state.closed = true;
        let entries = std::mem::take(&mut state.entries);
        drop(state);
        for entry in entries {
            entry.dispose();
        }
    }
}

impl<L: ResourceLoader> LoadTicket<L> {
    pub fn current(&self) -> bool {
        let Some(cache) = self.cache.upgrade() else {
            return false;
        };
        let state = cache.state();
        state.entries.iter().any(|entry| {
            entry.generation == self.generation
                && entry.loader == TypeId::of::<L>()
                && entry.source.downcast_ref::<L::Source>() == Some(&self.source)
                && entry.output.is_none()
        })
    }

    pub fn cancel(&self) -> bool {
        let Some(cache) = self.cache.upgrade() else {
            return false;
        };
        let mut state = cache.state();
        let position = state.entries.iter().position(|entry| {
            entry.generation == self.generation
                && entry.loader == TypeId::of::<L>()
                && entry.source.downcast_ref::<L::Source>() == Some(&self.source)
                && entry.output.is_none()
        });
        let removed = position.and_then(|index| state.entries.remove(index));
        drop(state);
        if let Some(entry) = removed {
            entry.dispose();
            true
        } else {
            false
        }
    }

    pub fn attach(&self, abort: AbortHandle) -> bool {
        let Some(cache) = self.cache.upgrade() else {
            abort.abort();
            return false;
        };
        let mut state = cache.state();
        let Some(entry) = state
            .entries
            .iter_mut()
            .find(|entry| entry.generation == self.generation && entry.output.is_none())
        else {
            drop(state);
            abort.abort();
            return false;
        };
        if entry.cancel.is_some() {
            drop(state);
            abort.abort();
            return false;
        }
        entry.cancel = Some(abort);
        true
    }

    pub fn publish(
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
        let Some(entry) = state.entries.iter_mut().find(|entry| {
            entry.generation == self.generation
                && entry.loader == TypeId::of::<L>()
                && entry.source.downcast_ref::<L::Source>() == Some(&self.source)
                && entry.output.is_none()
        }) else {
            drop(state);
            release(output);
            return false;
        };
        let owned = output.clone();
        entry.output = Some(Box::new(output));
        entry.cancel = None;
        entry.bytes = self.source_bytes.saturating_add(output_bytes);
        entry.release = Some(Box::new(move || release(owned)));
        let waiters = std::mem::take(&mut entry.waiters);
        let mut removed = Vec::new();
        while state.entries.len() > MAX_ENTRIES
            || state
                .entries
                .iter()
                .map(Entry::resident_bytes)
                .sum::<usize>()
                > MAX_BYTES
        {
            let Some(entry) = state.entries.pop_front() else {
                break;
            };
            removed.push(entry);
        }
        drop(state);
        for entry in removed {
            entry.dispose();
        }
        for waiter in waiters {
            (waiter.wake)();
        }
        true
    }
}
