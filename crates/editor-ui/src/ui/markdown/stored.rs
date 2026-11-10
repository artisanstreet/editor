use std::ops::Range;

use artisan_api::{ContentRange, ContentRef, SourceIndexFormat, SourceIndexRange};

pub const MAX_SOURCE_BYTES: usize = 64 * 1024 * 1024;
pub const RANGE_BYTES: u32 = 65_536;
pub const MAX_INDEX_ENTRIES: usize = 48;
pub const MAX_PREPARED_ENTRIES: usize = 128;
pub const MAX_PENDING_JOBS: usize = 48;
pub const PREPARED_BYTES: usize = 32 * 1024 * 1024;
pub const QUEUE_DEPTH: usize = 8;

pub fn validate_page(
    reference: &ContentRef,
    format: SourceIndexFormat,
    offset: Option<u64>,
    page: &SourceIndexRange,
) -> Result<(), String> {
    if reference.bytes > MAX_SOURCE_BYTES as u64 {
        return Err(
            "Markdown source exceeds the 64 MiB source admission limit; export the full source"
                .to_owned(),
        );
    }
    if page.index.source != *reference
        || page.index.format != format
        || page.index.version != artisan_api::SOURCE_INDEX_VERSION
        || !page.index.metadata.id.is_valid()
        || page.start > page.end
        || page.end > page.index.leaves
        || page.end - page.start != page.leaves.len() as u64
        || page.leaves.len() > artisan_api::SOURCE_INDEX_PAGE_LEAVES as usize
        || offset.is_some_and(|offset| page.start != offset.min(page.index.leaves))
        || offset.is_none() && page.end != page.index.leaves
        || usize::try_from(page.index.leaves).is_err()
    {
        return Err("Markdown descriptor page does not match its immutable source".to_owned());
    }
    for (position, leaf) in page.leaves.iter().enumerate() {
        if leaf.ordinal != page.start + position as u64
            || leaf.source_start > leaf.source_end
            || leaf.source_end > reference.bytes
            || leaf.metadata_start >= leaf.metadata_end
            || leaf.metadata_end > page.index.metadata.bytes
        {
            return Err("Markdown descriptor does not match its source or metadata".to_owned());
        }
    }
    Ok(())
}

pub struct RangeCollector {
    reference: ContentRef,
    range: Range<usize>,
    cursor: usize,
    text: String,
}

impl RangeCollector {
    pub fn new(reference: ContentRef, range: Range<usize>) -> Result<Self, String> {
        if range.start > range.end
            || u64::try_from(range.end).map_or(true, |end| end > reference.bytes)
            || range.len() > MAX_SOURCE_BYTES
        {
            return Err("Markdown source range exceeds its admission limit".to_owned());
        }
        let cursor = range.start;
        Ok(Self {
            reference,
            text: String::with_capacity(range.len()),
            range,
            cursor,
        })
    }

    pub fn next_request(&self) -> Option<(u64, u32)> {
        (self.cursor < self.range.end).then(|| {
            let remaining = self.range.end - self.cursor;
            (
                self.cursor as u64,
                RANGE_BYTES.min(u32::try_from(remaining.max(4)).unwrap_or(u32::MAX)),
            )
        })
    }

    pub fn push(&mut self, page: ContentRange) -> Result<(), String> {
        let cursor = self.cursor as u64;
        if page.id != self.reference.id
            || page.total != self.reference.bytes
            || page.start > cursor
            || page.end <= cursor
            || page.end > self.reference.bytes
            || page.end - page.start != page.text.len() as u64
            || page.text.len() > RANGE_BYTES as usize + 3
        {
            return Err("Markdown source response does not match its immutable range".to_owned());
        }
        let start = usize::try_from(cursor - page.start)
            .map_err(|_| "Markdown source offset exceeds this platform")?;
        let end = usize::try_from(page.end.min(self.range.end as u64) - page.start)
            .map_err(|_| "Markdown source offset exceeds this platform")?;
        let fragment = page
            .text
            .get(start..end)
            .ok_or("Markdown source response splits a UTF-8 boundary")?;
        self.text.push_str(fragment);
        self.cursor = usize::try_from(page.end.min(self.range.end as u64))
            .map_err(|_| "Markdown source offset exceeds this platform")?;
        Ok(())
    }

    pub fn finish(self) -> Result<String, String> {
        if self.cursor != self.range.end {
            return Err("Markdown source range is incomplete".to_owned());
        }
        Ok(self.text)
    }
}

use std::collections::VecDeque;
use std::num::NonZeroUsize;
use std::sync::Arc;

use artisan_api::SourceLeafDescriptor;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::task::AbortHandle;

use super::{IndexedLeafContent, MarkdownEngine, normalize_record};
use editor_core::{delivery::Delivery, host::ScopedSourceProvider, runtime};

#[derive(Clone)]
pub struct StoredSource {
    provider: ScopedSourceProvider,
    page: Arc<SourceIndexRange>,
    revision: u64,
    owner: String,
}

impl StoredSource {
    pub fn literal_content_range(
        &self,
        offset: u64,
        limit: u32,
    ) -> Option<futures::future::BoxFuture<'static, anyhow::Result<ContentRange>>> {
        if self.format() != SourceIndexFormat::Literal || limit == 0 || limit > RANGE_BYTES {
            return None;
        }
        self.provider
            .content_range(self.source_ref().clone(), offset, limit)
    }

    pub fn source_ref(&self) -> &ContentRef {
        &self.page.index.source
    }
    pub fn format(&self) -> SourceIndexFormat {
        self.page.index.format
    }
    pub fn block_count(&self) -> usize {
        usize::try_from(self.page.index.leaves).unwrap_or(usize::MAX)
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn loaded_range(&self) -> Range<usize> {
        usize::try_from(self.page.start).unwrap_or(usize::MAX)
            ..usize::try_from(self.page.end).unwrap_or(usize::MAX)
    }
    pub fn descriptor(&self, ordinal: usize) -> Option<&SourceLeafDescriptor> {
        self.page
            .leaves
            .get(ordinal.checked_sub(usize::try_from(self.page.start).ok()?)?)
    }
}

#[derive(Clone)]
pub enum StoredPreparation {
    Pending,
    Ready(StoredSource),
    Failed(String),
}

#[derive(Clone)]
pub enum LeafPreparation {
    Pending,
    Ready(Arc<StoredLeaf>),
    Failed(String),
}

struct PreparedLease {
    _permit: Option<OwnedSemaphorePermit>,
    wake: aqua::mailbox::Wake,
}

impl Drop for PreparedLease {
    fn drop(&mut self) {
        drop(self._permit.take());
        (self.wake)();
    }
}

pub struct StoredLeaf {
    pub descriptor: SourceLeafDescriptor,
    pub source: Arc<str>,
    pub context: artisan_markdown_index::LeafContext,
    pub table_continues: bool,
    pub content: IndexedLeafContent,
    proof: LeafProof,
    _capacity: PreparedLease,
}

#[derive(Clone)]
pub struct LeafProof {
    provider: ScopedSourceProvider,
    owner: String,
    reference: ContentRef,
    descriptor: SourceLeafDescriptor,
    generation: u64,
}

impl LeafProof {
    pub fn owner(&self) -> &str {
        &self.owner
    }
}

impl StoredLeaf {
    pub fn proof(&self) -> &LeafProof {
        &self.proof
    }
}

struct IndexEntry {
    owner: String,
    provider: ScopedSourceProvider,
    reference: ContentRef,
    format: SourceIndexFormat,
    revision: u64,
    pending_offset: Option<Option<u64>>,
    failed_offset: Option<Option<u64>>,
    current: Option<StoredSource>,
    error: Option<String>,
    task: Option<AbortHandle>,
}

impl Drop for IndexEntry {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

enum LeafPhase {
    Metadata(RangeCollector),
    Source {
        metadata: String,
        collector: RangeCollector,
    },
    WaitingWorker {
        metadata: String,
        source: String,
    },
    Working,
    Ready(Arc<StoredLeaf>),
    Failed(String),
}

struct LeafEntry {
    key: String,
    source: StoredSource,
    descriptor: SourceLeafDescriptor,
    generation: u64,
    capacity: Option<Arc<OwnedSemaphorePermit>>,
    phase: LeafPhase,
    task: Option<AbortHandle>,
}

impl Drop for LeafEntry {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

enum Completion {
    Index {
        owner: String,
        revision: u64,
        result: Result<SourceIndexRange, String>,
    },
    Range {
        key: String,
        generation: u64,
        result: Result<ContentRange, String>,
    },
    Prepared {
        key: String,
        generation: u64,
        _worker: Option<OwnedSemaphorePermit>,
        _capacity: Option<Arc<OwnedSemaphorePermit>>,
        result: Result<
            (
                artisan_markdown_index::LeafContext,
                bool,
                IndexedLeafContent,
                Arc<str>,
                usize,
            ),
            String,
        >,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoredUpdate {
    Index { owner: String, revision: u64 },
    Leaf { key: String, ordinal: usize },
}

type WorkerJob = Box<dyn FnOnce(&Result<MarkdownEngine, String>) + Send>;

fn preparation_worker() -> Option<tokio::sync::mpsc::Sender<WorkerJob>> {
    let (sender, mut receiver) = tokio::sync::mpsc::channel::<WorkerJob>(QUEUE_DEPTH);
    std::thread::Builder::new()
        .name("artisan-markdown-stored".to_owned())
        .spawn(move || {
            let engine = MarkdownEngine::new().map_err(|error| error.to_string());
            while let Some(job) = receiver.blocking_recv() {
                job(&engine);
            }
        })
        .ok()?;
    Some(sender)
}

pub struct StoredOwner {
    indices: VecDeque<IndexEntry>,
    pages: VecDeque<(ScopedSourceProvider, Arc<SourceIndexRange>)>,
    leaves: VecDeque<LeafEntry>,
    generation: u64,
    capacity: Arc<Semaphore>,
    prepared_capacity: Arc<Semaphore>,
    worker: Option<tokio::sync::mpsc::Sender<WorkerJob>>,
    delivery: Delivery<Completion>,
}

impl Default for StoredOwner {
    fn default() -> Self {
        Self {
            indices: VecDeque::new(),
            pages: VecDeque::new(),
            leaves: VecDeque::new(),
            generation: 0,
            capacity: Arc::new(Semaphore::new(PREPARED_BYTES)),
            prepared_capacity: Arc::new(Semaphore::new(PREPARED_BYTES)),
            worker: preparation_worker(),
            delivery: Delivery::new(NonZeroUsize::new(MAX_PENDING_JOBS).unwrap()),
        }
    }
}

impl StoredOwner {
    pub fn wake_mailbox(&self) -> Option<&aqua::mailbox::Mailbox<()>> {
        self.delivery.wake_mailbox()
    }
    fn next_generation(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.generation
    }
    fn same_provider(a: &ScopedSourceProvider, b: &ScopedSourceProvider) -> bool {
        a.is_available() && b.is_available() && a.host() == b.host() && a.instance() == b.instance()
    }
    fn remember_page(&mut self, provider: ScopedSourceProvider, page: Arc<SourceIndexRange>) {
        self.pages.retain(|(existing_provider, existing)| {
            !(Self::same_provider(existing_provider, &provider)
                && existing.index.source == page.index.source
                && existing.index.format == page.index.format
                && existing.start == page.start)
        });
        while self
            .pages
            .iter()
            .filter(|(existing_provider, existing)| {
                Self::same_provider(existing_provider, &provider)
                    && existing.index.source == page.index.source
                    && existing.index.format == page.index.format
            })
            .count()
            >= 2
        {
            let Some(index) = self.pages.iter().position(|(existing_provider, existing)| {
                Self::same_provider(existing_provider, &provider)
                    && existing.index.source == page.index.source
                    && existing.index.format == page.index.format
            }) else {
                break;
            };
            self.pages.remove(index);
        }
        self.pages.push_back((provider, page));
        while self.pages.len() > MAX_INDEX_ENTRIES {
            self.pages.pop_front();
        }
    }
    pub fn clear(&mut self) {
        self.indices.clear();
        self.pages.clear();
        self.leaves.clear();
        self.generation = self.generation.wrapping_add(1);
    }
    pub fn release(&mut self, owner: &str) {
        self.indices.retain(|entry| entry.owner != owner);
        self.leaves.retain(|entry| entry.source.owner != owner);
    }
    pub fn ensure(
        &mut self,
        owner: &str,
        provider: ScopedSourceProvider,
        reference: ContentRef,
        format: SourceIndexFormat,
    ) -> StoredPreparation {
        self.indices.retain(|entry| entry.provider.is_available());
        self.leaves
            .retain(|entry| entry.source.provider.is_available());
        self.pages.retain(|(provider, _)| provider.is_available());
        if !provider.is_available() {
            return StoredPreparation::Failed("Host disconnected".to_owned());
        }
        if reference.bytes > MAX_SOURCE_BYTES as u64 {
            return StoredPreparation::Failed(
                "Markdown source exceeds the 64 MiB source admission limit; export the full source"
                    .to_owned(),
            );
        }
        if let Some(index) = self.indices.iter().position(|entry| entry.owner == owner) {
            let same = {
                let entry = &self.indices[index];
                Self::same_provider(&entry.provider, &provider)
                    && entry.reference == reference
                    && entry.format == format
            };
            if same {
                return self.current(owner);
            }
            self.indices.remove(index);
            self.leaves.retain(|entry| entry.source.owner != owner);
        }
        while self.indices.len() >= MAX_INDEX_ENTRIES {
            let Some(index) = self.indices.iter().position(|entry| entry.task.is_none()) else {
                return StoredPreparation::Pending;
            };
            self.indices.remove(index);
        }
        let revision = self.next_generation();
        self.indices.push_back(IndexEntry {
            owner: owner.to_owned(),
            provider,
            reference,
            format,
            revision,
            pending_offset: None,
            failed_offset: None,
            current: None,
            error: None,
            task: None,
        });
        self.start_index(owner, None)
    }
    pub fn current(&self, owner: &str) -> StoredPreparation {
        let Some(entry) = self
            .indices
            .iter()
            .find(|entry| entry.owner == owner && entry.provider.is_available())
        else {
            return StoredPreparation::Pending;
        };
        if let Some(current) = &entry.current {
            StoredPreparation::Ready(current.clone())
        } else if let Some(error) = &entry.error {
            StoredPreparation::Failed(error.clone())
        } else {
            StoredPreparation::Pending
        }
    }
    pub fn move_window(
        &mut self,
        owner: &str,
        source: &StoredSource,
        offset: u64,
    ) -> StoredPreparation {
        if !self.source_is_current(source) || source.owner != owner {
            return StoredPreparation::Pending;
        }
        if source.page.start == offset {
            return StoredPreparation::Ready(source.clone());
        }
        if let Some(entry) = self.indices.iter().find(|entry| entry.owner == owner)
            && entry.failed_offset == Some(Some(offset))
            && let Some(error) = &entry.error
        {
            return StoredPreparation::Failed(error.clone());
        }
        self.start_index(owner, Some(offset))
    }
    fn start_index(&mut self, owner: &str, offset: Option<u64>) -> StoredPreparation {
        let Some(index) = self.indices.iter().position(|entry| entry.owner == owner) else {
            return StoredPreparation::Pending;
        };
        if self.indices[index].pending_offset == Some(offset) {
            return StoredPreparation::Pending;
        }
        let cached = self.pages.iter().position(|(provider, page)| {
            let entry = &self.indices[index];
            Self::same_provider(provider, &entry.provider)
                && page.index.source == entry.reference
                && page.index.format == entry.format
                && offset.map_or(page.end == page.index.leaves, |offset| {
                    page.start == offset.min(page.index.leaves)
                })
        });
        if let Some(position) = cached {
            let (_, page) = self.pages.remove(position).unwrap();
            let revision = self.next_generation();
            let entry = &mut self.indices[index];
            if let Some(task) = entry.task.take() {
                task.abort();
            }
            entry.pending_offset = None;
            entry.failed_offset = None;
            entry.error = None;
            entry.revision = revision;
            let source = StoredSource {
                provider: entry.provider.clone(),
                page: page.clone(),
                revision,
                owner: owner.to_owned(),
            };
            entry.current = Some(source.clone());
            self.pages.push_back((entry.provider.clone(), page));
            return StoredPreparation::Ready(source);
        }
        let revision = self.next_generation();
        let entry = &mut self.indices[index];
        if let Some(task) = entry.task.take() {
            task.abort();
        }
        let Some(future) = entry.provider.index_range(
            entry.reference.clone(),
            entry.format,
            offset,
            artisan_api::SOURCE_INDEX_PAGE_LEAVES,
        ) else {
            return StoredPreparation::Failed("Host disconnected".to_owned());
        };
        entry.revision = revision;
        entry.pending_offset = Some(offset);
        entry.error = None;
        entry.failed_offset = None;
        let owner = owner.to_owned();
        let sender = self.delivery.sender();
        entry.task = Some(
            runtime::runtime()
                .spawn(async move {
                    let result = future.await.map_err(|error| error.to_string());
                    let _ = sender
                        .send(Completion::Index {
                            owner,
                            revision,
                            result,
                        })
                        .await;
                })
                .abort_handle(),
        );
        StoredPreparation::Pending
    }
    pub fn source_is_current(&self, source: &StoredSource) -> bool {
        source.provider.is_available()
            && self.indices.iter().any(|entry| {
                entry.owner == source.owner
                    && Self::same_provider(&entry.provider, &source.provider)
                    && entry.current.as_ref().is_some_and(|current| {
                        current.revision == source.revision
                            && current.page.index.source == source.page.index.source
                            && current.page.index.format == source.page.index.format
                    })
            })
    }
    pub fn leaf_is_current(&self, proof: &LeafProof) -> bool {
        proof.provider.is_available()
            && self.indices.iter().any(|entry| {
                entry.owner == proof.owner
                    && Self::same_provider(&entry.provider, &proof.provider)
                    && entry.reference == proof.reference
                    && entry.format == SourceIndexFormat::Markdown
                    && entry.current.as_ref().and_then(|source| {
                        source.descriptor(usize::try_from(proof.descriptor.ordinal).ok()?)
                    }) == Some(&proof.descriptor)
            })
            && self.leaves.iter().any(|entry| {
                entry.generation == proof.generation
                    && entry.source.owner == proof.owner
                    && entry.descriptor == proof.descriptor
                    && matches!(entry.phase, LeafPhase::Ready(_))
            })
    }
    pub fn leaf(&mut self, key: &str, source: &StoredSource, ordinal: usize) -> LeafPreparation {
        if source.format() != SourceIndexFormat::Markdown {
            return LeafPreparation::Failed(
                "Literal sources require the literal projection renderer".to_owned(),
            );
        }
        if !self.source_is_current(source) {
            return LeafPreparation::Pending;
        }
        let Some(descriptor) = source.descriptor(ordinal).cloned() else {
            return LeafPreparation::Pending;
        };
        if let Some(index) = self.leaves.iter().position(|entry| entry.key == key) {
            let same = {
                let entry = &self.leaves[index];
                entry.source.owner == source.owner
                    && Self::same_provider(&entry.source.provider, &source.provider)
                    && entry.source.page.index.source == source.page.index.source
                    && entry.descriptor == descriptor
            };
            if same {
                let entry = self.leaves.remove(index).unwrap();
                let result = match &entry.phase {
                    LeafPhase::Ready(leaf) => LeafPreparation::Ready(leaf.clone()),
                    LeafPhase::Failed(error) => LeafPreparation::Failed(error.clone()),
                    _ => LeafPreparation::Pending,
                };
                self.leaves.push_back(entry);
                return result;
            }
            self.leaves.remove(index);
        }
        let source_len = descriptor.source_end - descriptor.source_start;
        let metadata_len = descriptor.metadata_end - descriptor.metadata_start;
        let Some(bytes) = source_len
            .checked_add(metadata_len)
            .and_then(|size| size.checked_add(u64::from(RANGE_BYTES)))
        else {
            return LeafPreparation::Failed(
                "Markdown source exceeds the preparation ownership budget; open the full source"
                    .to_owned(),
            );
        };
        if bytes > PREPARED_BYTES as u64 {
            return LeafPreparation::Failed(
                "Markdown source exceeds the preparation ownership budget; open the full source"
                    .to_owned(),
            );
        }
        if self
            .leaves
            .iter()
            .filter(|entry| !matches!(entry.phase, LeafPhase::Ready(_) | LeafPhase::Failed(_)))
            .count()
            >= MAX_PENDING_JOBS
        {
            return LeafPreparation::Pending;
        }
        while self.leaves.len() >= MAX_PREPARED_ENTRIES {
            let Some(index) = self.leaves.iter().position(|entry| {
                matches!(entry.phase, LeafPhase::Ready(_) | LeafPhase::Failed(_))
            }) else {
                return LeafPreparation::Pending;
            };
            self.leaves.remove(index);
        }
        let capacity = loop {
            if let Ok(capacity) = self
                .capacity
                .clone()
                .try_acquire_many_owned(u32::try_from(bytes).unwrap_or(u32::MAX))
            {
                break Arc::new(capacity);
            }
            let Some(index) = self.leaves.iter().position(|entry| {
                matches!(entry.phase, LeafPhase::Ready(_) | LeafPhase::Failed(_))
            }) else {
                return LeafPreparation::Pending;
            };
            self.leaves.remove(index);
        };
        let metadata_start = match usize::try_from(descriptor.metadata_start) {
            Ok(value) => value,
            Err(_) => {
                return LeafPreparation::Failed(
                    "Markdown source offset exceeds this platform".to_owned(),
                );
            }
        };
        let metadata_end = match usize::try_from(descriptor.metadata_end) {
            Ok(value) => value,
            Err(_) => {
                return LeafPreparation::Failed(
                    "Markdown source offset exceeds this platform".to_owned(),
                );
            }
        };
        let collector = match RangeCollector::new(
            source.page.index.metadata.clone(),
            metadata_start..metadata_end,
        ) {
            Ok(value) => value,
            Err(error) => return LeafPreparation::Failed(error),
        };
        let generation = self.next_generation();
        self.leaves.push_back(LeafEntry {
            key: key.to_owned(),
            source: source.clone(),
            descriptor,
            generation,
            capacity: Some(capacity),
            phase: LeafPhase::Metadata(collector),
            task: None,
        });
        self.advance_leaf(key);
        LeafPreparation::Pending
    }
    fn advance_leaf(&mut self, key: &str) -> bool {
        let Some(index) = self.leaves.iter().position(|entry| entry.key == key) else {
            return false;
        };
        let entry = &mut self.leaves[index];
        if entry.task.is_some() || !entry.source.provider.is_available() {
            return false;
        }
        let request = match &entry.phase {
            LeafPhase::Metadata(collector) | LeafPhase::Source { collector, .. } => collector
                .next_request()
                .map(|(offset, limit)| (collector.reference.clone(), offset, limit)),
            _ => None,
        };
        if let Some((reference, offset, limit)) = request {
            let Some(future) = entry
                .source
                .provider
                .content_range(reference, offset, limit)
            else {
                return false;
            };
            let generation = entry.generation;
            let key = key.to_owned();
            let sender = self.delivery.sender();
            let capacity = entry.capacity.as_ref().unwrap().clone();
            entry.task = Some(
                runtime::runtime()
                    .spawn(async move {
                        let _capacity = capacity;
                        let result = future.await.map_err(|error| error.to_string());
                        let _ = sender
                            .send(Completion::Range {
                                key,
                                generation,
                                result,
                            })
                            .await;
                    })
                    .abort_handle(),
            );
            return false;
        }
        let LeafPhase::WaitingWorker { .. } = &entry.phase else {
            return false;
        };
        let Some(queue) = self.worker.as_ref().cloned() else {
            entry.phase = LeafPhase::Failed("Couldn't start Markdown preparation".to_owned());
            entry.capacity = None;
            return true;
        };
        let slot = match queue.try_reserve() {
            Ok(slot) => slot,
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => return false,
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                entry.phase = LeafPhase::Failed("Markdown preparation worker stopped".to_owned());
                entry.capacity = None;
                return true;
            }
        };
        let LeafPhase::WaitingWorker { metadata, source } =
            std::mem::replace(&mut entry.phase, LeafPhase::Working)
        else {
            return false;
        };
        let descriptor = entry.descriptor.clone();
        let generation = entry.generation;
        let key = key.to_owned();
        let sender = self.delivery.sender();
        let capacity = entry.capacity.as_ref().unwrap().clone();
        let (reply, receiver) = tokio::sync::oneshot::channel();
        let job: WorkerJob = Box::new(move |engine| {
            if reply.is_closed() {
                return;
            }
            let result = (|| {
                let record: artisan_markdown_index::LeafRecord =
                    serde_json::from_str(&metadata).map_err(|error| error.to_string())?;
                let start = usize::try_from(descriptor.source_start)
                    .map_err(|_| "Markdown source offset exceeds this platform")?;
                let end = usize::try_from(descriptor.source_end)
                    .map_err(|_| "Markdown source offset exceeds this platform")?;
                if record.leaf.source_range() != (start..end) {
                    return Err("Markdown metadata does not match its descriptor".to_owned());
                }
                let engine = engine.as_ref().map_err(Clone::clone)?;
                let content = normalize_record(&record, start, &source, engine)
                    .map_err(|error| error.to_string())?;
                let (mut document, table_header) = match content {
                    IndexedLeafContent::Block(block) => (
                        super::MarkdownDocument {
                            blocks: vec![block],
                        },
                        None,
                    ),
                    IndexedLeafContent::TableRow {
                        header,
                        alignments,
                        cells,
                    } => (
                        super::MarkdownDocument {
                            blocks: vec![super::Block::Table(super::Table {
                                alignments,
                                header: cells,
                                rows: Vec::new(),
                                range: 0..source.len(),
                            })],
                        },
                        Some(header),
                    ),
                };
                let syntax_bytes = document.resident_bytes();
                if syntax_bytes > PREPARED_BYTES / 8 {
                    return Err(
                        "Markdown leaf exceeds its syntax-memory budget; open the full source"
                            .to_owned(),
                    );
                }
                let block = document
                    .blocks
                    .pop()
                    .ok_or("Markdown leaf has no content")?;
                let content = match (table_header, block) {
                    (None, block) => IndexedLeafContent::Block(block),
                    (Some(header), super::Block::Table(table)) => IndexedLeafContent::TableRow {
                        header,
                        alignments: table.alignments,
                        cells: table.header,
                    },
                    _ => return Err("Markdown metadata does not match its content".to_owned()),
                };
                let context = record.context;
                let context_bytes = context.lists.capacity()
                    * std::mem::size_of::<artisan_markdown_index::ListLevel>()
                    + context.table_alignments.capacity()
                        * std::mem::size_of::<artisan_markdown_index::TableAlignment>()
                    + context.spacing.capacity()
                        * std::mem::size_of::<artisan_markdown_index::LeafSpacing>();
                let retained_bytes = syntax_bytes
                    .saturating_add(context_bytes)
                    .saturating_add(source.len())
                    .saturating_add(std::mem::size_of::<StoredLeaf>());
                Ok((
                    context,
                    record.table_continues,
                    content,
                    Arc::<str>::from(source),
                    retained_bytes,
                ))
            })();
            let _ = reply.send((result, capacity));
        });
        slot.send(job);
        entry.task = Some(
            runtime::runtime()
                .spawn(async move {
                    let (result, worker, capacity) = match receiver.await {
                        Ok((result, capacity)) => (result, None, Some(capacity)),
                        Err(error) => (Err(error.to_string()), None, None),
                    };
                    let _ = sender
                        .send(Completion::Prepared {
                            key,
                            generation,
                            result,
                            _worker: worker,
                            _capacity: capacity,
                        })
                        .await;
                })
                .abort_handle(),
        );
        false
    }
    pub fn drain(&mut self) -> Vec<StoredUpdate> {
        self.indices.retain(|entry| entry.provider.is_available());
        self.leaves
            .retain(|entry| entry.source.provider.is_available());
        self.pages.retain(|(provider, _)| provider.is_available());
        let mut updates = Vec::new();
        for completion in self.delivery.drain() {
            match completion {
                Completion::Index {
                    owner,
                    revision,
                    result,
                } => {
                    let Some(entry) = self.indices.iter_mut().find(|entry| {
                        entry.owner == owner
                            && entry.revision == revision
                            && entry.provider.is_available()
                    }) else {
                        continue;
                    };
                    entry.task = None;
                    let offset = entry.pending_offset.take().unwrap_or(None);
                    match result.and_then(|page| {
                        validate_page(&entry.reference, entry.format, offset, &page)?;
                        Ok(page)
                    }) {
                        Ok(page) => {
                            let source = StoredSource {
                                provider: entry.provider.clone(),
                                page: Arc::new(page),
                                revision,
                                owner: owner.clone(),
                            };
                            let provider = entry.provider.clone();
                            entry.current = Some(source.clone());
                            self.remember_page(provider, source.page);
                        }
                        Err(error) => {
                            entry.failed_offset = Some(offset);
                            entry.error = Some(error);
                        }
                    }
                    updates.push(StoredUpdate::Index { owner, revision });
                }
                Completion::Range {
                    key,
                    generation,
                    result,
                } => {
                    let Some(entry) = self.leaves.iter_mut().find(|entry| {
                        entry.key == key
                            && entry.generation == generation
                            && entry.source.provider.is_available()
                    }) else {
                        continue;
                    };
                    entry.task = None;
                    let phase = std::mem::replace(&mut entry.phase, LeafPhase::Working);
                    entry.phase = match (phase, result) {
                        (_, Err(error)) => LeafPhase::Failed(error),
                        (LeafPhase::Metadata(mut collector), Ok(page)) => {
                            match collector.push(page) {
                                Err(error) => LeafPhase::Failed(error),
                                Ok(()) if collector.next_request().is_some() => {
                                    LeafPhase::Metadata(collector)
                                }
                                Ok(()) => match collector.finish().and_then(|metadata| {
                                    let start = usize::try_from(entry.descriptor.source_start)
                                        .map_err(
                                            |_| "Markdown source offset exceeds this platform",
                                        )?;
                                    let end = usize::try_from(entry.descriptor.source_end)
                                        .map_err(
                                            |_| "Markdown source offset exceeds this platform",
                                        )?;
                                    Ok((
                                        metadata,
                                        RangeCollector::new(
                                            entry.source.page.index.source.clone(),
                                            start..end,
                                        )?,
                                    ))
                                }) {
                                    Ok((metadata, collector))
                                        if collector.next_request().is_some() =>
                                    {
                                        LeafPhase::Source {
                                            metadata,
                                            collector,
                                        }
                                    }
                                    Ok((metadata, collector)) => match collector.finish() {
                                        Ok(source) => LeafPhase::WaitingWorker { metadata, source },
                                        Err(error) => LeafPhase::Failed(error),
                                    },
                                    Err(error) => LeafPhase::Failed(error),
                                },
                            }
                        }
                        (
                            LeafPhase::Source {
                                metadata,
                                mut collector,
                            },
                            Ok(page),
                        ) => match collector.push(page) {
                            Err(error) => LeafPhase::Failed(error),
                            Ok(()) if collector.next_request().is_some() => LeafPhase::Source {
                                metadata,
                                collector,
                            },
                            Ok(()) => match collector.finish() {
                                Ok(source) => LeafPhase::WaitingWorker { metadata, source },
                                Err(error) => LeafPhase::Failed(error),
                            },
                        },
                        (other, Ok(_)) => other,
                    };
                    if matches!(entry.phase, LeafPhase::Failed(_)) {
                        entry.capacity = None;
                        updates.push(StoredUpdate::Leaf {
                            key: key.clone(),
                            ordinal: usize::try_from(entry.descriptor.ordinal)
                                .unwrap_or(usize::MAX),
                        });
                    }
                }
                Completion::Prepared {
                    key,
                    generation,
                    result,
                    _worker,
                    _capacity,
                } => {
                    if !self.leaves.iter().any(|entry| {
                        entry.key == key
                            && entry.generation == generation
                            && entry.source.provider.is_available()
                    }) {
                        continue;
                    }
                    let result = result.and_then(|(context, table_continues, content, source, bytes)| {
                        if bytes > PREPARED_BYTES {
                            return Err("Markdown preview exceeds its presentation-memory budget; open the full source".to_owned());
                        }
                        let permit = loop {
                            if let Ok(permit) = self.prepared_capacity.clone().try_acquire_many_owned(bytes as u32) {
                                break permit;
                            }
                            let Some(oldest) = self.leaves.iter().position(|entry| entry.key != key && matches!(entry.phase, LeafPhase::Ready(_) | LeafPhase::Failed(_))) else {
                                return Err("Markdown preview exceeds its presentation-memory budget; open the full source".to_owned());
                            };
                            self.leaves.remove(oldest);
                        };
                        Ok((context, table_continues, content, source, permit))
                    });
                    let Some(entry) = self
                        .leaves
                        .iter_mut()
                        .find(|entry| entry.key == key && entry.generation == generation)
                    else {
                        continue;
                    };
                    entry.task = None;
                    entry.capacity = None;
                    let ordinal = usize::try_from(entry.descriptor.ordinal).unwrap_or(usize::MAX);
                    entry.phase = match result {
                        Ok((context, table_continues, content, source, capacity)) => {
                            LeafPhase::Ready(Arc::new(StoredLeaf {
                                descriptor: entry.descriptor.clone(),
                                source,
                                context,
                                table_continues,
                                content,
                                proof: LeafProof {
                                    provider: entry.source.provider.clone(),
                                    owner: entry.source.owner.clone(),
                                    reference: entry.source.page.index.source.clone(),
                                    descriptor: entry.descriptor.clone(),
                                    generation,
                                },
                                _capacity: PreparedLease {
                                    _permit: Some(capacity),
                                    wake: self.delivery.waker(),
                                },
                            }))
                        }
                        Err(error) => LeafPhase::Failed(error),
                    };
                    updates.push(StoredUpdate::Leaf { key, ordinal });
                }
            }
        }
        let keys: Vec<_> = self
            .leaves
            .iter()
            .filter(|entry| {
                entry.task.is_none()
                    && !matches!(
                        entry.phase,
                        LeafPhase::Ready(_) | LeafPhase::Failed(_) | LeafPhase::Working
                    )
            })
            .map(|entry| entry.key.clone())
            .collect();
        for key in keys {
            if self.advance_leaf(&key)
                && let Some(entry) = self.leaves.iter().find(|entry| entry.key == key)
            {
                updates.push(StoredUpdate::Leaf {
                    key,
                    ordinal: usize::try_from(entry.descriptor.ordinal).unwrap_or(usize::MAX),
                });
            }
        }
        updates
    }
}

#[cfg(test)]
#[path = "../../../tests/markdown/stored.rs"]
mod tests;
