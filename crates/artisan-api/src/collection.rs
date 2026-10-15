//! Indexed canonical records share the immutable source store. A collection is
//! either producer data or a descriptor, never a partially loaded Vec.
use crate::ContentRef;
use serde::{Deserialize, Serialize};

/// The most records one collection range carries.
pub const COLLECTION_RANGE_ENTRIES: u32 = 64;
/// The most record bytes one collection range carries.
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inline_collections_are_plain_arrays_and_stored_ones_round_trip() {
        let records = vec!["α\n🙂".to_owned(), "second".to_owned()];
        let inline = Collection::from(records.clone());
        assert_eq!(
            serde_json::to_string(&inline).unwrap(),
            serde_json::to_string(&records).unwrap()
        );
        let stored = Collection::<String>::Stored(CollectionRef {
            source: ContentRef::from_text("\"a\"\n"),
            count: 1,
        });
        assert!(stored.inline().is_none());
        let roundtrip: Collection<String> =
            serde_json::from_str(&serde_json::to_string(&stored).unwrap()).unwrap();
        assert_eq!(roundtrip, stored);
    }
}
