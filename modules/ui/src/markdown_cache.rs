//! Bounded revision-keyed cache for parsed Markdown documents.
//!
//! [`MarkdownRenderer`](crate::markdown_renderer::MarkdownRenderer) runs
//! synchronously inside GPUI's render pass, and a window can hold many
//! assistant replies. Without memoization every frame re-parses every visible
//! body, including `syntect` highlighting for fenced code. This cache keeps
//! parsed [`MarkdownDocument`]s behind a shared pointer keyed by exact body
//! identity, so an unchanged body parses once and repeated frames borrow the
//! same document.
//!
//! Lookup is cheap when the body is unchanged. A caller holding the body as
//! a [`SharedString`] that the cache already holds matches by allocation
//! identity alone: the cache keeps that allocation alive, so equal pointers
//! and lengths prove equal bytes. A borrowed `&str` caller whose buffer and
//! length match its previous hit is confirmed by one byte comparison, with
//! no hashing. Only a body the cache has not seen at that address pays the
//! content hash.
//!
//! Each entry also carries the body's prepared presentation (flattened
//! inline text, theme-independent style runs, fence sources and tokens,
//! element ids), built once when the document enters the cache, so a frame
//! that renders an unchanged body re-flattens and re-highlights nothing.
//! A separate bounded memo keeps fence tokens by language and source, so a
//! streaming body that re-parses on every delta re-classifies only the fence
//! whose source changed.
//!
//! Parse output is theme-independent: code tokens carry semantic kinds, not
//! resolved colors, and inline presentation resolves theme and rich-link
//! titles at render time. The cache key is therefore the body bytes alone;
//! theme or tone changes reuse the same parsed document.
//!
//! Bounds: at most [`MARKDOWN_PARSE_CACHE_MAX_ENTRIES`] documents charged
//! against [`MARKDOWN_PARSE_CACHE_MAX_BYTES`] source bytes. Eviction is
//! least-recently-used; a document larger than the byte budget is parsed but
//! never cached, so one pathological body cannot evict the whole cache.

#![forbid(unsafe_code)]

use std::cell::Cell;
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use gpui::SharedString;

use crate::markdown::{CodeToken, FenceTokenMemo, MarkdownDocument, MarkdownEngine};
use crate::markdown_renderer::PreparedMarkdown;

/// Maximum live parsed documents retained across render passes.
///
/// Above the transcript's built-row budget (48 rows, each holding one reply
/// and its session's commentary), so a window of Markdown bodies always
/// fits: an LRU smaller than the visible set misses on every body every
/// frame, and each miss is a full parse plus fence highlighting.
pub const MARKDOWN_PARSE_CACHE_MAX_ENTRIES: usize = 192;

/// Maximum charged source bytes across live parsed documents.
pub const MARKDOWN_PARSE_CACHE_MAX_BYTES: usize = 8 * 1024 * 1024;

/// Maximum classified fences retained by the fence-token memo.
pub const MARKDOWN_FENCE_CACHE_MAX_ENTRIES: usize = 256;

/// Maximum charged bytes (fence source plus tokens) in the fence-token memo.
pub const MARKDOWN_FENCE_CACHE_MAX_BYTES: usize = 4 * 1024 * 1024;

/// Snapshot of the renderer's Markdown parse-cache counters.
///
/// The report is a review and test seam: `parses` counts bodies that reached
/// the parser (successes and failures), `hits` counts lookups served from the
/// cache, and `entries`/`bytes` describe the live cache after the last
/// lookup. The work counters prove that an unchanged body costs no
/// presentation work: `inline_flattens` counts inline runs flattened into
/// presentation data, `fence_highlights` counts fences classified through
/// `syntect`, and `fence_highlight_hits` counts fences whose tokens the
/// fence memo served during a re-parse.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MarkdownParseReport {
    /// Parse attempts that missed the cache.
    pub parses: u64,
    /// Lookups served from a cached entry.
    pub hits: u64,
    /// Live cache entries after the latest lookup.
    pub entries: usize,
    /// Charged source bytes held by live entries after the latest lookup.
    pub bytes: usize,
    /// Inline runs flattened into presentation data.
    pub inline_flattens: u64,
    /// Fences classified through `syntect`.
    pub fence_highlights: u64,
    /// Fences whose tokens the fence memo served instead of `syntect`.
    pub fence_highlight_hits: u64,
}

/// Presentation work counters shared by the cache and the documents it
/// prepared, so work done lazily at render time still reaches the report.
#[derive(Debug, Default)]
pub(crate) struct MarkdownWorkCounters {
    inline_flattens: Cell<u64>,
}

impl MarkdownWorkCounters {
    /// Records one inline flatten.
    pub(crate) fn record_inline_flatten(&self) {
        self.inline_flattens
            .set(self.inline_flattens.get().saturating_add(1));
    }
}

#[derive(Debug)]
struct CacheEntry {
    hash: u64,
    /// The body bytes. Holding the allocation keeps identity matches sound:
    /// while the entry lives, no other body can occupy its address.
    source: SharedString,
    /// Address and length of the borrowed buffer that last matched this
    /// entry. A hint only: a match is confirmed by comparing bytes, because
    /// the caller's buffer may have been freed and reused since.
    caller_hint: (usize, usize),
    /// `None` is a cached parse failure: the body renders its plain fallback
    /// without paying for the failing parse again.
    prepared: Option<Rc<PreparedMarkdown>>,
}

/// Least-recently-used cache of parsed Markdown documents.
#[derive(Debug, Default)]
pub(crate) struct MarkdownParseCache {
    /// Front is least recently used; hits move their entry to the back.
    entries: VecDeque<CacheEntry>,
    bytes: usize,
    parses: u64,
    hits: u64,
    fences: FenceTokenCache,
    counters: Rc<MarkdownWorkCounters>,
}

impl MarkdownParseCache {
    /// Returns the parsed document for `source`, parsing at most once per
    /// cached body.
    pub(crate) fn document(
        &mut self,
        engine: &MarkdownEngine,
        source: &str,
    ) -> Option<Rc<MarkdownDocument>> {
        self.prepared(engine, source)
            .map(|prepared| Rc::clone(prepared.document()))
    }

    /// Returns the prepared presentation for a borrowed body.
    pub(crate) fn prepared(
        &mut self,
        engine: &MarkdownEngine,
        source: &str,
    ) -> Option<Rc<PreparedMarkdown>> {
        self.lookup(engine, source, None)
    }

    /// Returns the prepared presentation for a shared body, matching by
    /// allocation identity before any byte work.
    pub(crate) fn prepared_shared(
        &mut self,
        engine: &MarkdownEngine,
        source: &SharedString,
    ) -> Option<Rc<PreparedMarkdown>> {
        if let Some(index) = self
            .entries
            .iter()
            .position(|entry| same_allocation(&entry.source, source))
        {
            return self.hit(index, None, None);
        }
        self.lookup(engine, source.as_str(), Some(source))
    }

    fn lookup(
        &mut self,
        engine: &MarkdownEngine,
        source: &str,
        shared: Option<&SharedString>,
    ) -> Option<Rc<PreparedMarkdown>> {
        let hint = caller_hint(source);
        if let Some(index) = self.entries.iter().position(|entry| {
            entry.source.len() == source.len()
                && (entry.source.as_ptr() == source.as_ptr()
                    || (entry.caller_hint == hint && entry.source.as_str() == source))
        }) {
            return self.hit(index, Some(hint), shared);
        }
        let hash = source_hash(source);
        if let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.hash == hash && entry.source.as_str() == source)
        {
            return self.hit(index, Some(hint), shared);
        }
        self.parses = self.parses.saturating_add(1);
        let prepared = engine
            .parse_document_with_fence_memo(source, &mut self.fences)
            .ok()
            .map(|document| {
                Rc::new(PreparedMarkdown::new(
                    Rc::new(document),
                    Rc::clone(&self.counters),
                ))
            });
        let source_bytes = source.len();
        if source_bytes <= MARKDOWN_PARSE_CACHE_MAX_BYTES {
            self.entries.push_back(CacheEntry {
                hash,
                source: shared.map_or_else(|| SharedString::from(source), Clone::clone),
                caller_hint: hint,
                prepared: prepared.clone(),
            });
            self.bytes = self.bytes.saturating_add(source_bytes);
            self.evict();
        }
        prepared
    }

    /// Serves entry `index`, moving it to the most recently used slot.
    ///
    /// A shared caller whose equal body lives in another allocation hands
    /// that allocation to the entry, so its next lookup matches by identity.
    fn hit(
        &mut self,
        index: usize,
        hint: Option<(usize, usize)>,
        shared: Option<&SharedString>,
    ) -> Option<Rc<PreparedMarkdown>> {
        let mut entry = self
            .entries
            .remove(index)
            .expect("a located cache entry must remain present");
        if let Some(hint) = hint {
            entry.caller_hint = hint;
        }
        if let Some(shared) = shared {
            entry.source = shared.clone();
        }
        let prepared = entry.prepared.clone();
        self.entries.push_back(entry);
        self.hits = self.hits.saturating_add(1);
        prepared
    }

    /// Returns the current counters.
    pub(crate) fn report(&self) -> MarkdownParseReport {
        MarkdownParseReport {
            parses: self.parses,
            hits: self.hits,
            entries: self.entries.len(),
            bytes: self.bytes,
            inline_flattens: self.counters.inline_flattens.get(),
            fence_highlights: self.fences.highlights,
            fence_highlight_hits: self.fences.hits,
        }
    }

    /// Drops least-recently-used entries until both bounds hold.
    fn evict(&mut self) {
        while self.entries.len() > MARKDOWN_PARSE_CACHE_MAX_ENTRIES
            || self.bytes > MARKDOWN_PARSE_CACHE_MAX_BYTES
        {
            let Some(entry) = self.entries.pop_front() else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(entry.source.len());
        }
    }
}

/// Whether two shared strings are the same bytes in memory.
///
/// Equal addresses and lengths of two live strings are the same bytes, so
/// no comparison is needed. Short strings live inline in each value and
/// never match, which only costs them the (trivial) byte path.
pub(crate) fn same_allocation(left: &SharedString, right: &SharedString) -> bool {
    left.as_ptr() == right.as_ptr() && left.len() == right.len()
}

fn caller_hint(source: &str) -> (usize, usize) {
    (source.as_ptr() as usize, source.len())
}

/// Stable-within-a-process identity for one body's bytes.
fn source_hash(source: &str) -> u64 {
    let mut hasher = std::hash::DefaultHasher::new();
    source.hash(&mut hasher);
    hasher.finish()
}

#[derive(Debug)]
struct FenceEntry {
    hash: u64,
    language: String,
    source: String,
    tokens: Vec<CodeToken>,
}

impl FenceEntry {
    fn charge(&self) -> usize {
        self.source.len().saturating_add(
            self.tokens
                .len()
                .saturating_mul(std::mem::size_of::<CodeToken>()),
        )
    }
}

/// Bounded least-recently-used memo of classified fence tokens.
///
/// Keyed by language, source hash, and length; a hit is confirmed by
/// comparing the source bytes, which is far cheaper than classifying them
/// and keeps a hash collision from ever serving another fence's tokens.
#[derive(Debug, Default)]
pub(crate) struct FenceTokenCache {
    /// Front is least recently used.
    entries: VecDeque<FenceEntry>,
    bytes: usize,
    highlights: u64,
    hits: u64,
}

impl FenceTokenCache {
    fn evict(&mut self) {
        while self.entries.len() > MARKDOWN_FENCE_CACHE_MAX_ENTRIES
            || self.bytes > MARKDOWN_FENCE_CACHE_MAX_BYTES
        {
            let Some(entry) = self.entries.pop_front() else {
                break;
            };
            self.bytes = self.bytes.saturating_sub(entry.charge());
        }
    }
}

fn fence_hash(language: &str, source: &str) -> u64 {
    let mut hasher = std::hash::DefaultHasher::new();
    language.hash(&mut hasher);
    source.hash(&mut hasher);
    hasher.finish()
}

impl FenceTokenMemo for FenceTokenCache {
    fn cached(&mut self, language: &str, source: &str) -> Option<Vec<CodeToken>> {
        let hash = fence_hash(language, source);
        let index = self.entries.iter().position(|entry| {
            entry.hash == hash
                && entry.source.len() == source.len()
                && entry.language == language
                && entry.source == source
        })?;
        let entry = self.entries.remove(index)?;
        let tokens = entry.tokens.clone();
        self.entries.push_back(entry);
        self.hits = self.hits.saturating_add(1);
        Some(tokens)
    }

    fn remember(&mut self, language: &str, source: &str, tokens: &[CodeToken]) {
        self.highlights = self.highlights.saturating_add(1);
        let entry = FenceEntry {
            hash: fence_hash(language, source),
            language: language.to_owned(),
            source: source.to_owned(),
            tokens: tokens.to_vec(),
        };
        let charge = entry.charge();
        if charge > MARKDOWN_FENCE_CACHE_MAX_BYTES {
            return;
        }
        self.entries.push_back(entry);
        self.bytes = self.bytes.saturating_add(charge);
        self.evict();
    }
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use gpui::SharedString;

    use super::{
        MARKDOWN_PARSE_CACHE_MAX_BYTES, MARKDOWN_PARSE_CACHE_MAX_ENTRIES, MarkdownParseCache,
        same_allocation,
    };
    use crate::markdown::MarkdownEngine;

    fn engine() -> MarkdownEngine {
        MarkdownEngine::new().expect("the built-in Markdown engine must construct")
    }

    #[test]
    fn repeated_body_parses_once_and_a_changed_body_reparses() {
        let engine = engine();
        let mut cache = MarkdownParseCache::default();
        let first = cache
            .document(&engine, "# heading\n\nbody")
            .expect("first body parses");
        let repeated = cache
            .document(&engine, "# heading\n\nbody")
            .expect("cached body is served");
        assert!(Rc::ptr_eq(&first, &repeated));
        let report = cache.report();
        assert_eq!(report.parses, 1);
        assert_eq!(report.hits, 1);
        assert_eq!(report.entries, 1);

        let changed = cache
            .document(&engine, "# heading\n\nbody!")
            .expect("revised body parses");
        assert!(!Rc::ptr_eq(&first, &changed));
        let report = cache.report();
        assert_eq!(report.parses, 2);
        assert_eq!(report.hits, 1);
        assert_eq!(report.entries, 2);
    }

    #[test]
    fn shared_bodies_match_by_identity_after_their_first_lookup() {
        let engine = engine();
        let mut cache = MarkdownParseCache::default();
        let body = "# heading\n\nA body long enough to live on the heap.";
        let borrowed = cache.prepared(&engine, body).expect("body parses");

        // An equal body in another allocation hits by content once and is
        // adopted, so every later lookup is an identity match.
        let shared = SharedString::from(body.to_owned());
        let first = cache
            .prepared_shared(&engine, &shared)
            .expect("equal body is served");
        assert!(Rc::ptr_eq(&borrowed, &first));
        let entry = cache.entries.back().expect("the entry stays cached");
        assert!(same_allocation(&entry.source, &shared));
        let again = cache
            .prepared_shared(&engine, &shared)
            .expect("identity lookup is served");
        assert!(Rc::ptr_eq(&borrowed, &again));
        let report = cache.report();
        assert_eq!(report.parses, 1);
        assert_eq!(report.hits, 2);
        assert_eq!(report.entries, 1);

        // A changed body never matches a stale identity or hint.
        let revised = SharedString::from(format!("{body}!"));
        let changed = cache
            .prepared_shared(&engine, &revised)
            .expect("revised body parses");
        assert!(!Rc::ptr_eq(&borrowed, &changed));
        assert_eq!(cache.report().parses, 2);
    }

    #[test]
    fn a_reused_caller_buffer_is_confirmed_by_bytes_not_trusted() {
        let engine = engine();
        let mut cache = MarkdownParseCache::default();
        let mut buffer = String::from("first body text");
        let first = cache.prepared(&engine, &buffer).expect("first parses");
        // Same address and length, different bytes: the hint must not serve
        // the old document.
        buffer.replace_range(.., "other body text");
        let second = cache.prepared(&engine, &buffer).expect("second parses");
        assert!(!Rc::ptr_eq(&first, &second));
        assert_eq!(cache.report().parses, 2);
        assert_eq!(
            crate::markdown::spans_text(match &second.document().blocks()[0] {
                crate::markdown::Block::Paragraph { spans, .. } => spans,
                other => panic!("expected a paragraph, got {other:?}"),
            }),
            "other body text"
        );
    }

    #[test]
    fn fence_memo_serves_only_the_exact_language_and_source() {
        let engine = engine();
        let mut cache = MarkdownParseCache::default();
        let rust = "```rust\nlet a = 1;\n```\n";
        let _ = cache.document(&engine, rust);
        assert_eq!(cache.report().fence_highlights, 1);
        // Same source under another language classifies again.
        let _ = cache.document(&engine, "```python\nlet a = 1;\n```\n");
        assert_eq!(cache.report().fence_highlights, 2);
        assert_eq!(cache.report().fence_highlight_hits, 0);
        // The same fence inside a different body is served from the memo.
        let wrapped = format!("intro\n\n{rust}");
        let _ = cache.document(&engine, &wrapped);
        let report = cache.report();
        assert_eq!(report.fence_highlights, 2);
        assert_eq!(report.fence_highlight_hits, 1);
    }

    #[test]
    fn entry_bound_evicts_oldest_and_keeps_the_newest_hot() {
        let engine = engine();
        let mut cache = MarkdownParseCache::default();
        let total = MARKDOWN_PARSE_CACHE_MAX_ENTRIES + 8;
        for index in 0..total {
            assert!(cache.document(&engine, &format!("body {index}")).is_some());
        }
        let report = cache.report();
        assert_eq!(report.entries, MARKDOWN_PARSE_CACHE_MAX_ENTRIES);
        assert!(report.bytes <= MARKDOWN_PARSE_CACHE_MAX_BYTES);
        let expected_parses = u64::try_from(total).expect("fixture count fits u64");

        // The newest body is still hot after the eviction wave.
        let newest = format!("body {}", total - 1);
        assert!(cache.document(&engine, &newest).is_some());
        assert_eq!(cache.report().hits, 1);
        assert_eq!(cache.report().parses, expected_parses);

        // The evicted oldest body re-parses instead of serving stale data.
        assert!(cache.document(&engine, "body 0").is_some());
        assert_eq!(cache.report().parses, expected_parses + 1);
        assert_eq!(cache.report().entries, MARKDOWN_PARSE_CACHE_MAX_ENTRIES);
    }

    #[test]
    fn byte_bound_keeps_charged_bytes_bounded() {
        let engine = engine();
        let mut cache = MarkdownParseCache::default();
        let chunk = "x".repeat(MARKDOWN_PARSE_CACHE_MAX_BYTES / 4);
        for _ in 0..4 {
            assert!(cache.document(&engine, &chunk).is_some());
        }
        // One body identity is one entry no matter how often it renders.
        assert_eq!(cache.report().entries, 1);
        for index in 0..12 {
            let body = format!("{chunk}{index}");
            assert!(cache.document(&engine, &body).is_some());
        }
        let report = cache.report();
        assert!(report.bytes <= MARKDOWN_PARSE_CACHE_MAX_BYTES);
        assert!(report.entries <= MARKDOWN_PARSE_CACHE_MAX_ENTRIES);
    }
}
