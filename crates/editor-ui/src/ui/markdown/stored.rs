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

    pub(crate) fn push(&mut self, page: &ContentRange) -> Result<(), String> {
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
    permit: std::cell::Cell<Option<OwnedSemaphorePermit>>,
    wake: aqua::mailbox::Wake,
}

impl PreparedLease {
    fn take(&self) -> Option<OwnedSemaphorePermit> {
        self.permit.take()
    }

    fn restore(&self, permit: Option<OwnedSemaphorePermit>) {
        self.permit.set(permit);
    }
}

impl Drop for PreparedLease {
    fn drop(&mut self) {
        drop(self.permit.take());
        (self.wake)();
    }
}

pub struct StoredLeaf {
    pub(crate) descriptor: SourceLeafDescriptor,
    source: Arc<str>,
    pub(crate) context: artisan_markdown_index::LeafContext,
    pub(crate) table_continues: bool,
    pub(crate) content: IndexedLeafContent,
    mentions: LeafMentions,
    proof: LeafProof,
    capacity: PreparedLease,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LeafKind {
    Plain,
    Message,
}

enum LeafMentions {
    Plain,
    Message(Box<MessageMentions>),
}

struct MessageMentions {
    resolved_with: std::cell::RefCell<Arc<artisan_mentions::MessageVocabulary>>,
    record: artisan_markdown_index::LeafRecord,
    record_bytes: usize,
    start: usize,
    applied: Vec<artisan_mentions::MessageMention>,
}

enum Revision {
    Kept,
    Remark(PreparationInput),
}

impl MessageMentions {
    fn current(&self, vocabulary: &Arc<artisan_mentions::MessageVocabulary>) -> bool {
        Arc::ptr_eq(&self.resolved_with.borrow(), vocabulary)
    }

    fn revision(
        &self,
        key: &str,
        source: &Arc<str>,
        vocabulary: &Arc<artisan_mentions::MessageVocabulary>,
    ) -> Revision {
        if self.current(vocabulary) {
            return Revision::Kept;
        }
        match super::mentions::record_mentions(&self.record, self.start, source, vocabulary) {
            Ok(resolved) if resolved == self.applied => {
                self.resolved_with.replace(Arc::clone(vocabulary));
                Revision::Kept
            }
            Ok(_) => Revision::Remark(PreparationInput::Retained {
                record: Box::new(self.record.clone()),
                record_bytes: self.record_bytes,
                source: Arc::clone(source),
            }),
            Err(error) => {
                tracing::warn!(leaf = key, %error, "resolve stored message mentions; keeping the shown marks");
                Revision::Kept
            }
        }
    }
}

impl StoredLeaf {
    fn revision(
        &self,
        key: &str,
        vocabulary: &Arc<artisan_mentions::MessageVocabulary>,
    ) -> Revision {
        match &self.mentions {
            LeafMentions::Plain => Revision::Kept,
            LeafMentions::Message(mentions) => mentions.revision(key, &self.source, vocabulary),
        }
    }
}

enum PreparationInput {
    Fetched {
        metadata: String,
        source: String,
    },
    Retained {
        record: Box<artisan_markdown_index::LeafRecord>,
        record_bytes: usize,
        source: Arc<str>,
    },
}

struct PreparedParts {
    context: artisan_markdown_index::LeafContext,
    table_continues: bool,
    content: IndexedLeafContent,
    mentions: LeafMentions,
    source: Arc<str>,
    bytes: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum PrepareLeafError {
    #[error("Markdown metadata of leaf {leaf} is not a leaf record")]
    Metadata {
        leaf: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("Markdown leaf {leaf} has offset {offset}, beyond this platform")]
    Offset {
        leaf: String,
        offset: u64,
        #[source]
        source: std::num::TryFromIntError,
    },
    #[error("Markdown metadata of leaf {leaf} does not match its descriptor")]
    Descriptor { leaf: String },
    #[error("Markdown preparation is unavailable for leaf {leaf}")]
    Engine {
        leaf: String,
        #[source]
        source: Arc<super::MarkdownError>,
    },
    #[error("Couldn't prepare Markdown leaf {leaf}")]
    Normalize {
        leaf: String,
        #[source]
        source: super::MarkdownIndexError,
    },
    #[error(
        "Markdown leaf {leaf} needs {bytes} bytes of syntax, beyond its budget; open the full source"
    )]
    SyntaxBudget { leaf: String, bytes: usize },
    #[error(
        "Markdown preview of leaf {leaf} needs {bytes} bytes, beyond its presentation-memory budget; open the full source"
    )]
    PresentationBudget { leaf: String, bytes: usize },
    #[error("Markdown preparation worker stopped before it finished leaf {leaf}")]
    WorkerStopped {
        leaf: String,
        #[source]
        source: tokio::sync::oneshot::error::RecvError,
    },
}

fn described(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut cause = error.source();
    while let Some(source) = cause {
        text.push_str(": ");
        text.push_str(&source.to_string());
        cause = source.source();
    }
    text
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
    WaitingWorker(PreparationInput),
    Working,
    Ready(Rc<StoredLeaf>),
    RemarkWaiting {
        shown: Rc<StoredLeaf>,
        input: PreparationInput,
        decided: Arc<artisan_mentions::MessageVocabulary>,
    },
    Remarking(Rc<StoredLeaf>),
    RemarkBlocked {
        shown: Rc<StoredLeaf>,
        needed: usize,
    },
    Failed(String),
}

impl LeafPhase {
    const fn settled(&self) -> bool {
        match self {
            Self::Ready(_)
            | Self::RemarkWaiting { .. }
            | Self::Remarking(_)
            | Self::RemarkBlocked { .. }
            | Self::Failed(_) => true,
            Self::Metadata(_) | Self::Source { .. } | Self::WaitingWorker(_) | Self::Working => {
                false
            }
        }
    }

    const fn advances(&self) -> bool {
        match self {
            Self::Metadata(_)
            | Self::Source { .. }
            | Self::WaitingWorker(_)
            | Self::RemarkWaiting { .. } => true,
            Self::Working
            | Self::Ready(_)
            | Self::Remarking(_)
            | Self::RemarkBlocked { .. }
            | Self::Failed(_) => false,
        }
    }

    const fn shown(&self) -> Option<&Rc<StoredLeaf>> {
        match self {
            Self::Ready(leaf)
            | Self::RemarkWaiting { shown: leaf, .. }
            | Self::Remarking(leaf)
            | Self::RemarkBlocked { shown: leaf, .. } => Some(leaf),
            Self::Metadata(_)
            | Self::Source { .. }
            | Self::WaitingWorker(_)
            | Self::Working
            | Self::Failed(_) => None,
        }
    }
}

struct LeafEntry {
    key: String,
    kind: LeafKind,
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
        vocabulary: Option<Arc<artisan_mentions::MessageVocabulary>>,
        result: Result<Box<PreparedParts>, Box<FailedPreparation>>,
    },
}

struct FailedPreparation {
    error: PrepareLeafError,
    retry: Option<PreparationInput>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoredUpdate {
    Index { owner: String, revision: u64 },
    Leaf { key: String, ordinal: usize },
    Remarked { key: String, ordinal: usize },
}

type WorkerEngine = Result<MarkdownEngine, Arc<super::MarkdownError>>;
type WorkerJob = Box<dyn FnOnce(&WorkerEngine) + Send>;

fn preparation_worker() -> Result<tokio::sync::mpsc::Sender<WorkerJob>, String> {
    let (sender, mut receiver) = tokio::sync::mpsc::channel::<WorkerJob>(QUEUE_DEPTH);
    std::thread::Builder::new()
        .name("artisan-markdown-stored".to_owned())
        .spawn(move || {
            let engine = MarkdownEngine::new().map_err(Arc::new);
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
    vocabulary: Arc<artisan_mentions::MessageVocabulary>,
    budget: usize,
    syntax_budget: usize,
}

impl std::fmt::Debug for StoredOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredOwner").finish_non_exhaustive()
    }
}

impl Default for StoredOwner {
    fn default() -> Self {
        Self::with_budgets(PREPARED_BYTES, PREPARED_BYTES / 8)
    }
}

impl StoredOwner {
    #[must_use]
    fn with_budgets(budget: usize, syntax_budget: usize) -> Self {
        Self {
            indices: VecDeque::new(),
            pages: VecDeque::new(),
            leaves: VecDeque::new(),
            generation: 0,
            capacity: Arc::new(Semaphore::new(budget)),
            prepared_capacity: Arc::new(Semaphore::new(budget)),
            worker: preparation_worker(),
            delivery: Delivery::new(
                const {
                    NonZeroUsize::new(MAX_PENDING_JOBS)
                        .expect("invariant: MAX_PENDING_JOBS is nonzero")
                },
            ),
            vocabulary: Arc::default(),
            budget,
            syntax_budget,
        }
    }
}

fn refuse(entry: &mut LeafEntry, error: &str) -> bool {
    if let LeafPhase::RemarkWaiting { shown, .. } = &entry.phase {
        tracing::warn!(
            leaf = entry.key,
            error,
            "mark a stored message leaf again; keeping the shown leaf"
        );
        entry.phase = LeafPhase::Ready(Rc::clone(shown));
        false
    } else {
        entry.phase = LeafPhase::Failed(error.to_owned());
        entry.capacity = None;
        true
    }
}

fn leaf_ordinal(key: &str, ordinal: u64) -> usize {
    usize::try_from(ordinal)
        .unwrap_or_else(|_| panic!("invariant: leaf {key} has an ordinal that fits its index"))
}

fn requeued(
    key: &str,
    shown: Option<Rc<StoredLeaf>>,
    input: PreparationInput,
    vocabulary: &Arc<artisan_mentions::MessageVocabulary>,
) -> LeafPhase {
    let Some(shown) = shown else {
        return LeafPhase::WaitingWorker(input);
    };
    match shown.revision(key, vocabulary) {
        Revision::Kept => LeafPhase::Ready(shown),
        Revision::Remark(input) => LeafPhase::RemarkWaiting {
            shown,
            input,
            decided: Arc::clone(vocabulary),
        },
    }
}

fn leaf_offset(leaf: &str, offset: u64) -> Result<usize, PrepareLeafError> {
    usize::try_from(offset).map_err(|source| PrepareLeafError::Offset {
        leaf: leaf.to_owned(),
        offset,
        source,
    })
}

struct LeafJob<'a> {
    key: &'a str,
    descriptor: &'a SourceLeafDescriptor,
    vocabulary: Option<&'a Arc<artisan_mentions::MessageVocabulary>>,
    syntax_budget: usize,
}

fn prepare_parts(
    job: &LeafJob<'_>,
    input: &PreparationInput,
    engine: &WorkerEngine,
) -> Result<Box<PreparedParts>, PrepareLeafError> {
    let leaf = || job.key.to_owned();
    let (record, record_bytes, source) = match input {
        PreparationInput::Fetched { metadata, source } => (
            serde_json::from_str::<artisan_markdown_index::LeafRecord>(metadata).map_err(
                |source| PrepareLeafError::Metadata {
                    leaf: leaf(),
                    source,
                },
            )?,
            metadata.len(),
            source.as_str(),
        ),
        PreparationInput::Retained {
            record,
            record_bytes,
            source,
        } => ((**record).clone(), *record_bytes, &**source),
    };
    let start = leaf_offset(job.key, job.descriptor.source_start)?;
    let end = leaf_offset(job.key, job.descriptor.source_end)?;
    if record.leaf.source_range() != (start..end) {
        return Err(PrepareLeafError::Descriptor { leaf: leaf() });
    }
    let engine = engine.as_ref().map_err(|source| PrepareLeafError::Engine {
        leaf: leaf(),
        source: Arc::clone(source),
    })?;
    let normalized = |source: super::MarkdownIndexError| PrepareLeafError::Normalize {
        leaf: leaf(),
        source,
    };
    let (content, resolved) = match job.vocabulary {
        Some(vocabulary) => {
            let message = super::source_index::normalize_message_record(
                &record,
                start,
                source,
                engine,
                &mut super::NoFenceTokenMemo,
                vocabulary,
            )
            .map_err(normalized)?;
            (
                message.content,
                Some((Arc::clone(vocabulary), message.mentions)),
            )
        }
        None => (
            normalize_record(&record, start, source, engine, &mut super::NoFenceTokenMemo)
                .map_err(normalized)?,
            None,
        ),
    };
    let syntax_bytes = content.resident_bytes();
    if syntax_bytes > job.syntax_budget {
        return Err(PrepareLeafError::SyntaxBudget {
            leaf: leaf(),
            bytes: syntax_bytes,
        });
    }
    let leaf_context = record.context.clone();
    let table_continues = record.table_continues;
    let context_bytes = leaf_context.lists.capacity()
        * size_of::<artisan_markdown_index::ListLevel>()
        + leaf_context.table_alignments.capacity()
            * size_of::<artisan_markdown_index::TableAlignment>()
        + leaf_context.spacing.capacity() * size_of::<artisan_markdown_index::LeafSpacing>();
    let (mentions, record_bytes) = match resolved {
        Some((vocabulary, applied)) => (
            LeafMentions::Message(Box::new(MessageMentions {
                resolved_with: std::cell::RefCell::new(vocabulary),
                record,
                record_bytes,
                start,
                applied,
            })),
            record_bytes,
        ),
        None => (LeafMentions::Plain, 0),
    };
    let bytes = syntax_bytes
        .saturating_add(context_bytes)
        .saturating_add(record_bytes)
        .saturating_add(source.len())
        .saturating_add(size_of::<StoredLeaf>());
    Ok(Box::new(PreparedParts {
        context: leaf_context,
        table_continues,
        content,
        mentions,
        source: match input {
            PreparationInput::Fetched { source, .. } => Arc::from(source.as_str()),
            PreparationInput::Retained { source, .. } => Arc::clone(source),
        },
        bytes,
    }))
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
                    sender
                        .offer(Completion::Index {
                            owner,
                            revision,
                            offset,
                            result,
                        })
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
                    && entry.phase.shown().is_some()
            })
    }
    pub fn leaf(&mut self, key: &str, source: &StoredSource, ordinal: usize) -> LeafPreparation {
        self.prepare_leaf(key, source, ordinal, LeafKind::Plain)
    }

    pub fn message_leaf(
        &mut self,
        key: &str,
        source: &StoredSource,
        ordinal: usize,
        vocabulary: &Arc<artisan_mentions::MessageVocabulary>,
    ) -> LeafPreparation {
        self.revise_mentions(vocabulary);
        self.prepare_leaf(key, source, ordinal, LeafKind::Message)
    }

    pub fn revise_mentions(&mut self, vocabulary: &Arc<artisan_mentions::MessageVocabulary>) {
        if Arc::ptr_eq(&self.vocabulary, vocabulary) {
            return;
        }
        self.vocabulary = Arc::clone(vocabulary);
        let mut remarked = Vec::new();
        for entry in &mut self.leaves {
            let shown = match &entry.phase {
                LeafPhase::Ready(shown) | LeafPhase::RemarkBlocked { shown, .. } => {
                    Rc::clone(shown)
                }
                LeafPhase::Metadata(_)
                | LeafPhase::Source { .. }
                | LeafPhase::WaitingWorker(_)
                | LeafPhase::Working
                | LeafPhase::RemarkWaiting { .. }
                | LeafPhase::Remarking(_)
                | LeafPhase::Failed(_) => continue,
            };
            entry.phase = match shown.revision(&entry.key, vocabulary) {
                Revision::Kept => LeafPhase::Ready(shown),
                Revision::Remark(input) => {
                    remarked.push(entry.key.clone());
                    LeafPhase::RemarkWaiting {
                        shown,
                        input,
                        decided: Arc::clone(vocabulary),
                    }
                }
            };
        }
        for key in remarked {
            self.advance_leaf(&key);
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one leaf lookup walking the source, page and preparation states of a stored leaf"
    )]
    fn prepare_leaf(
        &mut self,
        key: &str,
        source: &StoredSource,
        ordinal: usize,
        kind: LeafKind,
    ) -> LeafPreparation {
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
                    && entry.kind == kind
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
                    LeafPhase::Ready(leaf)
                    | LeafPhase::RemarkWaiting { shown: leaf, .. }
                    | LeafPhase::Remarking(leaf)
                    | LeafPhase::RemarkBlocked { shown: leaf, .. } => {
                        LeafPreparation::Ready(Rc::clone(leaf))
                    }
                    LeafPhase::Failed(error) => LeafPreparation::Failed(error.clone()),
                    LeafPhase::Metadata(_)
                    | LeafPhase::Source { .. }
                    | LeafPhase::WaitingWorker(_)
                    | LeafPhase::Working => LeafPreparation::Pending,
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
        if bytes > self.budget as u64 {
            return LeafPreparation::Failed(
                "Markdown source exceeds the preparation ownership budget; open the full source"
                    .to_owned(),
            );
        }
        if self
            .leaves
            .iter()
            .filter(|entry| !entry.phase.settled())
            .count()
            >= MAX_PENDING_JOBS
        {
            return LeafPreparation::Pending;
        }
        while self.leaves.len() >= MAX_PREPARED_ENTRIES {
            let Some(index) = self.leaves.iter().position(|entry| entry.phase.settled()) else {
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
            let Some(index) = self.leaves.iter().position(|entry| entry.phase.settled()) else {
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
            kind,
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
    #[expect(
        clippy::too_many_lines,
        reason = "one step of the stored leaf state machine, issuing the next page or parse request"
    )]
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
            LeafPhase::WaitingWorker(_)
            | LeafPhase::Working
            | LeafPhase::Ready(_)
            | LeafPhase::RemarkWaiting { .. }
            | LeafPhase::Remarking(_)
            | LeafPhase::RemarkBlocked { .. }
            | LeafPhase::Failed(_) => None,
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
                        sender
                            .offer(Completion::Range {
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
        if !matches!(
            entry.phase,
            LeafPhase::WaitingWorker(_) | LeafPhase::RemarkWaiting { .. }
        ) {
            return false;
        }
        let queue = match &self.worker {
            Ok(queue) => queue.clone(),
            Err(error) => return refuse(entry, error),
        };
        let slot = match queue.try_reserve() {
            Ok(slot) => slot,
            Err(tokio::sync::mpsc::error::TrySendError::Full(())) => return false,
            Err(tokio::sync::mpsc::error::TrySendError::Closed(())) => {
                return refuse(entry, "Markdown preparation worker stopped");
            }
        };
        let (input, capacity) = match std::mem::replace(&mut entry.phase, LeafPhase::Working) {
            LeafPhase::WaitingWorker(input) => (
                input,
                Some(Arc::clone(entry.capacity.as_ref().unwrap_or_else(|| {
                    panic!("invariant: leaf {key} holds capacity while it waits for the worker")
                }))),
            ),
            LeafPhase::RemarkWaiting {
                shown,
                input,
                decided,
            } => {
                let input = if Arc::ptr_eq(&decided, &self.vocabulary) {
                    input
                } else {
                    match shown.revision(key, &self.vocabulary) {
                        Revision::Kept => {
                            entry.phase = LeafPhase::Ready(shown);
                            return false;
                        }
                        Revision::Remark(input) => input,
                    }
                };
                entry.phase = LeafPhase::Remarking(shown);
                (input, None)
            }
            LeafPhase::Metadata(_)
            | LeafPhase::Source { .. }
            | LeafPhase::Working
            | LeafPhase::Ready(_)
            | LeafPhase::Remarking(_)
            | LeafPhase::RemarkBlocked { .. }
            | LeafPhase::Failed(_) => {
                unreachable!("invariant: leaf {key} reserved the worker while it waits for it")
            }
        };
        let descriptor = entry.descriptor.clone();
        let vocabulary = match entry.kind {
            LeafKind::Plain => None,
            LeafKind::Message => Some(Arc::clone(&self.vocabulary)),
        };
        let syntax_budget = self.syntax_budget;
        let generation = entry.generation;
        let key = key.to_owned();
        let sender = self.delivery.sender();
        let (reply, receiver) = tokio::sync::oneshot::channel();
        let job_key = key.clone();
        let attempted = vocabulary.clone();
        let job: WorkerJob = Box::new(move |engine| {
            if reply.is_closed() {
                return;
            }
            let job = LeafJob {
                key: &job_key,
                descriptor: &descriptor,
                vocabulary: vocabulary.as_ref(),
                syntax_budget,
            };
            let result = prepare_parts(&job, &input, engine).map_err(|error| {
                Box::new(FailedPreparation {
                    error,
                    retry: Some(input),
                })
            });
            match reply.send((result, capacity)) {
                Ok(()) | Err((_, _)) => {}
            }
        });
        slot.send(job);
        entry.task = Some(
            runtime::runtime()
                .spawn(async move {
                    let (result, capacity) = match receiver.await {
                        Ok((result, capacity)) => (result, capacity),
                        Err(source) => (
                            Err(Box::new(FailedPreparation {
                                error: PrepareLeafError::WorkerStopped {
                                    leaf: key.clone(),
                                    source,
                                },
                                retry: None,
                            })),
                            None,
                        ),
                    };
                    sender
                        .offer(Completion::Prepared {
                            key,
                            generation,
                            result,
                            vocabulary: attempted,
                            _worker: None,
                            _capacity: capacity,
                        })
                        .await;
                })
                .abort_handle(),
        );
        false
    }

    fn admit(&mut self, key: &str, bytes: usize) -> Option<OwnedSemaphorePermit> {
        let count = u32::try_from(bytes).unwrap_or_else(|_| {
            panic!("invariant: leaf {key} prepares {bytes} bytes, bounded by its budget")
        });
        loop {
            if let Ok(permit) = Arc::clone(&self.prepared_capacity).try_acquire_many_owned(count) {
                return Some(permit);
            }
            let oldest = self
                .leaves
                .iter()
                .position(|entry| entry.key != key && entry.phase.settled())?;
            self.leaves.remove(oldest);
        }
    }

    fn remark_permit(
        &mut self,
        key: &str,
        shown: &StoredLeaf,
        bytes: usize,
    ) -> Option<OwnedSemaphorePermit> {
        let held = shown.capacity.take();
        let count = held.as_ref().map_or(0, OwnedSemaphorePermit::num_permits);
        if let Some(mut permit) = held {
            if count >= bytes {
                drop(permit.split(count - bytes));
                return Some(permit);
            }
            let Some(extra) = self.admit(key, bytes - count) else {
                shown.capacity.restore(Some(permit));
                return None;
            };
            permit.merge(extra);
            return Some(permit);
        }
        self.admit(key, bytes)
    }

    fn unblock_remarks(&mut self) {
        let available = self.prepared_capacity.available_permits();
        for entry in &mut self.leaves {
            let LeafPhase::RemarkBlocked { shown, needed } = &entry.phase else {
                continue;
            };
            let held = shown.capacity.take();
            let count = held.as_ref().map_or(0, OwnedSemaphorePermit::num_permits);
            shown.capacity.restore(held);
            if available + count < *needed {
                continue;
            }
            let shown = Rc::clone(shown);
            entry.phase = match shown.revision(&entry.key, &self.vocabulary) {
                Revision::Kept => LeafPhase::Ready(shown),
                Revision::Remark(input) => LeafPhase::RemarkWaiting {
                    shown,
                    input,
                    decided: Arc::clone(&self.vocabulary),
                },
            };
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one publication walking failure, obsolete marks, admission and the new leaf"
    )]
    fn publish(
        &mut self,
        key: String,
        generation: u64,
        attempted: Option<&Arc<artisan_mentions::MessageVocabulary>>,
        result: Result<Box<PreparedParts>, Box<FailedPreparation>>,
    ) -> Option<StoredUpdate> {
        let index = self.leaves.iter().position(|entry| {
            entry.key == key
                && entry.generation == generation
                && entry.source.provider.is_available()
        })?;
        let entry = &mut self.leaves[index];
        entry.task = None;
        let ordinal = leaf_ordinal(&key, entry.descriptor.ordinal);
        let shown = match &entry.phase {
            LeafPhase::Remarking(shown) => Some(Rc::clone(shown)),
            LeafPhase::Working => None,
            LeafPhase::Metadata(_)
            | LeafPhase::Source { .. }
            | LeafPhase::WaitingWorker(_)
            | LeafPhase::Ready(_)
            | LeafPhase::RemarkWaiting { .. }
            | LeafPhase::RemarkBlocked { .. }
            | LeafPhase::Failed(_) => {
                unreachable!(
                    "invariant: leaf {key} publishes only the preparation it is waiting for"
                )
            }
        };
        let obsolete = attempted.is_some_and(|used| !Arc::ptr_eq(used, &self.vocabulary));
        let parts = match result {
            Ok(parts) => parts,
            Err(failed) => {
                let FailedPreparation { error, retry } = *failed;
                if let Some(input) = retry.filter(|_| obsolete) {
                    entry.phase = requeued(&key, shown, input, &self.vocabulary);
                    return None;
                }
                let described = described(&error);
                tracing::warn!(
                    leaf = key,
                    error = described,
                    "prepare a stored Markdown leaf"
                );
                if let Some(shown) = shown {
                    entry.phase = LeafPhase::Ready(shown);
                    return None;
                }
                entry.phase = LeafPhase::Failed(described);
                entry.capacity = None;
                return Some(StoredUpdate::Leaf { key, ordinal });
            }
        };
        if let LeafMentions::Message(mentions) = &parts.mentions
            && let Revision::Remark(input) =
                mentions.revision(&key, &parts.source, &self.vocabulary)
        {
            entry.phase = requeued(&key, shown, input, &self.vocabulary);
            return None;
        }
        let bytes = parts.bytes;
        let permit = if bytes > self.budget {
            None
        } else {
            match &shown {
                Some(shown) => self.remark_permit(&key, shown, bytes),
                None => self.admit(&key, bytes),
            }
        };
        let index = self
            .leaves
            .iter()
            .position(|entry| entry.key == key && entry.generation == generation)
            .unwrap_or_else(|| panic!("invariant: admission never evicts leaf {key} itself"));
        let entry = &mut self.leaves[index];
        let Some(permit) = permit else {
            let error = PrepareLeafError::PresentationBudget {
                leaf: key.clone(),
                bytes,
            };
            let described = described(&error);
            tracing::warn!(
                leaf = key,
                error = described,
                "admit a stored Markdown leaf"
            );
            return match shown {
                Some(shown) if bytes <= self.budget => {
                    entry.phase = LeafPhase::RemarkBlocked {
                        shown,
                        needed: bytes,
                    };
                    None
                }
                Some(shown) => {
                    entry.phase = LeafPhase::Ready(shown);
                    None
                }
                None => {
                    entry.phase = LeafPhase::Failed(described);
                    entry.capacity = None;
                    Some(StoredUpdate::Leaf { key, ordinal })
                }
            };
        };
        entry.capacity = None;
        entry.phase = LeafPhase::Ready(Rc::new(StoredLeaf {
            descriptor: entry.descriptor.clone(),
            source: parts.source,
            context: parts.context,
            table_continues: parts.table_continues,
            content: parts.content,
            mentions: parts.mentions,
            proof: LeafProof {
                provider: entry.source.provider.clone(),
                owner: entry.source.owner.clone(),
                reference: entry.source.header.source.clone(),
                descriptor: entry.descriptor.clone(),
                generation,
            },
            capacity: PreparedLease {
                permit: std::cell::Cell::new(Some(permit)),
                wake: self.delivery.waker(),
            },
        }));
        Some(match shown {
            Some(_) => StoredUpdate::Remarked { key, ordinal },
            None => StoredUpdate::Leaf { key, ordinal },
        })
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one match applying every kind of completed index, page and leaf delivery"
    )]
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
                            match collector.push(&page) {
                                Err(error) => LeafPhase::Failed(error),
                                Ok(()) if collector.next_request().is_some() => {
                                    LeafPhase::Metadata(collector)
                                }
                                Ok(()) => {
                                    match collector.finish().and_then(|metadata| {
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
                                            Ok(source) => LeafPhase::WaitingWorker(
                                                PreparationInput::Fetched { metadata, source },
                                            ),
                                            Err(error) => LeafPhase::Failed(error),
                                        },
                                        Err(error) => LeafPhase::Failed(error),
                                    }
                                }
                            }
                        }
                        (
                            LeafPhase::Source {
                                metadata,
                                mut collector,
                            },
                            Ok(page),
                        ) => match collector.push(&page) {
                            Err(error) => LeafPhase::Failed(error),
                            Ok(()) if collector.next_request().is_some() => LeafPhase::Source {
                                metadata,
                                collector,
                            },
                            Ok(()) => match collector.finish() {
                                Ok(source) => LeafPhase::WaitingWorker(PreparationInput::Fetched {
                                    metadata,
                                    source,
                                }),
                                Err(error) => LeafPhase::Failed(error),
                            },
                        },
                        (other, Ok(_)) => other,
                    };
                    if matches!(entry.phase, LeafPhase::Failed(_)) {
                        entry.capacity = None;
                        updates.push(StoredUpdate::Leaf {
                            key: key.clone(),
                            ordinal: leaf_ordinal(&key, entry.descriptor.ordinal),
                        });
                    }
                }
                Completion::Prepared {
                    key,
                    generation,
                    result,
                    vocabulary,
                    _worker,
                    _capacity,
                } => updates.extend(self.publish(key, generation, vocabulary.as_ref(), result)),
            }
        }
        self.unblock_remarks();
        let keys: Vec<_> = self
            .leaves
            .iter()
            .filter(|entry| entry.task.is_none() && entry.phase.advances())
            .map(|entry| entry.key.clone())
            .collect();
        for key in keys {
            if self.advance_leaf(&key)
                && let Some(entry) = self.leaves.iter().find(|entry| entry.key == key)
            {
                let ordinal = leaf_ordinal(&key, entry.descriptor.ordinal);
                updates.push(StoredUpdate::Leaf { key, ordinal });
            }
        }
        updates
    }
}

#[cfg(test)]
#[path = "../../../tests/markdown/stored.rs"]
mod tests;
