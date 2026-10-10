//! `code-ref`: a pointer into the code, with an optional numbered excerpt.

use gpui::{AnyElement, IntoElement, ParentElement as _, Styled as _, div, px};
use serde::Deserialize;
use std::fmt::Write as _;

use super::super::kit::{self, CodeRef};
use super::super::{BlockContext, Chrome, RichBlock};
use super::{fence, numbered, path_link};

/// The `code-ref` spec.
#[derive(Debug, Deserialize)]
pub(super) struct CodeRefBlock {
    #[serde(rename = "ref")]
    reference: CodeRef,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    excerpt: Option<String>,
    #[serde(default)]
    note: Option<String>,
}

impl CodeRefBlock {
    fn language(&self) -> Option<&str> {
        self.language
            .as_deref()
            .map(str::trim)
            .filter(|language| !language.is_empty())
    }

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
            .children(
                self.excerpt()
                    .map(|excerpt| numbered(theme, excerpt, reference.start.unwrap_or(1))),
            )
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

    fn markdown(&self) -> String {
        let mut out = format!("`{}`", self.reference.display());
        if let Some(note) = self.note() {
            let _ = write!(out, " — {note}");
        }
        out.push('\n');
        if let Some(excerpt) = self.excerpt() {
            out.push('\n');
            out.push_str(&fence(self.language(), excerpt));
        }
        out
    }
}
