//! Forge's content policy over `artisan-api` items: which authored texts and
//! record collections leave the item as stored sources, and the identity
//! those sources get. The store applies it when it saves an item; engine
//! decoders apply the same rules to the copies they keep of settled items, so
//! a later event about the same item names sources the store already has.

mod texts;

pub use texts::ItemTexts;

use artisan_api::{
    Collection, CollectionRef, ContentId, ContentRef, INLINE_TEXT_BYTES, ItemKind, TextContent,
};
use serde::Serialize;

/// A collection with more records than this is stored.
pub const INLINE_COLLECTION_ENTRIES: usize = 64;
/// A collection whose NDJSON is larger than this is stored.
pub const INLINE_COLLECTION_BYTES: usize = 64 * 1024;

/// The stored identity of `records`: SHA-256 of their exact NDJSON, one
/// record per line. Serialization streams into the digest; no combined JSON
/// buffer is built.
pub fn collection_ref<T: Serialize>(records: &[T]) -> Result<CollectionRef, serde_json::Error> {
    use sha2::{Digest, Sha256};
    use std::io::Write;
    struct Writer {
        digest: Sha256,
        bytes: u64,
    }
    impl Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.bytes = self
                .bytes
                .checked_add(bytes.len() as u64)
                .ok_or_else(|| std::io::Error::other("collection size overflow"))?;
            self.digest.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = Writer {
        digest: Sha256::new(),
        bytes: 0,
    };
    for record in records {
        serde_json::to_writer(&mut writer, record)?;
        writer.write_all(b"\n").map_err(serde_json::Error::io)?;
    }
    Ok(CollectionRef {
        source: ContentRef {
            id: ContentId(hex::encode(writer.digest.finalize())),
            bytes: writer.bytes,
            lines: records.len() as u64 + 1,
        },
        count: records.len() as u64,
    })
}

/// Replaces a large inline collection by its stored identity. Nested texts
/// must already be compacted, so the identity matches what the store saved.
pub fn compact_collection<T: Serialize>(
    collection: &mut Collection<T>,
) -> Result<bool, serde_json::Error> {
    let Collection::Inline(records) = collection else {
        return Ok(false);
    };
    let reference = collection_ref(records)?;
    if records.len() > INLINE_COLLECTION_ENTRIES
        || reference.source.bytes.saturating_add(1) > INLINE_COLLECTION_BYTES as u64
    {
        *collection = Collection::Stored(reference);
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Replaces a text the store keeps as a source by its identity.
pub fn compact_text(text: &mut TextContent) {
    if let TextContent::Inline(value) = text
        && value.len() > INLINE_TEXT_BYTES
    {
        *text = TextContent::Stored(ContentRef::from_text(value));
    }
}

/// What a settled item becomes once the store has its complete event:
/// every large text and collection by its stored identity.
pub fn compact_settled(kind: &mut ItemKind) -> Result<(), serde_json::Error> {
    let texts: Result<(), std::convert::Infallible> = kind.try_visit_texts_mut(|text| {
        compact_text(text);
        Ok(())
    });
    match texts {
        Ok(()) => {}
        Err(never) => match never {},
    }
    kind.compact_collections()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The store and the decoders name a stored collection the same way, or
    /// a later event about a settled item would name a source Forge lacks.
    #[test]
    fn a_collection_is_named_by_its_exact_ndjson() {
        let records = vec!["α\n🙂".to_owned(), "second".to_owned()];
        let reference = collection_ref(&records).unwrap();
        let exact = records
            .iter()
            .map(|record| serde_json::to_string(record).unwrap() + "\n")
            .collect::<String>();
        assert_eq!(reference.source, ContentRef::from_text(&exact));
        assert_eq!(reference.count, 2);
    }

    #[test]
    fn a_large_collection_keeps_its_count_and_drops_its_records() {
        let mut records = Collection::from(
            (0..5000)
                .map(|index| format!("record-{index}"))
                .collect::<Vec<_>>(),
        );
        assert!(compact_collection(&mut records).unwrap());
        assert_eq!(records.len(), 5000);
        assert!(records.inline().is_none());
        assert!(!compact_collection(&mut records).unwrap());
    }
}
