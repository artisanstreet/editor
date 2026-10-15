//! Worker-owned memo of syntax tokens for unchanged fenced sources.
//! Exact language/source checks protect against hash collisions. Entries
//! share immutable token slices with prepared presentations and charge
//! string capacities, token payloads and container capacity.
use crate::markdown::{CodeToken, FenceTokenMemo};
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
const MARKDOWN_FENCE_CACHE_MAX_ENTRIES: usize = 256;
const MARKDOWN_FENCE_CACHE_MAX_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug)]
struct FenceEntry {
    hash: u64,
    language: String,
    source: String,
    tokens: Arc<[CodeToken]>,
}

impl FenceEntry {
    fn charge(&self) -> usize {
        self.source
            .capacity()
            .saturating_add(self.language.capacity())
            .saturating_add(
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
    /// The immediately preceding miss; highlighting does not change source.
    pending_hash: Option<u64>,
}

impl FenceTokenCache {
    fn resident_bytes(&self) -> usize {
        self.bytes
            .saturating_add(self.entries.capacity() * std::mem::size_of::<FenceEntry>())
    }
    fn evict(&mut self) {
        while self.entries.len() > MARKDOWN_FENCE_CACHE_MAX_ENTRIES
            || self.resident_bytes() > MARKDOWN_FENCE_CACHE_MAX_BYTES
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
    fn cached(&mut self, language: &str, source: &str) -> Option<Arc<[CodeToken]>> {
        let hash = fence_hash(language, source);
        self.pending_hash = Some(hash);
        let index = self.entries.iter().position(|entry| {
            entry.hash == hash
                && entry.source.len() == source.len()
                && entry.language == language
                && entry.source == source
        })?;
        let entry = self.entries.remove(index)?;
        let tokens = entry.tokens.clone();
        self.entries.push_back(entry);
        Some(tokens)
    }

    fn remember(&mut self, language: &str, source: &str, tokens: Arc<[CodeToken]>) {
        let entry = FenceEntry {
            hash: self
                .pending_hash
                .take()
                .unwrap_or_else(|| fence_hash(language, source)),
            language: language.to_owned(),
            source: source.to_owned(),
            tokens,
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
    use super::*;
    use crate::markdown::CodeTokenKind;
    #[test]
    fn closed_fence_tokens_share_storage_and_exact_bytes_protect_collisions() {
        let mut memo = FenceTokenCache::default();
        let tokens: Arc<[CodeToken]> = vec![CodeToken {
            range: 0..3,
            kind: CodeTokenKind::Keyword,
        }]
        .into();
        assert!(memo.cached("rust", "let").is_none());
        memo.remember("rust", "let", tokens.clone());
        assert!(Arc::ptr_eq(&tokens, &memo.cached("rust", "let").unwrap()));
        memo.entries[0].hash = fence_hash("rust", "var");
        assert!(
            memo.cached("rust", "var").is_none(),
            "equal hash/language/length cannot substitute other source bytes"
        );
    }
}
