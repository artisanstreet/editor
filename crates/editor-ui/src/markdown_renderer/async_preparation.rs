//! One bounded, renderer-owned Markdown worker and Item revision cache.
//! Parsing/highlighting run on the worker; only presentation construction
//! and publication run on the UI thread.

use ropey::Rope;
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::{Arc, Weak};
use tokio::sync::mpsc;

use futures::channel::oneshot;
use gpui::{App, SharedString, Task};

use super::PreparedMarkdown;
use super::prepared::PreparedParts;
use super::stored::{ContentFetch, fetch_complete_range};
use crate::fence_tokens::FenceTokenCache;
use crate::markdown::{IndexedLeafContent, LeafContext};
use crate::markdown::{MAX_MARKDOWN_SOURCE_BYTES, MarkdownDocument, MarkdownEngine};
use artisan_api::ContentRef;

// Retain the complete bounded mounted row set, including native overdraw.
const MAX_PREPARED_ENTRIES: usize = 128;
const MAX_PENDING_JOBS: usize = 48;
const MAX_CACHE_BYTES: usize = super::budget::PREPARED_BYTES;
const QUEUE_DEPTH: usize = 8;

/// State of an Item revision's asynchronous preparation.
#[derive(Clone, Debug)]
pub enum MarkdownPreparation {
    Pending,
    Ready(Rc<PreparedSource>),
    Failed(String),
}

/// Immutable UI presentation of one prepared Item revision.
#[derive(Debug)]
pub struct PreparedSource {
    pub(super) presentation: Rc<PreparedMarkdown>,
    revision: u64,
    preview: bool,
}
impl PreparedSource {
    /// Renderer job revision; independent of allocation reuse.
    pub const fn revision(&self) -> u64 {
        self.revision
    }
    pub fn block_count(&self) -> usize {
        self.presentation.block_count()
    }
    pub const fn is_preview(&self) -> bool {
        self.preview
    }
    pub fn resident_bytes(&self) -> usize {
        self.presentation.resident_bytes()
    }
}

/// Static Item text or an immutable, shared live rope revision.
pub(super) enum Source {
    Text(Arc<str>),
    Rope(Arc<Rope>),
    Remote {
        reference: ContentRef,
        metadata: ContentRef,
        descriptor: artisan_api::SourceLeafDescriptor,
        fetcher: ContentFetch,
    },
    RemoteResolved {
        reference: ContentRef,
        descriptor: artisan_api::SourceLeafDescriptor,
        metadata_json: String,
        source: Arc<str>,
    },
}
impl Source {
    fn len(&self) -> usize {
        match self {
            Self::Remote { descriptor, .. } => {
                super::wire_index(
                    descriptor.source_end - descriptor.source_start + descriptor.metadata_end
                        - descriptor.metadata_start,
                ) + 65536
            }
            Self::RemoteResolved {
                descriptor, source, ..
            } => {
                source.len()
                    + super::wire_index(descriptor.metadata_end - descriptor.metadata_start)
                    + 65536
            }
            Self::Text(text) => text.len(),
            Self::Rope(text) => text.len_bytes(),
        }
    }
    fn identity(&self) -> SourceIdentity {
        match self {
            Self::Remote {
                reference,
                descriptor,
                ..
            }
            | Self::RemoteResolved {
                reference,
                descriptor,
                ..
            } => SourceIdentity::Remote(reference.clone(), super::wire_index(descriptor.ordinal)),
            Self::Text(text) => SourceIdentity::Text(Arc::downgrade(text)),
            Self::Rope(text) => SourceIdentity::Rope(Arc::downgrade(text)),
        }
    }
    async fn resolve(self) -> Result<Self, String> {
        match self {
            Self::Remote {
                reference,
                metadata,
                descriptor,
                fetcher,
            } => {
                let json = fetch_complete_range(
                    &metadata,
                    super::wire_index(descriptor.metadata_start)
                        ..super::wire_index(descriptor.metadata_end),
                    &fetcher,
                )
                .await?;
                let source = fetch_complete_range(
                    &reference,
                    super::wire_index(descriptor.source_start)
                        ..super::wire_index(descriptor.source_end),
                    &fetcher,
                )
                .await?;
                Ok(Self::RemoteResolved {
                    reference,
                    descriptor,
                    metadata_json: json,
                    source: Arc::from(source),
                })
            }
            source @ (Self::Text(_) | Self::Rope(_) | Self::RemoteResolved { .. }) => Ok(source),
        }
    }
    fn previewed(&self) -> bool {
        !matches!(self, Self::Remote { .. } | Self::RemoteResolved { .. })
            && self.len() > MAX_MARKDOWN_SOURCE_BYTES
    }
    fn parse(
        &self,
        engine: &MarkdownEngine,
        fences: &mut FenceTokenCache,
    ) -> Result<(MarkdownDocument, Option<IndexedStyle>), String> {
        match self {
            Self::RemoteResolved {
                descriptor,
                metadata_json,
                source,
                ..
            } => {
                let record: artisan_markdown_index::LeafRecord =
                    serde_json::from_str(metadata_json).map_err(|error| error.to_string())?;
                if record.leaf.source_range()
                    != (super::wire_index(descriptor.source_start)
                        ..super::wire_index(descriptor.source_end))
                {
                    return Err("Markdown metadata does not match its descriptor".into());
                }
                let style = Some(IndexedStyle {
                    context: record.context.clone(),
                    table_header: None,
                    table_continues: record.table_continues,
                });
                let content = crate::markdown::normalize_record(
                    &record,
                    super::wire_index(descriptor.source_start),
                    source,
                    engine,
                )
                .map_err(|error| error.to_string())?;
                Ok(Self::document_from_content(content, source, style))
            }
            Self::Remote { .. } => {
                Err("Markdown range was not fetched before worker admission".into())
            }
            Self::Text(_) | Self::Rope(_) => engine
                .parse_document_with_fence_memo(&self.preview(), fences)
                .map(|document| (document, None))
                .map_err(|error| error.to_string()),
        }
    }
    fn document_from_content(
        content: IndexedLeafContent,
        source: &str,
        style: Option<IndexedStyle>,
    ) -> (MarkdownDocument, Option<IndexedStyle>) {
        let mut style = style;
        let block = match content {
            IndexedLeafContent::Block(block) => block,
            IndexedLeafContent::TableRow {
                header,
                alignments,
                cells,
            } => {
                if let Some(style) = style.as_mut() {
                    style.table_header = Some(header);
                }
                crate::markdown::Block::Table(crate::markdown::Table {
                    alignments,
                    header: cells,
                    rows: Vec::new(),
                    range: 0..source.len(),
                })
            }
        };
        (
            MarkdownDocument {
                blocks: vec![block],
            },
            style,
        )
    }
    /// Flatten only the bounded preview on the preparation worker.
    fn preview(&self) -> Cow<'_, str> {
        match self {
            Self::Text(text) => {
                let mut end = text.len().min(MAX_MARKDOWN_SOURCE_BYTES);
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                Cow::Borrowed(&text[..end])
            }
            Self::RemoteResolved { source, .. } => Cow::Borrowed(source),
            #[expect(
                clippy::panic,
                reason = "the worker only receives sources resolved before admission"
            )]
            Self::Remote { .. } => {
                panic!("Stored source must resolve before Markdown worker execution")
            }
            Self::Rope(text) => {
                let end = text.byte_to_char(text.len_bytes().min(MAX_MARKDOWN_SOURCE_BYTES));
                Cow::Owned(text.slice(..end).to_string())
            }
        }
    }
}
enum SourceIdentity {
    Text(Weak<str>),
    Rope(Weak<Rope>),
    Remote(ContentRef, usize),
}
impl SourceIdentity {
    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Remote(a, x), Self::Remote(b, y)) => a == b && x == y,
            (Self::Text(a), Self::Text(b)) => Weak::ptr_eq(a, b),
            (Self::Rope(a), Self::Rope(b)) => Weak::ptr_eq(a, b),
            _ => false,
        }
    }
}

pub(super) struct IndexedStyle {
    pub context: LeafContext,
    pub table_header: Option<bool>,
    pub table_continues: bool,
}
type DocumentResult = Result<(MarkdownDocument, PreparedParts, Option<IndexedStyle>), String>;
pub(super) struct DocumentJob {
    // Capacity belongs to the actual queued/running snapshot, not its view task.
    // Canceling a view cannot release this until the worker drops the job.
    _source_capacity: tokio::sync::OwnedSemaphorePermit,
    source: Source,
    /// A message's leading `/command`, marked once parsed.
    command: Option<String>,
    renderer: gpui::SvgRenderer,
    result: oneshot::Sender<DocumentResult>,
}
/// Shaping a visible paragraph's glyph window: the job and where its result
/// goes.
pub(super) struct ShapeJob {
    pub work: Box<dyn FnOnce() -> super::projection_worker::ShapeResult + Send>,
    pub result: oneshot::Sender<super::projection_worker::ShapeResult>,
}
pub(super) enum Job {
    Document(DocumentJob),
    Shape(ShapeJob),
}
/// The renderer's one preparation thread. Shaping what a row shows goes
/// ahead of every queued parse, so visible text never waits behind
/// preparation of what may never show.
pub(super) struct Worker {
    pub queue: mpsc::Sender<Job>,
    source_capacity: Arc<tokio::sync::Semaphore>,
}
impl Worker {
    #[expect(
        clippy::significant_drop_tightening,
        reason = "the worker thread owns its queues for its whole life"
    )]
    fn new() -> Result<Self, String> {
        let (queue, mut receiver) = mpsc::channel::<Job>(QUEUE_DEPTH);
        std::thread::Builder::new()
            .name("artisan-markdown".into())
            .spawn(move || {
                let engine = MarkdownEngine::new();
                let mut diagrams = super::diagram::DiagramCache::default();
                let mut fences = FenceTokenCache::default();
                let mut documents = VecDeque::new();
                let mut shapes = VecDeque::new();
                loop {
                    if documents.is_empty() && shapes.is_empty() {
                        match receiver.blocking_recv() {
                            Some(job) => queue_job(job, &mut documents, &mut shapes),
                            None => return,
                        }
                    }
                    while let Ok(job) = receiver.try_recv() {
                        queue_job(job, &mut documents, &mut shapes);
                    }
                    if let Some(job) = shapes.pop_front() {
                        let ShapeJob { work, result } = job;
                        if !result.is_canceled() {
                            // The requester may be gone; its shapes are then unwanted.
                            result.send(work()).ok();
                        }
                    } else if let Some(job) = documents.pop_front() {
                        parse(job, engine.as_ref(), &mut diagrams, &mut fences);
                    }
                }
            })
            .map_err(|error| format!("Couldn't start Markdown preparation: {error}"))?;
        Ok(Self {
            queue,
            source_capacity: super::budget::pool(MAX_CACHE_BYTES),
        })
    }
}

fn queue_job(job: Job, documents: &mut VecDeque<DocumentJob>, shapes: &mut VecDeque<ShapeJob>) {
    match job {
        Job::Document(job) => documents.push_back(job),
        Job::Shape(job) => shapes.push_back(job),
    }
}

/// Parses and highlights one document within its memory budgets.
fn parse(
    job: DocumentJob,
    engine: Result<&MarkdownEngine, &crate::markdown::MarkdownError>,
    diagrams: &mut super::diagram::DiagramCache,
    fences: &mut FenceTokenCache,
) {
    if job.result.is_canceled() {
        return;
    }
    let document = engine
        .map_err(ToString::to_string)
        .and_then(|engine| job.source.parse(engine, fences))
        .map(|(mut document, style)| {
            if let Some(command) = &job.command {
                document.mark_command(command);
            }
            if !job.result.is_canceled() {
                diagrams.prepare(&mut document);
            }
            (document, style)
        })
        .and_then(|(document, style)| {
            if document.resident_bytes() > MAX_CACHE_BYTES / 8 {
                return Err(
                    "Markdown leaf exceeds its syntax-memory budget; open the full source"
                        .to_owned(),
                );
            }
            let parts = PreparedParts::new(&document, Some(&job.renderer));
            if parts.resident_bytes() > MAX_CACHE_BYTES / 2 {
                return Err(
                    "Markdown leaf exceeds its normalized-memory budget; open the full source"
                        .to_owned(),
                );
            }
            Ok((document, parts, style))
        });
    // The requester may be gone; its document is then unwanted.
    job.result.send(document).ok();
}

struct Entry {
    key: SharedString,
    source: SourceIdentity,
    generation: u64,
    prepared: Option<Rc<PreparedSource>>,
    error: Option<String>,
    task: Option<Task<()>>,
    pending_bytes: Option<usize>,
}
/// Tells a turned-away request to ask again.
type Waiter = Box<dyn Fn(&mut App)>;

#[derive(Default)]
struct Cache {
    entries: VecDeque<Entry>,
    generation: u64,
    /// Requests the budget turned away, told when any job finishes so they
    /// can ask again.
    waiters: Vec<(SharedString, Waiter)>,
}
impl Cache {
    fn wait(&mut self, key: SharedString, notify: impl Fn(&mut App) + 'static) {
        self.waiters.retain(|(waiting, _)| *waiting != key);
        if self.waiters.len() >= MAX_PENDING_JOBS * 8 {
            drop(self.waiters.remove(0));
        }
        self.waiters.push((key, Box::new(notify)));
    }
    fn can_admit(&self, bytes: usize) -> bool {
        let pending = self.entries.iter().filter_map(|entry| entry.pending_bytes);
        let count = pending.clone().count();
        let used = pending.sum::<usize>();
        count < MAX_PENDING_JOBS && used.saturating_add(bytes) <= MAX_CACHE_BYTES
    }
    fn eviction_index(&self) -> Option<usize> {
        self.entries
            .iter()
            .position(|entry| entry.pending_bytes.is_none())
    }
    fn trim(&mut self, cx: &mut App) {
        while self.entries.len() > MAX_PREPARED_ENTRIES
            || self
                .entries
                .iter()
                .map(|entry| {
                    entry.pending_bytes.unwrap_or(0).saturating_add(
                        entry
                            .prepared
                            .as_ref()
                            .map_or(0, |source| source.resident_bytes()),
                    )
                })
                .sum::<usize>()
                > MAX_CACHE_BYTES
        {
            let Some(index) = self.eviction_index() else {
                // Keep pending work, but its preceding revision is optional
                // while the replacement is being normalized.
                if let Some(source) = self
                    .entries
                    .iter_mut()
                    .find_map(|entry| entry.prepared.take())
                {
                    source.presentation.release_images(cx);
                    continue;
                }
                break;
            };
            if let Some(entry) = self.entries.remove(index)
                && let Some(source) = entry.prepared
            {
                source.presentation.release_images(cx);
            }
        }
    }
}

pub(super) struct PreparationOwner {
    worker: RefCell<Option<Arc<Worker>>>,
    cache: Rc<RefCell<Cache>>,
}
impl std::fmt::Debug for PreparationOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparationOwner")
            .field("entries", &self.cache.borrow().entries.len())
            .finish_non_exhaustive()
    }
}
impl Default for PreparationOwner {
    fn default() -> Self {
        Self {
            worker: RefCell::new(None),
            cache: Rc::new(RefCell::new(Cache::default())),
        }
    }
}
impl PreparationOwner {
    pub(super) fn worker(&self) -> Result<Arc<Worker>, String> {
        let worker = self.worker.borrow().clone();
        if let Some(worker) = worker {
            return Ok(worker);
        }
        let worker = Arc::new(Worker::new()?);
        *self.worker.borrow_mut() = Some(Arc::clone(&worker));
        Ok(worker)
    }
    pub(super) fn is_remote_prepared(
        &self,
        key: &SharedString,
        reference: &ContentRef,
        leaf: usize,
    ) -> bool {
        self.identity_prepared(key, &SourceIdentity::Remote(reference.clone(), leaf))
    }
    pub(super) fn clear(&self, cx: &mut App) {
        self.cache.borrow_mut().waiters.clear();
        for entry in self.cache.borrow_mut().entries.drain(..) {
            if let Some(source) = entry.prepared {
                source.presentation.release_images(cx);
            }
        }
        self.worker.borrow_mut().take();
    }
    pub(super) fn is_prepared(&self, key: &SharedString, source: &Arc<str>) -> bool {
        self.identity_prepared(key, &SourceIdentity::Text(Arc::downgrade(source)))
    }
    pub(super) fn is_rope_prepared(&self, key: &SharedString, source: &Arc<Rope>) -> bool {
        self.identity_prepared(key, &SourceIdentity::Rope(Arc::downgrade(source)))
    }
    fn identity_prepared(&self, key: &SharedString, source: &SourceIdentity) -> bool {
        self.cache.borrow().entries.iter().any(|entry| {
            &entry.key == key
                && entry.source.same(source)
                && entry.pending_bytes.is_none()
                && entry.task.is_none()
                && entry.error.is_none()
                && entry.prepared.is_some()
        })
    }
    pub(super) fn prepared(&self, key: &SharedString) -> Option<Rc<PreparedSource>> {
        self.cache
            .borrow()
            .entries
            .iter()
            .find(|entry| &entry.key == key)
            .and_then(|entry| entry.prepared.clone())
    }
    #[expect(
        clippy::too_many_lines,
        reason = "one sequential pass; splitting it would only thread its locals through helpers"
    )]
    pub(super) fn prepare(
        &self,
        key: SharedString,
        source: Source,
        command: Option<String>,
        cx: &mut App,
        notify: impl Fn(&mut App) + 'static,
    ) -> MarkdownPreparation {
        let worker = match self.worker() {
            Ok(worker) => worker,
            Err(error) => return MarkdownPreparation::Failed(error),
        };
        let source_identity = source.identity();
        let mut cache = self.cache.borrow_mut();
        let old = cache
            .entries
            .iter()
            .position(|entry| entry.key == key)
            .and_then(|index| cache.entries.remove(index));
        let old = match old {
            Some(entry) if entry.source.same(&source_identity) => {
                let result = if entry.task.is_some() {
                    MarkdownPreparation::Pending
                } else {
                    entry.prepared.as_ref().map_or_else(
                        || {
                            entry
                                .error
                                .as_ref()
                                .map_or(MarkdownPreparation::Pending, |error| {
                                    MarkdownPreparation::Failed(error.clone())
                                })
                        },
                        |source| MarkdownPreparation::Ready(Rc::clone(source)),
                    )
                };
                cache.entries.push_back(entry);
                return result;
            }
            old => old,
        };
        // A queued job owns the complete immutable snapshot until completion.
        // Rope snapshots may share chunks; charging all bytes is conservative
        // and prevents repeated resets from pinning unbounded former versions.
        let pending_bytes = source.len();
        if pending_bytes > MAX_CACHE_BYTES {
            if let Some(entry) = old {
                cache.entries.push_back(entry);
            }
            return MarkdownPreparation::Failed(
                "Markdown source exceeds the preparation ownership budget; open the full source"
                    .into(),
            );
        }
        if !cache.can_admit(pending_bytes) {
            // Active requests are never evicted to admit more work; this one
            // asks again when one of them finishes.
            if let Some(entry) = old {
                cache.entries.push_back(entry);
            }
            cache.wait(key, notify);
            return MarkdownPreparation::Pending;
        }
        cache.generation = cache.generation.wrapping_add(1);
        let generation = cache.generation;
        let previous = old.and_then(|entry| entry.prepared);
        let preview = source.previewed();

        let cache_owner = Rc::downgrade(&self.cache);
        let task_key = key.clone();
        let renderer = cx.svg_renderer();
        let task = cx.spawn(async move |cx| {
            // Waiting tasks remain charged in the renderer cache. Admitted
            // jobs have a separate bound, including canceled queue entries.
            let Ok(source_capacity) = Arc::clone(&worker.source_capacity).acquire_many_owned(super::budget::permits(pending_bytes)).await else { return; };
            let document=match source.resolve().await {
                Ok(source)=> {
                    let (result,response)=oneshot::channel();
                    let job=DocumentJob { _source_capacity:source_capacity,source,command,renderer,result };
                    if worker.queue.send(Job::Document(job)).await.is_err() { Err("Markdown worker closed".to_owned()) }
                    else { response.await.unwrap_or_else(|_|Err("Markdown worker stopped before publication".to_owned())) }
                }
                Err(error)=>Err(error),
            };
            cx.update(|cx| {
                let Some(owner) = cache_owner.upgrade() else {
                    return;
                };
                let mut cache = owner.borrow_mut();
                // Whatever became of this job, its budget is free now.
                let waiters = std::mem::take(&mut cache.waiters);
                let Some(entry) = cache
                    .entries
                    .iter_mut()
                    .find(|entry| entry.key == task_key && entry.generation == generation)
                else {
                    drop(cache);
                    for (_, waiter) in waiters {
                        waiter(cx);
                    }
                    return;
                };
                match document {
                    Ok((document, parts, style)) => {
                        let source = Rc::new(PreparedSource {
                            presentation: Rc::new(PreparedMarkdown::from_indexed_parts(&document,parts,style)),revision:generation,preview,
                        });
                        if source.resident_bytes() > MAX_CACHE_BYTES {
                            source.presentation.release_images(cx);
                            if let Some(old) = entry.prepared.take() { old.presentation.release_images(cx); }
                            entry.error = Some("Markdown preview exceeds its presentation-memory budget; open the full source".into());
                        } else {
                            if let Some(old) = entry.prepared.replace(source) { old.presentation.release_images(cx); }
                            entry.error = None;
                        }

                    }
                    Err(error) => entry.error = Some(error),
                }
                entry.task = None;
                entry.pending_bytes = None;
                cache.trim(cx);
                drop(cache);
                notify(cx);
                for (_, waiter) in waiters {
                    waiter(cx);
                }
            });
        });
        cache.entries.push_back(Entry {
            key,
            source: source_identity,
            generation,
            prepared: previous,
            error: None,
            task: Some(task),
            pending_bytes: Some(pending_bytes),
        });
        cache.trim(cx);
        MarkdownPreparation::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rope_preview_is_utf8_bounded_and_identity_tracks_snapshot_not_content() {
        let full = format!("A{}", "🙂".repeat(MAX_MARKDOWN_SOURCE_BYTES / 4 + 2));
        let mut rope = Arc::new(Rope::from_str(&full));
        let old = Arc::clone(&rope);
        let source = Source::Rope(Arc::clone(&old));
        assert_eq!(source.len(), full.len());
        let prefix = source.preview();
        assert!(prefix.len() <= MAX_MARKDOWN_SOURCE_BYTES);
        assert!(prefix.starts_with('A') && prefix.ends_with('🙂'));
        assert!(full.starts_with(prefix.as_ref()));
        let identity = source.identity();
        assert!(identity.same(&Source::Rope(old).identity()));
        let end = rope.len_chars();
        Arc::make_mut(&mut rope).insert(end, "tail");
        assert!(!identity.same(&Source::Rope(rope).identity()));
        assert!(!identity.same(&Source::Text(Arc::from(full)).identity()));
    }
}
