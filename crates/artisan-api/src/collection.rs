//! Indexed canonical records share the immutable source store. A collection is
//! either producer data or a descriptor, never a partially loaded Vec.
use crate::ContentRef;
use serde::{Deserialize, Serialize};

pub const INLINE_COLLECTION_ENTRIES: usize = 64;
pub const COLLECTION_RANGE_ENTRIES: u32 = 64;
pub const INLINE_COLLECTION_BYTES: usize = 64 * 1024;
pub const COLLECTION_RANGE_BYTES: usize = 128 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollectionRef {
    pub source: ContentRef,
    pub count: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Collection<T> {
    Inline(Vec<T>),
    Stored(CollectionRef),
}
impl<T> Default for Collection<T> {
    fn default() -> Self {
        Self::Inline(Vec::new())
    }
}
impl<T> From<Vec<T>> for Collection<T> {
    fn from(records: Vec<T>) -> Self {
        Self::Inline(records)
    }
}
impl<T> Collection<T> {
    pub fn len(&self) -> u64 {
        match self {
            Self::Inline(records) => records.len() as u64,
            Self::Stored(reference) => reference.count,
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn inline(&self) -> Option<&[T]> {
        match self {
            Self::Inline(records) => Some(records),
            Self::Stored(_) => None,
        }
    }
    pub fn inline_mut(&mut self) -> Option<&mut Vec<T>> {
        match self {
            Self::Inline(records) => Some(records),
            Self::Stored(_) => None,
        }
    }
    pub fn resident_bytes(&self, record_bytes: impl Fn(&T) -> usize) -> usize {
        match self {
            Self::Inline(records) => {
                records.capacity() * std::mem::size_of::<T>()
                    + records.iter().map(record_bytes).sum::<usize>()
            }
            Self::Stored(reference) => reference.source.id.0.capacity(),
        }
    }
    pub fn stored(&self) -> Option<&CollectionRef> {
        match self {
            Self::Inline(_) => None,
            Self::Stored(reference) => Some(reference),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollectionQuery {
    #[serde(default)]
    pub offset: u64,
    pub limit: Option<u32>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollectionRange<T> {
    pub source: ContentRef,
    pub start: u64,
    pub end: u64,
    pub total: u64,
    pub records: Vec<T>,
}

impl CollectionRef {
    /// Exact NDJSON identity shared by adapters and Store. Serialization emits
    /// one record at a time to the digest writer, without a combined JSON buffer.
    pub fn from_records<T: Serialize>(records: &[T]) -> Result<Self, serde_json::Error> {
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
        Ok(Self {
            source: ContentRef {
                id: crate::ContentId(hex::encode(writer.digest.finalize())),
                bytes: writer.bytes,
                lines: records.len() as u64 + 1,
            },
            count: records.len() as u64,
        })
    }
}

impl<T: Serialize> Collection<T> {
    /// Call after the initial complete producer event has been emitted and its
    /// nested sources normalized. The descriptor matches Store's NDJSON identity.
    pub fn compact_if_large(&mut self) -> Result<bool, serde_json::Error> {
        let Self::Inline(records) = self else {
            return Ok(false);
        };
        let reference = CollectionRef::from_records(records)?;
        if records.len() > INLINE_COLLECTION_ENTRIES
            || reference.source.bytes.saturating_add(1) > INLINE_COLLECTION_BYTES as u64
        {
            *self = Self::Stored(reference);
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reference_identity_is_exact_ndjson_and_serialization_preserves_inline_arrays() {
        let records = vec!["α\n🙂".to_owned(), "second".to_owned()];
        let reference = CollectionRef::from_records(&records).unwrap();
        let exact = records
            .iter()
            .map(|record| format!("{}\n", serde_json::to_string(record).unwrap()))
            .collect::<String>();
        assert_eq!(reference.source, ContentRef::from_text(&exact));
        assert_eq!(reference.count, 2);
        let inline = Collection::from(records.clone());
        assert_eq!(
            serde_json::to_string(&inline).unwrap(),
            serde_json::to_string(&records).unwrap()
        );
        let stored = Collection::<String>::Stored(reference);
        assert!(stored.inline().is_none());
        let roundtrip: Collection<String> =
            serde_json::from_str(&serde_json::to_string(&stored).unwrap()).unwrap();
        assert_eq!(roundtrip, stored);
    }
    #[test]
    fn large_retained_collection_keeps_count_without_retaining_the_array() {
        let mut records = Collection::from(
            (0..5000)
                .map(|index| format!("record-{index}"))
                .collect::<Vec<_>>(),
        );
        assert!(records.compact_if_large().unwrap());
        assert_eq!(records.len(), 5000);
        assert!(records.inline().is_none());
        assert!(records.resident_bytes(String::capacity) < 128);
        assert!(!records.compact_if_large().unwrap());
    }
}
