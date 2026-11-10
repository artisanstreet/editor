use super::super::kit::CodeRef;
use super::{code_lines, excerpt_width};
use serde::Deserialize;
use std::sync::Arc;

mod view;

#[derive(Debug, Deserialize)]
struct Spec {
    #[serde(rename = "ref")]
    reference: CodeRef,
    #[serde(default)]
    excerpt: Option<String>,
    #[serde(default)]
    note: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(from = "Spec")]
pub(crate) struct CodeRefBlock {
    reference: CodeRef,
    excerpt: Option<String>,
    note: Option<String>,
    lines: Arc<[String]>,
    width: f32,
}
impl CodeRefBlock {
    pub(crate) fn resident_bytes(&self) -> usize {
        use crate::ui::rich::kit::retained_optional;
        std::mem::size_of::<Self>()
            + self.reference.path.capacity()
            + retained_optional(self.excerpt.as_ref())
            + retained_optional(self.note.as_ref())
            + std::mem::size_of_val(self.lines.as_ref())
            + self.lines.iter().map(String::capacity).sum::<usize>()
    }
}
impl From<Spec> for CodeRefBlock {
    fn from(spec: Spec) -> Self {
        let lines: Arc<[String]> = code_lines(spec.excerpt.as_deref().unwrap_or_default()).into();
        let width = excerpt_width(&lines);
        Self {
            reference: spec.reference,
            excerpt: spec.excerpt,
            note: spec.note,
            lines,
            width,
        }
    }
}

impl CodeRefBlock {
    fn excerpt(&self) -> Option<&str> {
        self.excerpt
            .as_deref()
            .filter(|excerpt| !excerpt.trim().is_empty())
    }

    fn note(&self) -> Option<&str> {
        self.note
            .as_deref()
            .map(str::trim)
            .filter(|note| !note.is_empty())
    }
}
