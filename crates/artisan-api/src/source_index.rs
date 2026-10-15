//! Pure source topology. Client alone supplies fonts, widths, layout and styling.
use crate::ContentRef;
use serde::{Deserialize, Serialize};
/// Source interpretation is independent of the content hash and renderer.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceIndexFormat {
    Literal,
    Markdown,
}
impl SourceIndexFormat {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Literal => "literal",
            Self::Markdown => "markdown",
        }
    }
}
pub const SOURCE_INDEX_VERSION: u32 = 1;
pub const SOURCE_INDEX_PAGE_LEAVES: u32 = 64;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceIndexHeader {
    pub format: SourceIndexFormat,
    pub source: ContentRef,
    pub metadata: ContentRef,
    pub version: u32,
    pub leaves: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceLeafDescriptor {
    pub ordinal: u64,
    pub source_start: u64,
    pub source_end: u64,
    pub metadata_start: u64,
    pub metadata_end: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceIndexQuery {
    pub format: SourceIndexFormat,
    /// Absent starts at the latest bounded leaf window.
    pub offset: Option<u64>,
    pub limit: Option<u32>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceIndexRange {
    pub index: SourceIndexHeader,
    pub start: u64,
    pub end: u64,
    pub leaves: Vec<SourceLeafDescriptor>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_interpretation_is_explicit_on_the_wire() {
        let query: SourceIndexQuery =
            serde_json::from_str(r#"{"format":"literal","offset":null,"limit":64}"#).unwrap();
        assert_eq!(query.format, SourceIndexFormat::Literal);
        assert!(serde_json::from_str::<SourceIndexQuery>(r#"{"offset":null,"limit":64}"#).is_err());
        assert!(
            serde_json::from_str::<SourceIndexQuery>(
                r#"{"format":"ansi","offset":null,"limit":64}"#
            )
            .is_err()
        );
        assert_eq!(SourceIndexFormat::Markdown.as_str(), "markdown");
    }
}
