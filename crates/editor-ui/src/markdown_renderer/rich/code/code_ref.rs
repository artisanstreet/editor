//! `code-ref`: a pointer into the code, with an optional numbered excerpt.

use crate::markdown_renderer::rich::budget;
use gpui::{AnyElement, IntoElement, ParentElement as _, SharedString, Styled as _, div, px};
use serde::Deserialize;
use std::sync::Arc;

use super::super::kit::{self, CodeRef};
use super::super::{BlockContext, Chrome, RichBlock};
use super::{code_lines, excerpt_width, numbered, path_link};

/// The `code-ref` spec.
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
pub(super) struct CodeRefBlock {
    reference: CodeRef,
    excerpt: Option<String>,
    note: Option<String>,
    lines: Arc<[SharedString]>,
    width: gpui::Pixels,
}
impl From<Spec> for CodeRefBlock {
    fn from(spec: Spec) -> Self {
        let lines: Arc<[SharedString]> =
            code_lines(spec.excerpt.as_deref().unwrap_or_default()).into();
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

impl RichBlock for CodeRefBlock {
    fn resident_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.reference.path.len()
            + budget::optional(&self.excerpt)
            + budget::optional(&self.note)
            + budget::shared(&self.lines)
    }

    fn render(&self, cx: &BlockContext<'_>) -> AnyElement {
        let theme = cx.theme;
        let reference = &self.reference;
        let head = div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .min_w_0()
            .child(path_link(
                theme,
                cx.id("path"),
                &reference.path,
                reference.start,
                cx.media.and_then(|provider| provider.resource_scope()),
                reference.display(),
            ));
        div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(head)
            .children(self.note().map(|note| kit::muted(theme, note.to_owned())))
            .children(self.excerpt().map(|_| {
                numbered(
                    theme,
                    cx.id("excerpt"),
                    self.lines.clone(),
                    reference.start.unwrap_or(1),
                    self.width,
                )
            }))
            .into_any_element()
    }

    fn chrome(&self) -> Chrome {
        Chrome::Card
    }

    fn copy(&self) -> Option<String> {
        Some(
            self.excerpt()
                .map_or_else(|| self.reference.display(), str::to_owned),
        )
    }
}
