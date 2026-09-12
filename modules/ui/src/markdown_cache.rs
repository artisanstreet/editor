//! Bounded revision-keyed cache for parsed Markdown documents.
//!
//! [`MarkdownRenderer`](crate::markdown_renderer::MarkdownRenderer) runs
//! synchronously inside GPUI's render pass, and a window can hold many
//! assistant replies. Without memoization every frame re-parses every visible
//! body, including `syntect` highlighting for fenced code. This cache keeps
//! parsed [`MarkdownDocument`]s behind a shared pointer keyed by exact body
//! identity (length, hash, and byte comparison), so an unchanged body parses
//! once and repeated frames borrow the same document.
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

use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use crate::markdown::{MarkdownDocument, MarkdownEngine};

/// Maximum live parsed documents retained across render passes.
pub const MARKDOWN_PARSE_CACHE_MAX_ENTRIES: usize = 32;

/// Maximum charged source bytes across live parsed documents.
pub const MARKDOWN_PARSE_CACHE_MAX_BYTES: usize = 2 * 1024 * 1024;

/// Snapshot of the renderer's Markdown parse-cache counters.
///
/// The report is a review and test seam: `parses` counts bodies that reached
/// the parser (successes and failures), `hits` counts lookups served from the
/// cache, and `entries`/`bytes` describe the live cache after the last
/// lookup.
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
}

#[derive(Debug)]
struct CacheEntry {
    hash: u64,
    source: String,
    /// `None` is a cached parse failure: the body renders its plain fallback
    /// without paying for the failing parse again.
    document: Option<Rc<MarkdownDocument>>,
}

/// Least-recently-used cache of parsed Markdown documents.
#[derive(Debug, Default)]
pub(crate) struct MarkdownParseCache {
    /// Front is least recently used; hits move their entry to the back.
    entries: VecDeque<CacheEntry>,
    bytes: usize,
    parses: u64,
    hits: u64,
}

impl MarkdownParseCache {
    /// Returns the parsed document for `source`, parsing at most once per
    /// cached body.
    pub(crate) fn document(
        &mut self,
        engine: &MarkdownEngine,
        source: &str,
    ) -> Option<Rc<MarkdownDocument>> {
        let hash = source_hash(source);
        if let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.hash == hash && entry.source == source)
        {
            let entry = self
                .entries
                .remove(index)
                .expect("a located cache entry must remain present");
            let document = entry.document.clone();
            self.entries.push_back(entry);
            self.hits = self.hits.saturating_add(1);
            return document;
        }
        self.parses = self.parses.saturating_add(1);
        let document = engine.parse_document(source).ok().map(Rc::new);
        let source_bytes = source.len();
        if source_bytes <= MARKDOWN_PARSE_CACHE_MAX_BYTES {
            self.entries.push_back(CacheEntry {
                hash,
                source: source.to_owned(),
                document: document.clone(),
            });
            self.bytes = self.bytes.saturating_add(source_bytes);
            self.evict();
        }
        document
    }

    /// Returns the current counters.
    pub(crate) fn report(&self) -> MarkdownParseReport {
        MarkdownParseReport {
            parses: self.parses,
            hits: self.hits,
            entries: self.entries.len(),
            bytes: self.bytes,
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

/// Stable-within-a-process identity for one body's bytes.
fn source_hash(source: &str) -> u64 {
    let mut hasher = std::hash::DefaultHasher::new();
    source.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use std::rc::Rc;

    use super::{
        MARKDOWN_PARSE_CACHE_MAX_BYTES, MARKDOWN_PARSE_CACHE_MAX_ENTRIES, MarkdownParseCache,
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
