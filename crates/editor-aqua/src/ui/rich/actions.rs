#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceScope {
    pub host: String,
    pub thread: artisan_api::ThreadId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenReference {
    pub scope: Option<ResourceScope>,
    pub path: String,
    pub line: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CopyEntry {
    pub label: String,
    pub text: String,
    pub kind: CopyKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CopyKind {
    Primary,
    Alternative,
}

impl CopyEntry {
    pub fn primary(label: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            text: text.into(),
            kind: CopyKind::Primary,
        }
    }

    pub fn alternative(label: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            text: text.into(),
            kind: CopyKind::Alternative,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShowCopyMenu {
    pub position: aqua_core::Point,
    pub entries: Vec<CopyEntry>,
}
