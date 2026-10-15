//! Exact indexed literal bytes. Admission precedes every source request; live
//! leaf references keep their byte charge after the renderer evicts its cache.
use std::cell::RefCell;
use std::collections::VecDeque;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use artisan_api::{ContentRef, SourceIndexFormat};
use gpui::{App, SharedString, Task};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::stored::{StoredSource, fetch_complete_range};

const MAX_ENTRIES: usize = 128;
const MAX_RAW_BYTES: usize = 8 * 1024 * 1024;
const MAX_LEAF_BYTES: usize = 64 * 1024;
const MAX_FETCHES: usize = 4;
// fetch_complete_range retains its output while a bounded transport page is
// alive. String-to-SharedString conversion can temporarily retain both copies.
const FETCH_HEADROOM: usize = 65_539;

#[derive(Clone, Debug)]
pub enum LiteralPreparation {
    Pending,
    Ready(Arc<LiteralLeaf>),
    Failed(String),
}
#[derive(Debug)]
pub struct LiteralLeaf {
    pub text: SharedString,
    pub source_ref: ContentRef,
    pub ordinal: usize,
    pub source_range: Range<usize>,
    _capacity: OwnedSemaphorePermit,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Identity {
    reference: ContentRef,
    ordinal: usize,
    range: Range<usize>,
}
impl Identity {
    fn from_source(source: &StoredSource, ordinal: usize) -> Result<Option<Self>, String> {
        if source.format() != SourceIndexFormat::Literal {
            return Err("Raw literal leaves require a literal source index".into());
        }
        let Some(descriptor) = source.leaf_descriptor(ordinal) else {
            return Ok(None);
        };
        let start = usize::try_from(descriptor.source_start)
            .map_err(|_| "Literal source offset exceeds this platform")?;
        let end = usize::try_from(descriptor.source_end)
            .map_err(|_| "Literal source offset exceeds this platform")?;
        if descriptor.ordinal != ordinal as u64
            || start > end
            || descriptor.source_end > source.source_ref().bytes
        {
            return Err("Literal descriptor does not match its immutable source".into());
        }
        if end - start > MAX_LEAF_BYTES {
            return Err(
                "Literal physical line exceeds the 64 KiB raw leaf limit; open the full source"
                    .into(),
            );
        }
        Ok(Some(Self {
            reference: source.source_ref().clone(),
            ordinal,
            range: start..end,
        }))
    }
    fn admission_bytes(&self) -> usize {
        self.range.len() * 2 + FETCH_HEADROOM
    }
}
struct Entry {
    key: SharedString,
    identity: Identity,
    generation: u64,
    ready: Option<Arc<LiteralLeaf>>,
    error: Option<String>,
    task: Option<Task<()>>,
}
struct Waiter {
    key: SharedString,
    notify: Box<dyn Fn(&mut App)>,
}
#[derive(Default)]
struct Cache {
    entries: VecDeque<Entry>,
    waiters: Vec<Waiter>,
    generation: u64,
}
impl Cache {
    fn wait(&mut self, key: SharedString, notify: impl Fn(&mut App) + 'static) {
        if let Some(position) = self.waiters.iter().position(|waiter| waiter.key == key) {
            self.waiters.remove(position);
        }
        while self.waiters.len() >= MAX_ENTRIES {
            self.waiters.remove(0);
        }
        self.waiters.push(Waiter {
            key,
            notify: Box::new(notify),
        });
    }
    fn evict_ready(&mut self) -> bool {
        let Some(position) = self.entries.iter().position(|entry| entry.task.is_none()) else {
            return false;
        };
        self.entries.remove(position);
        true
    }
}
pub(super) struct LiteralOwner {
    cache: Rc<RefCell<Cache>>,
    bytes: Arc<Semaphore>,
    fetches: Arc<Semaphore>,
}
impl Default for LiteralOwner {
    fn default() -> Self {
        Self {
            cache: Rc::new(RefCell::new(Cache::default())),
            bytes: Arc::new(Semaphore::new(MAX_RAW_BYTES)),
            fetches: Arc::new(Semaphore::new(MAX_FETCHES)),
        }
    }
}
impl std::fmt::Debug for LiteralOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiteralOwner")
            .field("entries", &self.cache.borrow().entries.len())
            .field(
                "charged_bytes",
                &(MAX_RAW_BYTES - self.bytes.available_permits()),
            )
            .finish()
    }
}
impl LiteralOwner {
    pub(super) fn clear(&self) {
        let mut cache = self.cache.borrow_mut();
        cache.entries.clear();
        cache.waiters.clear();
        cache.generation = cache.generation.wrapping_add(1);
    }
    pub fn is_prepared(&self, key: &SharedString, source: &StoredSource, ordinal: usize) -> bool {
        let Ok(Some(identity)) = Identity::from_source(source, ordinal) else {
            return false;
        };
        self.cache
            .borrow()
            .entries
            .iter()
            .any(|entry| &entry.key == key && entry.identity == identity && entry.ready.is_some())
    }
    pub fn prepare(
        &self,
        key: SharedString,
        source: &StoredSource,
        ordinal: usize,
        cx: &mut App,
        notify: impl Fn(&mut App) + 'static,
    ) -> LiteralPreparation {
        let identity = match Identity::from_source(source, ordinal) {
            Ok(Some(identity)) => identity,
            Ok(None) => return LiteralPreparation::Pending,
            Err(error) => return LiteralPreparation::Failed(error),
        };
        let mut cache = self.cache.borrow_mut();
        if let Some(position) = cache.entries.iter().position(|entry| entry.key == key) {
            if cache.entries[position].identity == identity {
                let entry = cache.entries.remove(position).expect("existing entry");
                let result = if let Some(ready) = &entry.ready {
                    LiteralPreparation::Ready(ready.clone())
                } else if let Some(error) = &entry.error {
                    LiteralPreparation::Failed(error.clone())
                } else {
                    cache.wait(key, notify);
                    LiteralPreparation::Pending
                };
                cache.entries.push_back(entry);
                return result;
            }
            cache.entries.remove(position);
        }
        // No source-bearing task waits for admission. At most four fetches
        // own input references and permits; bounded weak callbacks retry later.
        let Ok(fetch_capacity) = self.fetches.clone().try_acquire_owned() else {
            cache.wait(key, notify);
            return LiteralPreparation::Pending;
        };
        let requested = identity.admission_bytes() as u32;
        let mut capacity = loop {
            if let Ok(capacity) = self.bytes.clone().try_acquire_many_owned(requested) {
                break capacity;
            }
            if !cache.evict_ready() {
                if cache.entries.iter().any(|entry| entry.task.is_some()) {
                    cache.wait(key, notify);
                    return LiteralPreparation::Pending;
                }
                return LiteralPreparation::Failed(
                    "Live literal leaves exhaust the 8 MiB raw byte budget".into(),
                );
            }
        };
        while cache.entries.len() >= MAX_ENTRIES {
            if !cache.evict_ready() {
                cache.wait(key, notify);
                return LiteralPreparation::Pending;
            }
        }
        cache.generation = cache.generation.wrapping_add(1);
        let generation = cache.generation;
        let fetcher = source.fetcher.clone();
        let job_identity = identity.clone();
        let job_key = key.clone();
        let weak = Rc::downgrade(&self.cache);
        let task = cx.spawn(async move |cx| {
            let result = fetch_complete_range(
                &job_identity.reference,
                job_identity.range.clone(),
                &fetcher,
            )
            .await
            .map(|text| {
                let text: SharedString = text.into();
                // Fetch/conversion headroom is transient. Keep only the exact
                // shared raw allocation charged for the lifetime of this leaf.
                let release = capacity.num_permits() - text.len().max(1);
                drop(capacity.split(release));
                Arc::new(LiteralLeaf {
                    text,
                    source_ref: job_identity.reference.clone(),
                    ordinal: job_identity.ordinal,
                    source_range: job_identity.range.clone(),
                    _capacity: capacity,
                })
            });
            drop(fetch_capacity);
            let _ = cx.update(|cx| {
                let Some(owner) = weak.upgrade() else {
                    return;
                };
                let mut cache = owner.borrow_mut();
                let Some(entry) = cache.entries.iter_mut().find(|entry| {
                    entry.key == job_key
                        && entry.identity == job_identity
                        && entry.generation == generation
                }) else {
                    return;
                };
                entry.task = None;
                match result {
                    Ok(leaf) => entry.ready = Some(leaf),
                    Err(error) => entry.error = Some(error),
                }
                let waiters = std::mem::take(&mut cache.waiters);
                drop(cache);
                notify(cx);
                for waiter in waiters {
                    (waiter.notify)(cx);
                }
            });
        });
        cache.entries.push_back(Entry {
            key,
            identity,
            generation,
            ready: None,
            error: None,
            task: Some(task),
        });
        LiteralPreparation::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn literal_fetch_preserves_syntax_and_only_requests_its_source_range() {
        let full = Arc::new("prefix\n**literal** &amp; 界🙂\nnext\n".to_owned());
        let reference = ContentRef::from_text(&full);
        let start = "prefix\n".len();
        let end = full.find("next").unwrap();
        let requested = Arc::new(std::sync::Mutex::new(Vec::new()));
        let calls = requested.clone();
        let original = full.clone();
        let fetcher: super::super::stored::ContentFetch =
            Arc::new(move |reference, offset, limit| {
                calls.lock().unwrap().push((offset, limit));
                let full = original.clone();
                Box::pin(async move {
                    let end = (offset as usize + limit as usize).min(full.len());
                    Ok(artisan_api::ContentRange {
                        id: reference.id,
                        total: reference.bytes,
                        start: offset,
                        end: end as u64,
                        text: full[offset as usize..end].to_owned(),
                    })
                })
            });
        let text =
            futures::executor::block_on(fetch_complete_range(&reference, start..end, &fetcher))
                .unwrap();
        assert_eq!(text, &full[start..end]);
        assert_eq!(
            *requested.lock().unwrap(),
            vec![(start as u64, (end - start) as u32)]
        );
    }
    #[test]
    fn live_leaf_references_keep_byte_admission_after_cache_eviction() {
        let semaphore = Arc::new(Semaphore::new(20));
        let permit = semaphore.clone().try_acquire_many_owned(12).unwrap();
        let leaf = Arc::new(LiteralLeaf {
            text: "literal".into(),
            source_ref: ContentRef::from_text("literal"),
            ordinal: 0,
            source_range: 0..7,
            _capacity: permit,
        });
        let mounted = leaf.clone();
        drop(leaf);
        assert_eq!(semaphore.available_permits(), 8);
        drop(mounted);
        assert_eq!(semaphore.available_permits(), 20);
    }
}
