//! Rich blocks: `artisan-<kind>` fences drawn as native widgets.
//!
//! An agent writes a fence whose info string names a kind and whose body is
//! one JSON object; the transcript draws the kind's widget in the theme's
//! own tokens instead of the code. The contract and the catalog live in
//! `docs/rich-blocks.md`.
//!
//! A fence parses once, when its message body enters the parse cache
//! ([`prepare`]); frames reuse the parsed block. Every kind implements
//! [`RichBlock`]: it renders from a [`BlockContext`] and exposes optional
//! source for its copy action. Interaction state lives in the window,
//! keyed under the block's stable element id, so the renderer stays
//! synchronous and stateless.
//!
//! Most blocks sit directly on the transcript. Only blocks whose content
//! is code take the fence's glass card, and only they carry a copy action:
//! copying a chart or an image comparison means nothing.
//!
//! Each family of kinds owns one submodule and one `parse` entry point that
//! claims its kinds; [`parse_block`] asks each family in turn.

use std::fmt::Debug;
use std::sync::Arc;

use gpui::{
    AnyElement, ElementId, IntoElement, ParentElement as _, SharedString, Styled as _, div, px,
};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::theme::ArtisanTheme;

mod actions;
pub(super) mod budget;
mod charts;
mod code;
mod data;
mod kit;
mod media;
mod narrative;

pub(super) use actions::show_copy_menu;
pub use actions::{CopyEntry, CopyKind, OpenReference, ResourceScope, ShowCopyMenu};
pub use media::{MediaFetch, MediaFetcher, MediaRequest, decode_image_bytes, image_resident_bytes};
pub(super) use media::{color_copy_entries, color_paint, color_ranges};

/// Info-string prefix that marks a fence as a rich block.
const LANGUAGE_PREFIX: &str = "artisan-";

/// Largest fence body parsed as a block; anything longer stays code.
const MAX_SOURCE_BYTES: usize = crate::markdown::MAX_MARKDOWN_SOURCE_BYTES;

/// One parsed kind, ready to draw.
pub(super) trait RichBlock: Debug + Send + Sync {
    /// Retained payload and container capacity charged to the source cache.
    fn resident_bytes(&self) -> usize;

    /// The block's own content, without the envelope's title and caption.
    fn render(&self, cx: &BlockContext<'_>) -> AnyElement;

    /// What the card's copy action puts on the clipboard; `None` draws no
    /// copy action. Only code is worth copying.
    fn copy(&self) -> Option<String> {
        None
    }

    /// How the block sits in the transcript.
    fn chrome(&self) -> Chrome {
        Chrome::Open
    }

    /// Whether the block draws the envelope's `title` itself, so the
    /// envelope does not draw it again above the block.
    fn owns_title(&self) -> bool {
        false
    }
}

/// How a block sits in the transcript.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Chrome {
    /// On the fence's glass card, with the copy action in its corner when
    /// the block has something to copy: code.
    Card,
    /// Directly on the transcript with no frame: callouts.
    Bare,
    /// On the transcript with a little breathing room above and below:
    /// charts, tables, trees, images, framed by whitespace and their
    /// headline.
    Open,
}

/// What a block renders from: the theme and its stable element id.
pub(super) struct BlockContext<'a> {
    pub(super) theme: &'a ArtisanTheme,
    pub(super) media: Option<&'a MediaFetcher>,
    /// Unique per block in the message and stable across frames; every
    /// interactive part keys its window state under it.
    pub(super) id: SharedString,
}

impl BlockContext<'_> {
    /// A stable id for one part of the block.
    pub(super) fn id(&self, part: impl std::fmt::Display) -> ElementId {
        ElementId::Name(self.key(part))
    }

    /// The string form of [`Self::id`], for ids that must be `SharedString`.
    pub(super) fn key(&self, part: impl std::fmt::Display) -> SharedString {
        kit::part_key(&self.id, part)
    }
}

/// A closed rich fence as prepared once per message body.
#[derive(Debug)]
pub(super) enum RichFence {
    /// Still streaming: the kind is known, the body is not complete.
    Pending { kind: SharedString },
    /// Parsed and ready to draw.
    Ready(Prepared),
    /// Closed but unusable; the fence renders as code with this note.
    Invalid { note: SharedString },
}

/// A parsed block with its envelope.
#[derive(Debug)]
pub(super) struct Prepared {
    block: Arc<dyn RichBlock>,
    title: Option<SharedString>,
    /// One muted line under the title for scope and units.
    subtitle: Option<SharedString>,
    caption: Option<SharedString>,
    /// What the copy action copies, made once; `None` draws no action.
    copy: Option<SharedString>,
}

impl Prepared {
    pub(super) fn resident_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.block.resident_bytes()
            + self.title.as_ref().map_or(0, |text| text.len())
            + self.subtitle.as_ref().map_or(0, |text| text.len())
            + self.caption.as_ref().map_or(0, |text| text.len())
            + self.copy.as_ref().map_or(0, |text| text.len())
    }
    pub(super) fn chrome(&self) -> Chrome {
        self.block.chrome()
    }

    pub(super) fn copy(&self) -> Option<SharedString> {
        self.copy.clone()
    }
}

/// Recognizes and parses a rich fence; `None` for every other fence.
pub(super) fn prepare(language: Option<&str>, source: &str, closed: bool) -> Option<RichFence> {
    let kind = language?.strip_prefix(LANGUAGE_PREFIX)?;
    if kind.is_empty() {
        return None;
    }
    if !closed {
        return Some(RichFence::Pending {
            kind: SharedString::from(kind.replace('-', " ")),
        });
    }
    let parsed = if source.len() > MAX_SOURCE_BYTES {
        Err(format!(
            "the spec is larger than {} KiB",
            MAX_SOURCE_BYTES / 1024
        ))
    } else {
        serde_json::from_str::<Value>(source)
            .map_err(|error| format!("the JSON does not parse: {error}"))
            .and_then(|value| prepare_value(kind, &value))
    };
    Some(match parsed {
        Ok(prepared) => RichFence::Ready(prepared),
        Err(error) => RichFence::Invalid {
            note: SharedString::from(format!("Couldn't draw artisan-{kind}: {error}")),
        },
    })
}

fn prepare_value(kind: &str, value: &Value) -> Result<Prepared, String> {
    if !value.is_object() {
        return Err("the spec must be a JSON object".to_owned());
    }
    validate_value_budget(value)?;
    let block = parse_block(kind, value)?;
    let text = |field: &str| {
        value
            .get(field)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(|text| SharedString::from(text.to_owned()))
    };
    let title = text("title");
    let subtitle = text("subtitle");
    let caption = text("caption");
    let copy = block.copy().map(SharedString::from);
    Ok(Prepared {
        block,
        title,
        subtitle,
        caption,
        copy,
    })
}

fn validate_value_budget(root: &Value) -> Result<(), String> {
    const VALUES: usize = 32768;
    const DEPTH: usize = 32;
    let mut pending = vec![(root, 0usize)];
    let mut visited = 0;
    while let Some((value, depth)) = pending.pop() {
        visited += 1;
        if visited > VALUES || depth > DEPTH {
            return Err("rich preview exceeds 32768 JSON values or 32 nesting levels".into());
        }
        let count = match value {
            Value::Array(values) => values.len(),
            Value::Object(values) => values.len(),
            _ => 0,
        };
        if pending.len().saturating_add(count) > VALUES - visited {
            return Err("rich preview exceeds 32768 JSON values".into());
        }
        match value {
            Value::Array(values) => pending.extend(values.iter().map(|value| (value, depth + 1))),
            Value::Object(values) => {
                pending.extend(values.values().map(|value| (value, depth + 1)))
            }
            _ => {}
        }
    }
    Ok(())
}

/// One family's `parse`: claims its kinds, `None` for every other kind.
type FamilyParse = fn(&str, &Value) -> Option<Result<Arc<dyn RichBlock>, String>>;

/// Asks each family of kinds to claim `kind`.
fn parse_block(kind: &str, value: &Value) -> Result<Arc<dyn RichBlock>, String> {
    let families: [FamilyParse; 5] = [
        narrative::parse,
        code::parse,
        charts::parse,
        data::parse,
        media::parse,
    ];
    families
        .iter()
        .find_map(|parse| parse(kind, value))
        .unwrap_or_else(|| Err(format!("`{kind}` is not a rich block kind")))
}

/// Deserializes a kind's spec, naming the field at fault on failure.
pub(super) fn spec<T: DeserializeOwned>(value: &Value) -> Result<T, String> {
    T::deserialize(value).map_err(|error| error.to_string())
}

/// Boxes a parsed spec as a block; the shape every family's `parse` returns.
pub(super) fn block<T: RichBlock + DeserializeOwned + 'static>(
    value: &Value,
) -> Result<Arc<dyn RichBlock>, String> {
    spec::<T>(value).map(|block| Arc::new(block) as Arc<dyn RichBlock>)
}

/// The block's content under its title and subtitle and over its caption.
/// The title states the finding; the subtitle names scope and units.
pub(super) fn render_content(prepared: &Prepared, cx: &BlockContext<'_>) -> AnyElement {
    let theme = cx.theme;
    let title = prepared
        .title
        .clone()
        .filter(|_| !prepared.block.owns_title());
    let heading = (title.is_some() || prepared.subtitle.is_some()).then(|| {
        div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .children(title.map(|title| {
                div()
                    .text_size(px(15.0))
                    .line_height(px(22.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.colors.foreground.to_paint())
                    .child(title)
            }))
            .children(prepared.subtitle.clone().map(|subtitle| {
                div()
                    .text_size(px(13.0))
                    .line_height(px(20.0))
                    .text_color(theme.colors.muted_foreground.to_paint())
                    .child(subtitle)
            }))
    });
    div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(12.0))
        .children(heading)
        .child(prepared.block.render(cx))
        .children(prepared.caption.clone().map(|caption| {
            div()
                .text_size(px(13.0))
                .line_height(px(20.0))
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(caption)
        }))
        .into_any_element()
}

/// The quiet placeholder of a fence still streaming.
pub(super) fn render_pending(kind: &SharedString, theme: &ArtisanTheme) -> AnyElement {
    div()
        .text_size(px(14.0))
        .line_height(px(22.0))
        .text_color(theme.colors.muted_foreground.to_paint())
        .child(SharedString::from(format!("Drawing {kind}…")))
        .into_any_element()
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod budget_tests {
    use super::*;
    #[test]
    fn ignored_fields_cannot_hide_unbounded_spec_work() {
        let value = serde_json::json!({"unused":vec![0;32768]});
        assert!(validate_value_budget(&value).is_err());
        let mut value = Value::Null;
        for _ in 0..33 {
            value = Value::Array(vec![value]);
        }
        assert!(validate_value_budget(&value).is_err());
        assert!(validate_value_budget(&serde_json::json!({"value": [1,2,3]})).is_ok());
    }
}
