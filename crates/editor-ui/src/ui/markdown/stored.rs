use std::ops::Range;
use std::rc::Rc;

use artisan_api::{ContentRange, ContentRef, SourceIndexFormat, SourceIndexRange};

const MAX_SOURCE_BYTES: usize = 64 * 1024 * 1024;
const RANGE_BYTES: u32 = 65_536;
const MAX_INDEX_ENTRIES: usize = 48;
pub const PAGE_LEAVES: usize = artisan_api::SOURCE_INDEX_PAGE_LEAVES as usize;
const PAGES_PER_SOURCE: usize = 8;
const MAX_CACHED_PAGES: usize = 64;
const MAX_PREPARED_ENTRIES: usize = 128;
const MAX_PENDING_JOBS: usize = 48;
pub(crate) const PREPARED_BYTES: usize = 32 * 1024 * 1024;
const QUEUE_DEPTH: usize = 8;

fn validate_page(
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
        let source_fits = (leaf.source_start..=reference.bytes).contains(&leaf.source_end);
        let metadata_fits = leaf.metadata_start < leaf.metadata_end
            && leaf.metadata_end <= page.index.metadata.bytes;
        if leaf.ordinal != page.start + position as u64 || !source_fits || !metadata_fits {
            return Err("Markdown descriptor does not match its source or metadata".to_owned());
        }
    }
    Ok(())
}

fn platform_offset(offset: u64) -> Result<usize, String> {
    usize::try_from(offset)
        .map_err(|error| format!("Markdown source offset {offset} exceeds this platform: {error}"))
}

#[derive(Debug)]
pub struct RangeCollector {
    reference: ContentRef,
    range: Range<usize>,
    cursor: usize,
    text: String,
}

impl RangeCollector {
    pub(crate) fn new(reference: ContentRef, range: Range<usize>) -> Result<Self, String> {
        if range.start > range.end
            || u64::try_from(range.end).map_or(true, |end| end > reference.bytes)
            || range.len() > MAX_SOURCE_BYTES
        {
            return Err("Markdown source range exceeds its admission limit".to_owned());
        }
        let cursor = range.start;
        let text = String::with_capacity(range.len());
        Ok(Self {
            reference,
            range,
            cursor,
            text,
        })
    }

    pub(crate) fn next_request(&self) -> Option<(u64, u32)> {
        (self.cursor < self.range.end).then(|| {
            let remaining = self.range.end - self.cursor;
            (
                self.cursor as u64,
                RANGE_BYTES.min(u32::try_from(remaining.max(4)).unwrap_or(u32::MAX)),
            )
        })
    }

    pub(crate) fn push(&mut self, page: ContentRange) -> Result<(), String> {
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
        let start = platform_offset(cursor - page.start)?;
        let end = platform_offset(page.end.min(self.range.end as u64) - page.start)?;
        let fragment = page
            .text
            .get(start..end)
            .ok_or("Markdown source response splits a UTF-8 boundary")?;
        self.text.push_str(fragment);
        self.cursor = platform_offset(page.end.min(self.range.end as u64))?;
        Ok(())
    }

    pub(crate) fn finish(self) -> Result<String, String> {
        if self.cursor != self.range.end {
            return Err("Markdown source range is incomplete".to_owned());
        }
        Ok(self.text)
    }
}

use std::collections::VecDeque;
use std::num::NonZeroUsize;
use std::sync::Arc;

use artisan_api::{SourceIndexHeader, SourceLeafDescriptor};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::task::AbortHandle;

use super::{IndexedLeafContent, MarkdownEngine, normalize_record};
use editor_core::{delivery::Delivery, host::ScopedSourceProvider, runtime};

#[derive(Clone)]
pub struct StoredSource {
    provider: ScopedSourceProvider,
    header: Arc<SourceIndexHeader>,
    revision: u64,
    owner: String,
}

impl std::fmt::Debug for StoredSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredSource").finish_non_exhaustive()
    }
}

impl StoredSource {
    pub(crate) fn literal_content_range(
        &self,
        offset: u64,
        limit: u32,
    ) -> Option<
        futures::future::BoxFuture<'static, Result<ContentRange, editor_core::client::RangeError>>,
    > {
        if self.format() != SourceIndexFormat::Literal || limit == 0 || limit > RANGE_BYTES {
            return None;
        }
        self.provider
            .content_range(self.source_ref().clone(), offset, limit)
    }

    pub fn source_ref(&self) -> &ContentRef {
        &self.header.source
    }
    pub fn format(&self) -> SourceIndexFormat {
        self.header.format
    }
    pub fn block_count(&self) -> usize {
        usize::try_from(self.header.leaves).unwrap_or(usize::MAX)
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    fn holds(&self, provider: &ScopedSourceProvider, page: &SourceIndexRange) -> bool {
        StoredOwner::same_provider(&self.provider, provider) && page.index == *self.header
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PagePreparation {
    Pending,
    Ready(SourceLeafDescriptor),
    Failed { offset: u64, cause: String },
}

fn page_start(ordinal: usize) -> u64 {
    (ordinal - ordinal % PAGE_LEAVES) as u64
}

#[derive(Debug, Clone)]
pub enum StoredPreparation {
    Pending,
    Ready(StoredSource),
    Failed(String),
}

#[derive(Debug, Clone)]
pub enum LeafPreparation {
    Pending,
    Ready(Rc<StoredLeaf>),
    Failed(String),
}

struct PreparedLease {
    permit: Option<OwnedSemaphorePermit>,
    wake: aqua::mailbox::Wake,
}

impl Drop for PreparedLease {
    fn drop(&mut self) {
        drop(self.permit.take());
        (self.wake)();
    }
}

pub struct StoredLeaf {
    pub(crate) descriptor: SourceLeafDescriptor,
    pub(crate) source: Arc<str>,
    pub(crate) context: artisan_markdown_index::LeafContext,
    pub(crate) table_continues: bool,
    pub(crate) content: IndexedLeafContent,
    proof: LeafProof,
    _capacity: PreparedLease,
}

impl StoredLeaf {
    pub fn source(&self) -> &str {
        &self.source
    }

    pub const fn context(&self) -> &artisan_markdown_index::LeafContext {
        &self.context
    }

    pub const fn content(&self) -> &IndexedLeafContent {
        &self.content
    }
}

impl std::fmt::Debug for StoredLeaf {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredLeaf").finish_non_exhaustive()
    }
}

#[derive(Clone)]
pub struct LeafProof {
    provider: ScopedSourceProvider,
    owner: String,
    reference: ContentRef,
    descriptor: SourceLeafDescriptor,
    generation: u64,
}

impl std::fmt::Debug for LeafProof {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LeafProof").finish_non_exhaustive()
    }
}

impl LeafProof {
    pub(crate) fn owner(&self) -> &str {
        &self.owner
    }
}

impl StoredLeaf {
    pub fn proof(&self) -> &LeafProof {
        &self.proof
    }
}

struct Task(AbortHandle);

impl Drop for Task {
    fn drop(&mut self) {
        self.0.abort();
    }
}

enum IndexPhase {
    Loading { _task: Task },
    Ready(StoredSource),
    Failed(String),
}

enum FetchState {
    Loading { _task: Task },
    Failed(String),
}

struct PageFetch {
    start: u64,
    state: FetchState,
}

struct IndexEntry {
    owner: String,
    provider: ScopedSourceProvider,
    reference: ContentRef,
    format: SourceIndexFormat,
    revision: u64,
    phase: IndexPhase,
    fetches: Vec<PageFetch>,
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
    Ready(Rc<StoredLeaf>),
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
        offset: Option<u64>,
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

fn preparation_worker() -> Result<tokio::sync::mpsc::Sender<WorkerJob>, String> {
    let (sender, mut receiver) = tokio::sync::mpsc::channel::<WorkerJob>(QUEUE_DEPTH);
    std::thread::Builder::new()
        .name("artisan-markdown-stored".to_owned())
        .spawn(move || {
            let engine = MarkdownEngine::new().map_err(|error| error.to_string());
            while let Some(job) = receiver.blocking_recv() {
                job(&engine);
            }
        })
        .map_err(|error| format!("Couldn't start Markdown preparation: {error}"))?;
    Ok(sender)
}

pub struct StoredOwner {
    indices: VecDeque<IndexEntry>,
    pages: VecDeque<(ScopedSourceProvider, Arc<SourceIndexRange>)>,
    leaves: VecDeque<LeafEntry>,
    generation: u64,
    capacity: Arc<Semaphore>,
    prepared_capacity: Arc<Semaphore>,
    worker: Result<tokio::sync::mpsc::Sender<WorkerJob>, String>,
    delivery: Delivery<Completion>,
}

impl std::fmt::Debug for StoredOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredOwner").finish_non_exhaustive()
    }
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
            delivery: Delivery::new(
                const { NonZeroUsize::new(MAX_PENDING_JOBS).expect("MAX_PENDING_JOBS is not zero") },
            ),
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
    fn same_source(
        provider: &ScopedSourceProvider,
        page: &SourceIndexRange,
        other_provider: &ScopedSourceProvider,
        other: &SourceIndexRange,
    ) -> bool {
        Self::same_provider(provider, other_provider)
            && page.index.source == other.index.source
            && page.index.format == other.index.format
    }
    fn remember_page(&mut self, provider: ScopedSourceProvider, page: Arc<SourceIndexRange>) {
        self.pages.retain(|(existing_provider, existing)| {
            !(Self::same_source(existing_provider, existing, &provider, &page)
                && existing.start == page.start)
        });
        let same =
            |(existing_provider, existing): &(ScopedSourceProvider, Arc<SourceIndexRange>)| {
                Self::same_source(existing_provider, existing, &provider, &page)
            };
        if self.pages.iter().filter(|entry| same(entry)).count() >= PAGES_PER_SOURCE
            && let Some(oldest) = self.pages.iter().position(same)
        {
            self.pages.remove(oldest);
        }
        self.pages.push_back((provider, page));
        while self.pages.len() > MAX_CACHED_PAGES {
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
            let Some(index) = self
                .indices
                .iter()
                .position(|entry| !matches!(entry.phase, IndexPhase::Loading { .. }))
            else {
                return StoredPreparation::Pending;
            };
            self.indices.remove(index);
        }
        let revision = self.next_generation();
        let cached = self
            .pages
            .iter()
            .find(|(cached_provider, page)| {
                Self::same_provider(cached_provider, &provider)
                    && page.index.source == reference
                    && page.index.format == format
                    && page.end == page.index.leaves
            })
            .map(|(_, page)| Arc::clone(page));
        let phase = if let Some(page) = cached {
            IndexPhase::Ready(StoredSource {
                provider: provider.clone(),
                header: Arc::new(page.index.clone()),
                revision,
                owner: owner.to_owned(),
            })
        } else {
            let Some(future) = provider.index_range(
                reference.clone(),
                format,
                None,
                artisan_api::SOURCE_INDEX_PAGE_LEAVES,
            ) else {
                return StoredPreparation::Failed("Host disconnected".to_owned());
            };
            IndexPhase::Loading {
                _task: self.spawn_index(owner, revision, None, future),
            }
        };
        self.indices.push_back(IndexEntry {
            owner: owner.to_owned(),
            provider,
            reference,
            format,
            revision,
            phase,
            fetches: Vec::new(),
        });
        self.current(owner)
    }
    fn spawn_index(
        &self,
        owner: &str,
        revision: u64,
        offset: Option<u64>,
        future: futures::future::BoxFuture<
            'static,
            Result<SourceIndexRange, editor_core::client::RangeError>,
        >,
    ) -> Task {
        let owner = owner.to_owned();
        let sender = self.delivery.sender();
        Task(
            runtime::runtime()
                .spawn(async move {
                    let result = future.await.map_err(|error| error.to_string());
                    crate::ui::deliver::deliver(
                        &sender,
                        Completion::Index {
                            owner,
                            revision,
                            offset,
                            result,
                        },
                    )
                    .await;
                })
                .abort_handle(),
        )
    }
    pub fn current(&self, owner: &str) -> StoredPreparation {
        let Some(entry) = self
            .indices
            .iter()
            .find(|entry| entry.owner == owner && entry.provider.is_available())
        else {
            return StoredPreparation::Pending;
        };
        match &entry.phase {
            IndexPhase::Loading { .. } => StoredPreparation::Pending,
            IndexPhase::Ready(source) => StoredPreparation::Ready(source.clone()),
            IndexPhase::Failed(error) => StoredPreparation::Failed(error.clone()),
        }
    }
    pub fn descriptor(
        &self,
        source: &StoredSource,
        ordinal: usize,
    ) -> Option<&SourceLeafDescriptor> {
        let ordinal = u64::try_from(ordinal).ok()?;
        self.pages
            .iter()
            .find(|(provider, page)| {
                source.holds(provider, page) && (page.start..page.end).contains(&ordinal)
            })
            .and_then(|(_, page)| page.leaves.get(usize::try_from(ordinal - page.start).ok()?))
    }
    pub fn page(&mut self, source: &StoredSource, ordinal: usize) -> PagePreparation {
        if !self.source_is_current(source) || ordinal >= source.block_count() {
            return PagePreparation::Pending;
        }
        let wanted = ordinal as u64;
        if let Some(position) = self.pages.iter().position(|(provider, page)| {
            source.holds(provider, page) && (page.start..page.end).contains(&wanted)
        }) {
            let cached = self
                .pages
                .remove(position)
                .expect("invariant: a found page position is in the cache");
            let descriptor = cached.1.leaves[usize::try_from(wanted - cached.1.start)
                .expect("invariant: a page holds at most SOURCE_INDEX_PAGE_LEAVES leaves")]
            .clone();
            self.pages.push_back(cached);
            return PagePreparation::Ready(descriptor);
        }
        let start = page_start(ordinal);
        let index = self
            .indices
            .iter()
            .position(|entry| entry.owner == source.owner)
            .expect("invariant: a current stored source keeps its index entry");
        match self.indices[index]
            .fetches
            .iter()
            .find(|fetch| fetch.start == start)
            .map(|fetch| &fetch.state)
        {
            Some(FetchState::Loading { .. }) => PagePreparation::Pending,
            Some(FetchState::Failed(cause)) => PagePreparation::Failed {
                offset: start,
                cause: cause.clone(),
            },
            None => self.start_page(index, start),
        }
    }
    pub fn retry_page(&mut self, owner: &str, offset: u64) -> bool {
        let Some(index) = self.indices.iter().position(|entry| {
            entry.owner == owner
                && entry.provider.is_available()
                && matches!(entry.phase, IndexPhase::Ready(_))
        }) else {
            return false;
        };
        let fetches = &mut self.indices[index].fetches;
        let Some(failed) = fetches.iter().position(|fetch| {
            fetch.start == offset && matches!(fetch.state, FetchState::Failed(_))
        }) else {
            return false;
        };
        fetches.remove(failed);
        self.start_page(index, offset);
        true
    }
    fn start_page(&mut self, index: usize, start: u64) -> PagePreparation {
        let entry = &self.indices[index];
        if entry
            .fetches
            .iter()
            .filter(|fetch| matches!(fetch.state, FetchState::Loading { .. }))
            .count()
            >= PAGES_PER_SOURCE
        {
            return PagePreparation::Pending;
        }
        let state = match entry.provider.index_range(
            entry.reference.clone(),
            entry.format,
            Some(start),
            artisan_api::SOURCE_INDEX_PAGE_LEAVES,
        ) {
            Some(future) => FetchState::Loading {
                _task: self.spawn_index(&entry.owner, entry.revision, Some(start), future),
            },
            None => FetchState::Failed("Host disconnected".to_owned()),
        };
        let fetches = &mut self.indices[index].fetches;
        if fetches.len() >= PAGES_PER_SOURCE
            && let Some(oldest) = fetches
                .iter()
                .position(|fetch| matches!(fetch.state, FetchState::Failed(_)))
        {
            fetches.remove(oldest);
        }
        let preparation = match &state {
            FetchState::Loading { .. } => PagePreparation::Pending,
            FetchState::Failed(cause) => PagePreparation::Failed {
                offset: start,
                cause: cause.clone(),
            },
        };
        fetches.push(PageFetch { start, state });
        preparation
    }
    pub fn source_is_current(&self, source: &StoredSource) -> bool {
        source.provider.is_available()
            && self.indices.iter().any(|entry| {
                entry.owner == source.owner
                    && Self::same_provider(&entry.provider, &source.provider)
                    && matches!(&entry.phase, IndexPhase::Ready(current) if current.revision == source.revision && current.header == source.header)
            })
    }
    pub fn leaf_is_current(&self, proof: &LeafProof) -> bool {
        proof.provider.is_available()
            && self.indices.iter().any(|entry| {
                entry.owner == proof.owner
                    && Self::same_provider(&entry.provider, &proof.provider)
                    && entry.reference == proof.reference
                    && entry.format == SourceIndexFormat::Markdown
                    && matches!(entry.phase, IndexPhase::Ready(_))
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
        let Some(descriptor) = self.descriptor(source, ordinal).cloned() else {
            return LeafPreparation::Pending;
        };
        let found = self.leaves.iter().position(|entry| entry.key == key);
        if let Some(index) = found {
            let same = {
                let entry = &self.leaves[index];
                entry.source.owner == source.owner
                    && Self::same_provider(&entry.source.provider, &source.provider)
                    && entry.source.header.source == source.header.source
                    && entry.descriptor == descriptor
            };
            if same {
                let entry = self
                    .leaves
                    .remove(index)
                    .unwrap_or_else(|| panic!("invariant: leaf {key} was found at {index}"));
                let result = match &entry.phase {
                    LeafPhase::Ready(leaf) => LeafPreparation::Ready(Rc::clone(leaf)),
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
            if let Ok(capacity) = Arc::clone(&self.capacity)
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
        let Ok(metadata_start) = usize::try_from(descriptor.metadata_start) else {
            return LeafPreparation::Failed(
                "Markdown source offset exceeds this platform".to_owned(),
            );
        };
        let Ok(metadata_end) = usize::try_from(descriptor.metadata_end) else {
            return LeafPreparation::Failed(
                "Markdown source offset exceeds this platform".to_owned(),
            );
        };
        let collector =
            match RangeCollector::new(source.header.metadata.clone(), metadata_start..metadata_end)
            {
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
            let capacity = Arc::clone(entry.capacity.as_ref().unwrap_or_else(|| {
                panic!("invariant: leaf {key} holds capacity while it collects")
            }));
            entry.task = Some(
                runtime::runtime()
                    .spawn(async move {
                        let _capacity = capacity;
                        let result = future.await.map_err(|error| error.to_string());
                        crate::ui::deliver::deliver(
                            &sender,
                            Completion::Range {
                                key,
                                generation,
                                result,
                            },
                        )
                        .await;
                    })
                    .abort_handle(),
            );
            return false;
        }
        let LeafPhase::WaitingWorker { .. } = &entry.phase else {
            return false;
        };
        let queue = match &self.worker {
            Ok(queue) => queue.clone(),
            Err(error) => {
                entry.phase = LeafPhase::Failed(error.clone());
                entry.capacity = None;
                return true;
            }
        };
        let slot = match queue.try_reserve() {
            Ok(slot) => slot,
            Err(tokio::sync::mpsc::error::TrySendError::Full(())) => return false,
            Err(tokio::sync::mpsc::error::TrySendError::Closed(())) => {
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
        let capacity = Arc::clone(entry.capacity.as_ref().unwrap_or_else(|| {
            panic!("invariant: leaf {key} holds capacity while it waits for the worker")
        }));
        let (reply, receiver) = tokio::sync::oneshot::channel();
        let job: WorkerJob = Box::new(move |engine| {
            if reply.is_closed() {
                return;
            }
            let result = (|| {
                let record: artisan_markdown_index::LeafRecord =
                    serde_json::from_str(&metadata).map_err(|error| error.to_string())?;
                let start = platform_offset(descriptor.source_start)?;
                let end = platform_offset(descriptor.source_end)?;
                if record.leaf.source_range() != (start..end) {
                    return Err("Markdown metadata does not match its descriptor".to_owned());
                }
                let engine = engine.as_ref().map_err(Clone::clone)?;
                let content = normalize_record(
                    &record,
                    start,
                    &source,
                    engine,
                    &mut super::NoFenceTokenMemo,
                )
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
                let leaf_context = record.context;
                let context_bytes = leaf_context.lists.capacity()
                    * size_of::<artisan_markdown_index::ListLevel>()
                    + leaf_context.table_alignments.capacity()
                        * size_of::<artisan_markdown_index::TableAlignment>()
                    + leaf_context.spacing.capacity()
                        * size_of::<artisan_markdown_index::LeafSpacing>();
                let retained_bytes = syntax_bytes
                    .saturating_add(context_bytes)
                    .saturating_add(source.len())
                    .saturating_add(size_of::<StoredLeaf>());
                Ok((
                    leaf_context,
                    record.table_continues,
                    content,
                    Arc::<str>::from(source),
                    retained_bytes,
                ))
            })();
            match reply.send((result, capacity)) {
                Ok(()) | Err((_, _)) => {}
            }
        });
        slot.send(job);
        entry.task = Some(
            runtime::runtime()
                .spawn(async move {
                    let (result, worker, capacity) = match receiver.await {
                        Ok((result, capacity)) => (result, None, Some(capacity)),
                        Err(error) => (Err(error.to_string()), None, None),
                    };
                    crate::ui::deliver::deliver(
                        &sender,
                        Completion::Prepared {
                            key,
                            generation,
                            result,
                            _worker: worker,
                            _capacity: capacity,
                        },
                    )
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
                    offset,
                    result,
                } => {
                    let Some(entry) = self.indices.iter_mut().find(|entry| {
                        entry.owner == owner
                            && entry.revision == revision
                            && entry.provider.is_available()
                    }) else {
                        continue;
                    };
                    let validated = result.and_then(|page| {
                        validate_page(&entry.reference, entry.format, offset, &page)?;
                        Ok(Arc::new(page))
                    });
                    let provider = entry.provider.clone();
                    let remembered = match (offset, validated) {
                        (None, Ok(page)) => {
                            entry.phase = IndexPhase::Ready(StoredSource {
                                provider: provider.clone(),
                                header: Arc::new(page.index.clone()),
                                revision,
                                owner: owner.clone(),
                            });
                            Some(page)
                        }
                        (None, Err(error)) => {
                            entry.phase = IndexPhase::Failed(error);
                            None
                        }
                        (Some(start), validated) => {
                            let Some(fetch) = entry.fetches.iter_mut().find(|fetch| {
                                fetch.start == start
                                    && matches!(fetch.state, FetchState::Loading { .. })
                            }) else {
                                continue;
                            };
                            let IndexPhase::Ready(source) = &entry.phase else {
                                unreachable!("page {start} of {owner} fetched before its index")
                            };
                            match validated {
                                Ok(page) if source.holds(&provider, &page) => {
                                    entry.fetches.retain(|fetch| fetch.start != start);
                                    Some(page)
                                }
                                Ok(_) => {
                                    fetch.state = FetchState::Failed(
                                        "Markdown descriptor page does not match its immutable source"
                                            .to_owned(),
                                    );
                                    None
                                }
                                Err(error) => {
                                    fetch.state = FetchState::Failed(error);
                                    None
                                }
                            }
                        }
                    };
                    if let Some(page) = remembered {
                        self.remember_page(provider, page);
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
                                    let start = platform_offset(entry.descriptor.source_start)?;
                                    let end = platform_offset(entry.descriptor.source_end)?;
                                    Ok((
                                        metadata,
                                        RangeCollector::new(
                                            entry.source.header.source.clone(),
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
                            if let Ok(permit) = Arc::clone(&self.prepared_capacity).try_acquire_many_owned(u32::try_from(bytes).unwrap_or_else(|_| panic!("invariant: leaf {key} prepares {bytes} bytes, bounded by PREPARED_BYTES"))) {
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
                        Ok((leaf_context, table_continues, content, source, capacity)) => {
                            LeafPhase::Ready(Rc::new(StoredLeaf {
                                descriptor: entry.descriptor.clone(),
                                source,
                                context: leaf_context,
                                table_continues,
                                content,
                                proof: LeafProof {
                                    provider: entry.source.provider.clone(),
                                    owner: entry.source.owner.clone(),
                                    reference: entry.source.header.source.clone(),
                                    descriptor: entry.descriptor.clone(),
                                    generation,
                                },
                                _capacity: PreparedLease {
                                    permit: Some(capacity),
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
