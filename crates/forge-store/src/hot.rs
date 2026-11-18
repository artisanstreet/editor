use artisan_api::{Envelope, Event, ItemId, ItemsPage, ItemsQuery, ThreadId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

const BUDGET_BYTES: usize = 32 * 1024 * 1024;
const LARGEST_PAGE_BYTES: usize = BUDGET_BYTES / 8;
const THREADS_REMEMBERED: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tail {
    Rows {
        limit: Option<u32>,
    },
    Items {
        parent: Option<ItemId>,
        limit: Option<u32>,
    },
}

impl Tail {
    pub fn rows(query: &ItemsQuery) -> Option<Self> {
        is_tail(query).then_some(Self::Rows { limit: query.limit })
    }

    pub fn items(parent: Option<&ItemId>, query: &ItemsQuery) -> Option<Self> {
        is_tail(query).then(|| Self::Items {
            parent: parent.cloned(),
            limit: query.limit,
        })
    }
}

const fn is_tail(query: &ItemsQuery) -> bool {
    query.before.is_none() && query.after.is_none()
}

pub enum Lookup {
    Hit(Arc<ItemsPage>),
    Miss(Version),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Version {
    epoch: u64,
    revision: u64,
}

struct Held {
    tail: Tail,
    page: Arc<ItemsPage>,
    bytes: usize,
    used: u64,
}

#[derive(Default)]
struct Thread {
    changed_at: u64,
    held: Vec<Held>,
}

#[derive(Default)]
struct Window {
    threads: HashMap<ThreadId, Thread>,
    bytes: usize,
    clock: u64,
    epoch: u64,
    revision: u64,
    forgotten: u64,
}

#[derive(Default)]
pub struct HotPages {
    window: Mutex<Window>,
}

impl HotPages {
    pub fn lookup(&self, thread: &ThreadId, tail: &Tail) -> Lookup {
        self.window
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .lookup(thread, tail)
    }

    pub fn hold(&self, thread: &ThreadId, tail: Tail, read_at: Version, page: &ItemsPage) {
        let bytes = page_bytes(page);
        if bytes > LARGEST_PAGE_BYTES {
            return;
        }
        let held = Held {
            tail,
            page: Arc::new(page.clone()),
            bytes,
            used: 0,
        };
        self.window
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .hold(thread, read_at, held);
    }

    pub fn committed(&self, envelopes: &[Envelope]) {
        self.window
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .committed(envelopes);
    }

    pub fn reset(&self) {
        self.window
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .invalidate_everything();
    }

    pub fn threads_remembered(&self) -> usize {
        self.window
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .threads
            .len()
    }

    pub fn held_bytes(&self) -> usize {
        self.window
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .bytes
    }
}

impl Window {
    fn lookup(&mut self, thread: &ThreadId, tail: &Tail) -> Lookup {
        self.clock += 1;
        let read_at = Version {
            epoch: self.epoch,
            revision: self.revision,
        };
        let Some(known) = self.threads.get_mut(thread) else {
            return Lookup::Miss(read_at);
        };
        match known.held.iter_mut().find(|held| held.tail == *tail) {
            Some(held) => {
                held.used = self.clock;
                Lookup::Hit(Arc::clone(&held.page))
            }
            None => Lookup::Miss(read_at),
        }
    }

    fn hold(&mut self, thread: &ThreadId, read_at: Version, mut held: Held) {
        self.clock += 1;
        let changed_at = self
            .threads
            .get(thread)
            .map_or(self.forgotten, |known| known.changed_at);
        if read_at.epoch != self.epoch || changed_at > read_at.revision {
            return;
        }
        let known = self.threads.entry(thread.clone()).or_insert(Thread {
            changed_at,
            held: Vec::new(),
        });
        if known.held.iter().any(|kept| kept.tail == held.tail) {
            return;
        }
        held.used = self.clock;
        self.bytes += held.bytes;
        known.held.push(held);
        self.shed();
        self.forget();
    }

    fn committed(&mut self, envelopes: &[Envelope]) {
        for envelope in envelopes {
            if matches!(envelope.event, Event::Reset) {
                self.invalidate_everything();
            } else if let Some(thread) = changed_transcript(&envelope.event) {
                self.invalidate(thread);
            }
        }
    }

    fn invalidate(&mut self, thread: &ThreadId) {
        self.revision += 1;
        let known = self.threads.entry(thread.clone()).or_default();
        known.changed_at = self.revision;
        let released: usize = known.held.drain(..).map(|held| held.bytes).sum();
        self.bytes -= released;
        self.forget();
    }

    fn invalidate_everything(&mut self) {
        self.epoch += 1;
        self.threads.clear();
        self.bytes = 0;
        self.forgotten = self.revision;
    }

    fn forget(&mut self) {
        if self.threads.len() <= THREADS_REMEMBERED {
            return;
        }
        let mut oldest: Vec<((bool, u64), ThreadId)> = self
            .threads
            .iter()
            .map(|(thread, known)| {
                let recency = known
                    .held
                    .iter()
                    .map(|held| held.used)
                    .max()
                    .map_or((false, known.changed_at), |used| (true, used));
                (recency, thread.clone())
            })
            .collect();
        let excess = self.threads.len() - THREADS_REMEMBERED / 2;
        oldest.select_nth_unstable(excess);
        oldest.truncate(excess);
        for (_, thread) in oldest {
            if let Some(known) = self.threads.remove(&thread) {
                self.forgotten = self.forgotten.max(known.changed_at);
                self.bytes -= known.held.iter().map(|held| held.bytes).sum::<usize>();
            }
        }
    }

    fn shed(&mut self) {
        while self.bytes > BUDGET_BYTES {
            let Some((thread, used)) = self
                .threads
                .iter()
                .flat_map(|(thread, known)| known.held.iter().map(move |held| (thread, held.used)))
                .min_by_key(|(_, used)| *used)
                .map(|(thread, used)| (thread.clone(), used))
            else {
                return;
            };
            if let Some(known) = self.threads.get_mut(&thread) {
                known.held.retain(|held| {
                    let keep = held.used != used;
                    if !keep {
                        self.bytes -= held.bytes;
                    }
                    keep
                });
            }
        }
    }
}

const fn changed_transcript(event: &Event) -> Option<&ThreadId> {
    match event {
        Event::Item { item } => Some(&item.thread_id),
        Event::Run { run } => Some(&run.thread_id),
        Event::ItemState { thread_id, .. } | Event::ThreadRemoved { thread_id } => Some(thread_id),
        Event::Hello { .. }
        | Event::Project { .. }
        | Event::Thread { .. }
        | Event::Setting { .. }
        | Event::Live { .. }
        | Event::Suggestion { .. }
        | Event::Engine { .. }
        | Event::EngineInstall { .. }
        | Event::EditorCall { .. }
        | Event::Reset => None,
    }
}

fn page_bytes(page: &ItemsPage) -> usize {
    crate::encoded::len(page)
}
