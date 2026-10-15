//! Bounded renderer ownership of immutable Stored Markdown topology.
//! Forge supplies a bounded descriptor page. Native leaf requests fetch exact
//! parser metadata and source bytes only after preparation owns capacity.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;

use artisan_api::{
    ContentRange, ContentRef, SourceIndexFormat, SourceIndexHeader, SourceIndexRange,
    SourceLeafDescriptor,
};
use futures::future::BoxFuture;
use gpui::{App, SharedString, Task};

const MAX_ENTRIES: usize = 48;
const MAX_SOURCE_BYTES: usize = 64 * 1024 * 1024;
const RANGE_BYTES: u32 = 65536;
pub type SourceIndexFetch = Arc<
    dyn Fn(
            ContentRef,
            SourceIndexFormat,
            Option<u64>,
            u32,
        ) -> BoxFuture<'static, Result<SourceIndexRange, String>>
        + Send
        + Sync,
>;
#[derive(Debug)]
pub(super) struct SparseIndex {
    pub header: SourceIndexHeader,
    pub page: SourceIndexRange,
}

pub type ContentFetch = Arc<
    dyn Fn(ContentRef, u64, u32) -> BoxFuture<'static, Result<ContentRange, String>> + Send + Sync,
>;

/// Readiness of topology alone never establishes transcript read evidence.
#[derive(Clone, Debug)]
pub enum StoredPreparation {
    Pending,
    Ready(StoredSource),
    Failed(String),
}
#[derive(Clone)]
pub struct StoredSource {
    /// The displayed page owns its bounded descriptors independently of cache eviction.
    pub(super) index: Arc<SparseIndex>,
    pub(super) reference: ContentRef,
    pub(super) format: SourceIndexFormat,
    pub(super) fetcher: ContentFetch,
    revision: u64,
    blocks: usize,
    pub(super) index_fetcher: SourceIndexFetch,
}
impl std::fmt::Debug for StoredSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredSource")
            .field("reference", &self.reference)
            .field("revision", &self.revision)
            .field("blocks", &self.blocks)
            .finish()
    }
}
impl StoredSource {
    pub fn format(&self) -> SourceIndexFormat {
        self.format
    }
    pub fn block_count(&self) -> usize {
        self.blocks
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn source_ref(&self) -> &ContentRef {
        &self.reference
    }
    pub fn loaded_range(&self) -> std::ops::Range<usize> {
        self.index.page.start as usize..self.index.page.end as usize
    }
    pub fn leaf_descriptor(&self, ordinal: usize) -> Option<SourceLeafDescriptor> {
        let index = &self.index;
        index
            .page
            .leaves
            .get(ordinal.checked_sub(index.page.start as usize)?)
            .cloned()
    }
}

struct Entry {
    reference: ContentRef,
    format: SourceIndexFormat,
    revision: u64,
    offset: Option<u64>,
    index: Option<Arc<SparseIndex>>,
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
    revision: u64,
    waiters: Vec<Waiter>,
}
impl Cache {
    fn trim(&mut self) {
        // Every validated page owns at most 64 fixed-size descriptors.
        // The entry cap therefore bounds descriptor memory directly.
        while self.entries.len() > MAX_ENTRIES {
            let Some(index) = self.entries.iter().position(|entry| entry.task.is_none()) else {
                break;
            };
            self.entries.remove(index);
        }
    }
}
#[derive(Default)]
pub(super) struct StoredOwner {
    cache: Rc<RefCell<Cache>>,
}
impl std::fmt::Debug for StoredOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredOwner")
            .field("entries", &self.cache.borrow().entries.len())
            .finish()
    }
}
impl StoredOwner {
    pub(super) fn clear(&self) {
        let mut cache = self.cache.borrow_mut();
        cache.entries.clear();
        cache.waiters.clear();
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "one preparation request: its key, reference, format, fetchers, page and the callback"
    )]
    pub(super) fn prepare(
        &self,
        key: SharedString,
        reference: ContentRef,
        format: SourceIndexFormat,
        fetcher: ContentFetch,
        index_fetcher: SourceIndexFetch,
        offset: Option<u64>,
        cx: &mut App,
        notify: impl Fn(&mut App) + 'static,
    ) -> StoredPreparation {
        if reference.bytes > MAX_SOURCE_BYTES as u64 {
            return StoredPreparation::Failed(
                "Markdown source exceeds the 64 MiB source admission limit; export the full source"
                    .into(),
            );
        }
        let mut cache = self.cache.borrow_mut();
        if let Some(position) = cache.entries.iter().rposition(|entry| {
            entry.reference == reference
                && entry.format == format
                && match offset {
                    None => entry
                        .index
                        .as_ref()
                        .map_or(entry.offset.is_none(), |index| {
                            index.page.end == index.header.leaves
                        }),
                    Some(offset) => entry
                        .index
                        .as_ref()
                        .map_or(entry.offset == Some(offset), |index| {
                            index.page.start == offset
                        }),
                }
        }) {
            let entry = cache.entries.remove(position).unwrap();
            let result = if let Some(index) = &entry.index {
                StoredPreparation::Ready(StoredSource {
                    index: index.clone(),
                    reference: reference.clone(),
                    format,
                    fetcher,
                    revision: entry.revision,
                    blocks: index.header.leaves as usize,
                    index_fetcher,
                })
            } else if let Some(error) = &entry.error {
                StoredPreparation::Failed(error.clone())
            } else {
                if let Some(waiter) = cache.waiters.iter_mut().find(|waiter| waiter.key == key) {
                    waiter.notify = Box::new(notify);
                } else if cache.waiters.len() < MAX_ENTRIES {
                    cache.waiters.push(Waiter {
                        key,
                        notify: Box::new(notify),
                    });
                }
                StoredPreparation::Pending
            };
            cache.entries.push_back(entry);
            return result;
        }
        while cache.entries.len() >= MAX_ENTRIES {
            let Some(position) = cache.entries.iter().position(|entry| entry.task.is_none()) else {
                if let Some(waiter) = cache.waiters.iter_mut().find(|waiter| waiter.key == key) {
                    waiter.notify = Box::new(notify);
                } else if cache.waiters.len() < MAX_ENTRIES {
                    cache.waiters.push(Waiter {
                        key,
                        notify: Box::new(notify),
                    });
                }
                return StoredPreparation::Pending;
            };
            cache.entries.remove(position);
        }
        while cache
            .entries
            .iter()
            .filter(|entry| entry.reference == reference && entry.format == format)
            .count()
            >= 2
        {
            let Some(position) = cache
                .entries
                .iter()
                .position(|entry| entry.reference == reference && entry.format == format)
            else {
                break;
            };
            // Dropping the oldest page owns cancellation; a displaced response
            // cannot publish because its revision no longer exists.
            cache.entries.remove(position);
        }
        cache.revision = cache.revision.wrapping_add(1);
        let revision = cache.revision;
        let reference_job = reference.clone();
        let owner = Rc::downgrade(&self.cache);
        let task = cx.spawn(async move |cx| {
            let result = index_fetcher(
                reference_job.clone(),
                format,
                offset,
                artisan_api::SOURCE_INDEX_PAGE_LEAVES,
            )
            .await
            .and_then(|page| {
                validate_page(&reference_job, format, offset, &page)?;
                Ok(page)
            })
            .map(|page| {
                Arc::new(SparseIndex {
                    header: page.index.clone(),
                    page,
                })
            });
            cx.update(|cx| {
                let Some(owner) = owner.upgrade() else {
                    return;
                };
                let mut cache = owner.borrow_mut();
                let Some(entry) = cache.entries.iter_mut().find(|entry| {
                    entry.reference == reference_job
                        && entry.format == format
                        && entry.revision == revision
                }) else {
                    return;
                };
                entry.task = None;
                match result {
                    Ok(index) => entry.index = Some(index),
                    Err(error) => entry.error = Some(error),
                }
                cache.trim();
                let waiters = std::mem::take(&mut cache.waiters);
                drop(cache);
                notify(cx);
                for waiter in waiters {
                    (waiter.notify)(cx);
                }
            });
        });
        cache.entries.push_back(Entry {
            reference,
            format,
            revision,
            offset,
            index: None,
            error: None,
            task: Some(task),
        });
        StoredPreparation::Pending
    }
}

fn validate_page(
    reference: &ContentRef,
    format: SourceIndexFormat,
    offset: Option<u64>,
    page: &SourceIndexRange,
) -> Result<(), String> {
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
        return Err("Markdown descriptor page does not match its immutable source".into());
    }
    for (position, leaf) in page.leaves.iter().enumerate() {
        if leaf.ordinal != page.start + position as u64
            || leaf.source_start > leaf.source_end
            || leaf.source_end > reference.bytes
            || leaf.metadata_start >= leaf.metadata_end
            || leaf.metadata_end > page.index.metadata.bytes
        {
            return Err("Markdown descriptor does not match its source or metadata".into());
        }
    }
    Ok(())
}

/// Collect exactly the requested immutable range, stripping UTF-8 overlap by
/// actual server bounds. No caller guesses page advances or parses a prefix.
pub(super) async fn fetch_complete_range(
    reference: &ContentRef,
    range: std::ops::Range<usize>,
    fetcher: &ContentFetch,
) -> Result<String, String> {
    if range.start > range.end
        || range.end > reference.bytes as usize
        || range.len() > MAX_SOURCE_BYTES
    {
        return Err("Markdown source range exceeds its admission limit".into());
    }
    let mut output = String::with_capacity(range.len());
    let mut cursor = range.start as u64;
    while cursor < range.end as u64 {
        let limit = RANGE_BYTES.min((range.end as u64 - cursor).max(4) as u32);
        let page = fetcher(reference.clone(), cursor, limit).await?;
        if page.id != reference.id
            || page.total != reference.bytes
            || page.start > cursor
            || page.end <= cursor
            || page.end > reference.bytes
            || page.end - page.start != page.text.len() as u64
            || page.text.len() > RANGE_BYTES as usize + 3
        {
            return Err("Markdown source response does not match its immutable range".into());
        }
        let start = (cursor - page.start) as usize;
        let end = (page.end.min(range.end as u64) - page.start) as usize;
        let fragment = page
            .text
            .get(start..end)
            .ok_or("Markdown source response splits a UTF-8 boundary")?;
        output.push_str(fragment);
        cursor = page.end.min(range.end as u64);
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn range_collection_preserves_utf8_aligned_overlaps_and_exact_leaf_edges() {
        let text = Arc::new(format!("prefix {} suffix", "β🙂&".repeat(1000)));
        let reference = ContentRef::from_text(&text);
        let range = 7..text.len() - 7;
        let source = text.clone();
        let fetcher: ContentFetch = Arc::new(move |reference, offset, _| {
            let source = source.clone();
            Box::pin(async move {
                let mut start = offset as usize;
                while !source.is_char_boundary(start) {
                    start -= 1;
                }
                let mut end = (start + 39).min(source.len());
                while !source.is_char_boundary(end) {
                    end -= 1;
                }
                Ok(ContentRange {
                    id: reference.id,
                    start: start as u64,
                    end: end as u64,
                    total: source.len() as u64,
                    text: source[start..end].to_owned(),
                })
            })
        });
        assert_eq!(
            futures::executor::block_on(fetch_complete_range(&reference, range.clone(), &fetcher))
                .unwrap(),
            text[range]
        );
    }
}

#[cfg(test)]
mod sparse_tests {
    use super::*;
    fn page(source: &ContentRef, offset: u64, total: u64) -> SourceIndexRange {
        let metadata = ContentRef::from_text("metadata");
        SourceIndexRange {
            index: SourceIndexHeader {
                format: SourceIndexFormat::Markdown,
                source: source.clone(),
                metadata,
                version: artisan_api::SOURCE_INDEX_VERSION,
                leaves: total,
            },
            start: offset,
            end: (offset + 64).min(total),
            leaves: (offset..(offset + 64).min(total))
                .map(|ordinal| SourceLeafDescriptor {
                    ordinal,
                    source_start: 0,
                    source_end: 1,
                    metadata_start: 0,
                    metadata_end: 1,
                })
                .collect(),
        }
    }
    #[gpui::test]
    fn sparse_owner_never_fetches_full_source_and_keeps_the_shown_page(
        cx: &mut gpui::TestAppContext,
    ) {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let reference = ContentRef::from_text(&"x".repeat(20 * 1024 * 1024));
        let raw: ContentFetch =
            Arc::new(|_, _, _| Box::pin(async { panic!("topology must not fetch raw source") }));
        let requests = Arc::new(AtomicUsize::new(0));
        let observed = requests.clone();
        let index: SourceIndexFetch = Arc::new(move |source, format, offset, limit| {
            assert_eq!(format, SourceIndexFormat::Markdown);
            assert_eq!(limit, 64);
            observed.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move { Ok(page(&source, offset.unwrap_or(448), 512)) })
        });
        let owner = StoredOwner::default();
        let fetch = |cx: &mut gpui::TestAppContext, offset| {
            cx.update(|cx| {
                owner.prepare(
                    "source".into(),
                    reference.clone(),
                    SourceIndexFormat::Markdown,
                    raw.clone(),
                    index.clone(),
                    offset,
                    cx,
                    |_| {},
                )
            })
        };
        assert!(matches!(fetch(cx, None), StoredPreparation::Pending));
        cx.run_until_parked();
        let StoredPreparation::Ready(source) = fetch(cx, None) else {
            panic!("tail topology ready");
        };
        assert_eq!(source.block_count(), 512);
        assert_eq!(source.loaded_range(), 448..512);
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        for start in [416, 384, 352, 320, 288] {
            assert!(matches!(fetch(cx, Some(start)), StoredPreparation::Pending));
            // The same pending offset coalesces without another request.
            assert!(matches!(fetch(cx, Some(start)), StoredPreparation::Pending));
            cx.run_until_parked();
            assert!(matches!(
                fetch(cx, Some(start)),
                StoredPreparation::Ready(_)
            ));
        }
        assert_eq!(requests.load(Ordering::SeqCst), 6);
        assert_eq!(
            source.loaded_range(),
            448..512,
            "cache eviction must not erase the page still owned by the displayed source"
        );
        assert_eq!(source.leaf_descriptor(448).unwrap().ordinal, 448);
        let page_lifetime = Arc::downgrade(&source.index);
        assert!(
            matches!(fetch(cx, None), StoredPreparation::Pending),
            "latest must not reuse an arbitrary older page"
        );
        cx.run_until_parked();
        let StoredPreparation::Ready(tail) = fetch(cx, None) else {
            panic!("new latest page")
        };
        assert_eq!(tail.loaded_range(), 448..512);
        assert_eq!(requests.load(Ordering::SeqCst), 7);
        owner.clear();
        assert_eq!(source.loaded_range().len(), 64);
        drop(source);
        assert!(
            page_lifetime.upgrade().is_none(),
            "releasing the displayed page releases its evicted descriptors"
        );
    }
    #[test]
    fn source_identity_offsets_and_totals_are_validated_before_publication() {
        let reference = ContentRef::from_text("a");
        let mut valid = page(&reference, 448, 512);
        assert!(validate_page(&reference, SourceIndexFormat::Markdown, None, &valid).is_ok());
        assert!(validate_page(&reference, SourceIndexFormat::Literal, None, &valid).is_err());
        valid.leaves[0].ordinal = 12;
        assert!(validate_page(&reference, SourceIndexFormat::Markdown, None, &valid).is_err());
        valid = page(&reference, 448, 512);
        valid.index.source = ContentRef::from_text("b");
        assert!(validate_page(&reference, SourceIndexFormat::Markdown, None, &valid).is_err());
        valid = page(&reference, 448, 512);
        valid.end -= 1;
        assert!(validate_page(&reference, SourceIndexFormat::Markdown, None, &valid).is_err());
    }
}
