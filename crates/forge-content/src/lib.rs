mod texts;

pub use texts::ItemTexts;

use artisan_api::{
    Collection, CollectionRef, ContentId, ContentRef, INLINE_TEXT_BYTES, ItemKind, TextContent,
};
use serde::Serialize;

pub const INLINE_COLLECTION_ENTRIES: usize = 64;
pub const INLINE_COLLECTION_BYTES: usize = 64 * 1024;

pub fn collection_ref<T: Serialize>(records: &[T]) -> Result<CollectionRef, serde_json::Error> {
    use sha2::{Digest, Sha256};
    use std::io::Write;
    struct Writer {
        digest: Sha256,
        bytes: u64,
    }
    impl Write for Writer {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.bytes = self
                .bytes
                .checked_add(buf.len() as u64)
                .ok_or_else(|| std::io::Error::other("collection size overflow"))?;
            self.digest.update(buf);
            Ok(buf.len())
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

fn compact_collection<T: Serialize>(
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

pub fn compact_text(text: &mut TextContent) {
    if let TextContent::Inline(value) = text
        && value.len() > INLINE_TEXT_BYTES
    {
        *text = TextContent::Stored(ContentRef::from_text(value));
    }
}

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
#[path = "../tests/lib/unit.rs"]
mod tests;
