use super::stored::{RangeCollector, StoredOwner, StoredSource};
use aqua_text::document::{SelectionSource, SelectionSourceId};
use artisan_api::{ContentRange, SourceIndexFormat, SourceLeafDescriptor};
use editor_core::{delivery::Delivery, runtime};
use std::{
    collections::{HashMap, VecDeque},
    num::NonZeroUsize,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};
use tokio::{
    sync::{OwnedSemaphorePermit, Semaphore},
    task::AbortHandle,
};

const MAX_ENTRIES: usize = 128;
const MAX_BYTES: usize = 8 * 1024 * 1024;
const MAX_FETCHES: usize = 4;
const MAX_LEAF_BYTES: usize = 64 * 1024;
const FETCH_HEADROOM: usize = 65_539;
const MAX_NATIVE_SOURCE: usize = 16 * 1024;

#[derive(Debug, Clone)]
pub enum LiteralPreparation {
    Pending,
    Ready(Arc<LiteralLeaf>),
    Failed(String),
}

struct Lease {
    permit: Option<OwnedSemaphorePermit>,
    wake: aqua::mailbox::Wake,
}
impl Drop for Lease {
    fn drop(&mut self) {
        drop(self.permit.take());
        (self.wake)();
    }
}

pub struct LiteralLeaf {
    text: Arc<str>,
    _lease: Lease,
}

impl std::fmt::Debug for LiteralLeaf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiteralLeaf").finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub struct LiteralSelection {
    source: SelectionSource,
    _leaf: Arc<LiteralLeaf>,
}
impl LiteralSelection {
    pub fn source(&self) -> &SelectionSource {
        &self.source
    }
}

fn selection_source(
    text: Arc<str>,
    id: SelectionSourceId,
    order: Vec<u64>,
    separator: &str,
) -> SelectionSource {
    SelectionSource {
        id,
        order,
        text,
        separator: Arc::from(separator),
        copies: Vec::new(),
    }
}
fn visible_text(text: &str) -> &str {
    text.strip_suffix('\n').unwrap_or(text)
}

impl LiteralLeaf {
    pub fn visible_text(&self) -> &str {
        visible_text(&self.text)
    }
    pub fn needs_native_window(&self) -> bool {
        self.visible_text().len() > MAX_NATIVE_SOURCE
    }
    pub fn selection(
        self: &Arc<Self>,
        id: SelectionSourceId,
        order: Vec<u64>,
        separator: &str,
    ) -> LiteralSelection {
        LiteralSelection {
            source: selection_source(Arc::clone(&self.text), id, order, separator),
            _leaf: Arc::clone(self),
        }
    }
}

enum Phase {
    Loading {
        collector: Option<RangeCollector>,
        capacity: Option<OwnedSemaphorePermit>,
    },
    Ready(Arc<LiteralLeaf>),
    Failed(String),
}
struct Entry {
    key: String,
    source: StoredSource,
    descriptor: SourceLeafDescriptor,
    generation: u64,
    valid: Arc<AtomicBool>,
    phase: Phase,
    task: Option<AbortHandle>,
}
impl Drop for Entry {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
struct Completion {
    key: String,
    generation: u64,
    result: Result<ContentRange, editor_core::client::RangeError>,
}

#[derive(Default)]
struct Epochs {
    entries: HashMap<String, Weak<AtomicBool>>,
}
impl Epochs {
    fn issue(&mut self, key: &str) -> Arc<AtomicBool> {
        self.prune();
        self.release(key);
        let valid = Arc::new(AtomicBool::new(true));
        self.entries.insert(key.to_owned(), Arc::downgrade(&valid));
        valid
    }
    fn release(&mut self, key: &str) {
        if let Some(valid) = self.entries.remove(key).and_then(|epoch| epoch.upgrade()) {
            valid.store(false, Ordering::Relaxed);
        }
    }
    fn clear(&mut self) {
        for epoch in self
            .entries
            .drain()
            .filter_map(|(_, epoch)| epoch.upgrade())
        {
            epoch.store(false, Ordering::Relaxed);
        }
    }
    fn prune(&mut self) {
        self.entries.retain(|_, epoch| epoch.strong_count() != 0);
    }
}

pub struct LiteralOwner {
    entries: VecDeque<Entry>,
    epochs: Epochs,
    generation: u64,
    epoch: Arc<AtomicU64>,
    capacity: Arc<Semaphore>,
    delivery: Delivery<Completion>,
}

impl std::fmt::Debug for LiteralOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiteralOwner").finish_non_exhaustive()
    }
}
impl Default for LiteralOwner {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
            epochs: Epochs::default(),
            generation: 0,
            epoch: Arc::new(AtomicU64::new(0)),
            capacity: Arc::new(Semaphore::new(MAX_BYTES)),
            delivery: Delivery::new(
                const { NonZeroUsize::new(MAX_FETCHES).expect("MAX_FETCHES is not zero") },
            ),
        }
    }
}
impl LiteralOwner {
    pub fn wake_mailbox(&self) -> Option<&aqua::mailbox::Mailbox<()>> {
        self.delivery.wake_mailbox()
    }
    pub fn clear(&mut self) {
        self.epochs.clear();
        self.epoch.fetch_add(1, Ordering::Relaxed);
        for entry in &self.entries {
            entry.valid.store(false, Ordering::Relaxed);
        }
        self.entries.clear();
        self.generation = self.generation.wrapping_add(1);
    }
    pub fn prepare(
        &mut self,
        owner: &StoredOwner,
        key: &str,
        source: &StoredSource,
        ordinal: usize,
    ) -> LiteralPreparation {
        self.entries.retain(|entry| {
            let current = owner.source_is_current(&entry.source);
            if !current {
                entry.valid.store(false, Ordering::Relaxed);
            }
            current
        });
        if source.format() != SourceIndexFormat::Literal {
            return LiteralPreparation::Failed(
                "Raw literal leaves require a literal source index".to_owned(),
            );
        }
        if !owner.source_is_current(source) {
            return LiteralPreparation::Pending;
        }
        let Some(descriptor) = owner.descriptor(source, ordinal).cloned() else {
            return LiteralPreparation::Pending;
        };
        let (Ok(start), Ok(end)) = (
            usize::try_from(descriptor.source_start),
            usize::try_from(descriptor.source_end),
        ) else {
            return LiteralPreparation::Failed(
                "Literal source offset exceeds this platform".to_owned(),
            );
        };
        if descriptor.ordinal != ordinal as u64
            || start > end
            || descriptor.source_end > source.source_ref().bytes
        {
            return LiteralPreparation::Failed(
                "Literal descriptor does not match its immutable source".to_owned(),
            );
        }
        if end - start > MAX_LEAF_BYTES {
            return LiteralPreparation::Failed(
                "Literal physical line exceeds the 64 KiB raw leaf limit; open the full source"
                    .to_owned(),
            );
        }
        let found = self.entries.iter().position(|entry| entry.key == key);
        if let Some(index) = found {
            let same = self.entries[index].source.revision() == source.revision()
                && self.entries[index].source.source_ref() == source.source_ref()
                && self.entries[index].descriptor == descriptor;
            if same {
                let entry = self
                    .entries
                    .remove(index)
                    .unwrap_or_else(|| panic!("invariant: literal {key} was found at {index}"));
                let result = match &entry.phase {
                    Phase::Ready(leaf) => LiteralPreparation::Ready(Arc::clone(leaf)),
                    Phase::Failed(error) => LiteralPreparation::Failed(error.clone()),
                    Phase::Loading { .. } => LiteralPreparation::Pending,
                };
                self.entries.push_back(entry);
                return result;
            }
            self.entries[index].valid.store(false, Ordering::Relaxed);
            self.entries.remove(index);
        }
        if self
            .entries
            .iter()
            .filter(|entry| matches!(entry.phase, Phase::Loading { .. }))
            .count()
            >= MAX_FETCHES
        {
            return LiteralPreparation::Pending;
        }
        while self.entries.len() >= MAX_ENTRIES {
            if !self.evict_finished() {
                return LiteralPreparation::Pending;
            }
        }
        let requested = (end - start) * 2 + FETCH_HEADROOM;
        let capacity = loop {
            if let Ok(permit) = Arc::clone(&self.capacity).try_acquire_many_owned(
                u32::try_from(requested).unwrap_or_else(|_| panic!("invariant: literal {key} requests {requested} bytes, bounded by MAX_LEAF_BYTES")),
            )
            {
                break permit;
            }
            if !self.evict_finished() {
                return if self
                    .entries
                    .iter()
                    .any(|entry| matches!(entry.phase, Phase::Loading { .. }))
                {
                    LiteralPreparation::Pending
                } else {
                    LiteralPreparation::Failed(
                        "Live literal leaves exhaust the 8 MiB raw byte budget".to_owned(),
                    )
                };
            }
        };
        let collector = match RangeCollector::new(source.source_ref().clone(), start..end) {
            Ok(collector) => collector,
            Err(error) => return LiteralPreparation::Failed(error),
        };
        self.generation = self.generation.wrapping_add(1);
        let valid = self.epochs.issue(key);
        self.entries.push_back(Entry {
            key: key.to_owned(),
            source: source.clone(),
            descriptor,
            generation: self.generation,
            valid,
            phase: Phase::Loading {
                collector: Some(collector),
                capacity: Some(capacity),
            },
            task: None,
        });
        self.advance(owner, key);
        self.result(key)
    }
    fn evict_finished(&mut self) -> bool {
        let Some(index) = self
            .entries
            .iter()
            .position(|entry| !matches!(entry.phase, Phase::Loading { .. }))
        else {
            return false;
        };
        self.entries.remove(index);
        true
    }
    fn result(&self, key: &str) -> LiteralPreparation {
        let phase = self
            .entries
            .iter()
            .find(|entry| entry.key == key)
            .map(|entry| &entry.phase);
        match phase {
            Some(Phase::Ready(leaf)) => LiteralPreparation::Ready(Arc::clone(leaf)),
            Some(Phase::Failed(error)) => LiteralPreparation::Failed(error.clone()),
            _ => LiteralPreparation::Pending,
        }
    }
    fn advance(&mut self, owner: &StoredOwner, key: &str) {
        let Some(entry) = self.entries.iter_mut().find(|entry| entry.key == key) else {
            return;
        };
        if !owner.source_is_current(&entry.source) {
            return;
        }
        let Phase::Loading {
            collector,
            capacity,
        } = &mut entry.phase
        else {
            return;
        };
        let Some(collector_ref) = collector.as_ref() else {
            return;
        };
        if let Some((offset, limit)) = collector_ref.next_request() {
            let Some(future) = entry.source.literal_content_range(offset, limit) else {
                entry.phase = Phase::Failed("Host disconnected".to_owned());
                return;
            };
            let sender = self.delivery.sender();
            let key = entry.key.clone();
            let generation = entry.generation;
            entry.task = Some(
                runtime::runtime()
                    .spawn(async move {
                        let result = future.await;
                        sender
                            .offer(Completion {
                                key,
                                generation,
                                result,
                            })
                            .await;
                    })
                    .abort_handle(),
            );
        } else {
            let text = match collector
                .take()
                .unwrap_or_else(|| panic!("invariant: literal {key} is loading with its collector"))
                .finish()
            {
                Ok(text) => text,
                Err(error) => {
                    entry.phase = Phase::Failed(error);
                    return;
                }
            };
            let mut permit = capacity
                .take()
                .unwrap_or_else(|| panic!("invariant: literal {key} is loading with its capacity"));
            let retained = text.len().max(1);
            let release = permit.num_permits() - retained;
            drop(permit.split(release));
            let leaf = LiteralLeaf {
                text: Arc::from(text),
                _lease: Lease {
                    permit: Some(permit),
                    wake: self.delivery.waker(),
                },
            };
            entry.phase = Phase::Ready(Arc::new(leaf));
            self.delivery.wake();
        }
    }
    pub fn drain(&mut self, owner: &StoredOwner) -> bool {
        self.epochs.prune();
        self.entries.retain(|entry| {
            let current = owner.source_is_current(&entry.source);
            if !current {
                entry.valid.store(false, Ordering::Relaxed);
            }
            current
        });
        let mut changed = false;
        for completion in self.delivery.drain() {
            let Some(entry) = self.entries.iter_mut().find(|entry| {
                entry.key == completion.key && entry.generation == completion.generation
            }) else {
                continue;
            };
            if !owner.source_is_current(&entry.source) {
                continue;
            }
            entry.task = None;
            let Phase::Loading {
                collector: Some(collector),
                ..
            } = &mut entry.phase
            else {
                continue;
            };
            let result = completion
                .result
                .map_err(|error| error.to_string())
                .and_then(|page| collector.push(page));
            if let Err(error) = result {
                entry.phase = Phase::Failed(error);
            } else {
                self.advance(owner, &completion.key);
            }
            changed = true;
        }
        changed
    }
}

#[cfg(test)]
#[path = "../../../tests/markdown/literal_owner.rs"]
mod tests;
