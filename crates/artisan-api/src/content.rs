use serde::{Deserialize, Serialize};

pub const INLINE_TEXT_BYTES: usize = 16 * 1024;
pub const CONTENT_RANGE_BYTES: u32 = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContentId(pub String);
impl ContentId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn is_valid(&self) -> bool {
        self.0.len() == 64
            && self
                .0
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    }
}
impl std::fmt::Display for ContentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentRef {
    pub id: ContentId,
    pub bytes: u64,
    pub lines: u64,
}

impl ContentRef {
    pub fn from_text(text: &str) -> Self {
        use sha2::{Digest as _, Sha256};
        Self {
            id: ContentId(hex::encode(Sha256::digest(text.as_bytes()))),
            bytes: text.len() as u64,
            lines: 1 + text.bytes().filter(|byte| *byte == b'\n').count() as u64,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TextContent {
    Inline(String),
    Stored(ContentRef),
}
impl Default for TextContent {
    fn default() -> Self {
        Self::Inline(String::new())
    }
}
impl From<String> for TextContent {
    fn from(text: String) -> Self {
        Self::Inline(text)
    }
}
impl From<&str> for TextContent {
    fn from(text: &str) -> Self {
        Self::Inline(text.to_owned())
    }
}
impl TextContent {
    pub fn inline(&self) -> Option<&str> {
        match self {
            Self::Inline(text) => Some(text),
            Self::Stored(_) => None,
        }
    }
    pub const fn byte_len(&self) -> u64 {
        match self {
            Self::Inline(text) => text.len() as u64,
            Self::Stored(source) => source.bytes,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContentQuery {
    #[serde(default)]
    pub offset: u64,
    pub limit: Option<u32>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentRange {
    pub id: ContentId,
    pub start: u64,
    pub end: u64,
    pub total: u64,
    pub text: String,
}

#[cfg(test)]
#[path = "../tests/content/unit.rs"]
mod tests;
