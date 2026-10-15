//! Code and repository kinds: see `docs/rich-blocks.md`.
//!
//! `code-ref`, `annotated-code`, and `file-tree`. The two code kinds sit on
//! the fence's glass card like any code and copy their code; the tree sits
//! on the transcript. The pieces they share — code lines with a
//! number gutter, clickable paths, quiet toggles, and Markdown fences — live
//! here; each kind's file holds only what is particular to it.

use std::sync::Arc;

use editor_assets::AssetId;
use gpui::{
    App, Div, ElementId, Entity, InteractiveElement as _, ParentElement as _, Pixels, SharedString,
    Stateful, StatefulInteractiveElement as _, Styled as _, Window, div, px,
};
use serde_json::Value;

use super::actions::open_reference;
use super::{RichBlock, block};
use crate::button::{Button, ButtonVariant, LabelOverflow};
use crate::theme::ArtisanTheme;

mod annotated;
mod code_ref;
mod file_tree;

#[cfg(test)]
mod tests;

/// Claims the code and repository kinds; `None` for every other kind.
pub(super) fn parse(kind: &str, value: &Value) -> Option<Result<Arc<dyn RichBlock>, String>> {
    Some(match kind {
        "code-ref" => block::<code_ref::CodeRefBlock>(value),
        "annotated-code" => block::<annotated::AnnotatedCode>(value),
        "file-tree" => block::<file_tree::FileTree>(value),
        _ => return None,
    })
}

/// Height of one code line.
const LINE_PX: f32 = 20.0;

/// `text` as display lines: tabs expanded, carriage returns and one
/// trailing newline dropped.
fn code_lines(text: &str) -> Vec<SharedString> {
    let text = text.strip_suffix('\n').unwrap_or(text);
    if text.is_empty() {
        return Vec::new();
    }
    text.split('\n')
        .map(|line| SharedString::from(line.trim_end_matches('\r').replace('\t', "    ")))
        .collect()
}

/// The column code lines sit in, directly on the block's card.
fn code_panel(theme: &ArtisanTheme) -> Div {
    div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .font_family(theme.typography.mono.family)
        .tracking(0.0)
        .text_size(px(13.0))
        .line_height(px(LINE_PX))
        .text_color(theme.colors.foreground.to_paint())
}

/// The width of a number gutter that fits `largest`.
fn gutter_width(largest: u32) -> Pixels {
    let digits = largest.max(1).ilog10() + 1;
    #[expect(clippy::cast_precision_loss, reason = "a digit count is tiny")]
    let digits = digits as f32;
    px(digits * 8.0 + 20.0)
}

/// A muted line number, right-aligned in a gutter of `width`.
fn line_number(theme: &ArtisanTheme, number: Option<u32>, width: Pixels) -> Div {
    div()
        .w(width)
        .flex_shrink_0()
        .flex()
        .justify_end()
        .pr(px(12.0))
        .text_color(theme.colors.muted_foreground.with_alpha(0.7).to_paint())
        .children(number.map(|number| SharedString::from(number.to_string())))
}

/// One code line: its number in the gutter, then the text.
fn line_row(theme: &ArtisanTheme, number: Option<u32>, gutter: Pixels, text: SharedString) -> Div {
    div()
        .w_full()
        .min_w_0()
        .flex()
        .child(line_number(theme, number, gutter))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .min_h(px(LINE_PX))
                .pr(px(12.0))
                .child(text),
        )
}

/// Intrinsic width of normalized monospace preview lines, measured once.
fn excerpt_width(lines: &[SharedString]) -> Pixels {
    let columns = lines
        .iter()
        .map(|line| unicode_width::UnicodeWidthStr::width(line.as_ref()))
        .max()
        .unwrap_or(0);
    px(columns as f32 * 7.8 + 12.0)
}

/// A numbered excerpt: `text` with lines counted from `start`.
fn numbered(
    theme: &ArtisanTheme,
    id: ElementId,
    lines: std::sync::Arc<[SharedString]>,
    start: u32,
    natural_width: Pixels,
) -> Stateful<Div> {
    let count = lines.len();
    let gutter = gutter_width(start.saturating_add(u32::try_from(count).unwrap_or(u32::MAX)));
    let theme = *theme;
    let panel_id = id.clone();
    let list = gpui::uniform_list(id, count, move |range, _, _| {
        range
            .filter_map(|index| {
                lines.get(index).map(|line| {
                    line_row(
                        &theme,
                        Some(start.saturating_add(u32::try_from(index).unwrap_or(u32::MAX))),
                        gutter,
                        line.clone(),
                    )
                    .h(px(LINE_PX))
                    .whitespace_nowrap()
                    .overflow_hidden()
                })
            })
            .collect::<Vec<_>>()
    })
    .w_full()
    .min_w(natural_width + gutter)
    .h(px(count.min(18) as f32 * LINE_PX));
    code_panel(&theme)
        .id(panel_id)
        .overflow_x_scroll()
        .child(list)
}

/// A path that opens in the host on click, its tail kept when it is long.
fn path_link(
    theme: &ArtisanTheme,
    id: ElementId,
    path: &SharedString,
    line: Option<u32>,
    scope: Option<super::actions::ResourceScope>,
    shown: impl Into<SharedString>,
) -> Button {
    let path = path.clone();
    Button::icon_text(id, *theme, AssetId::TABLER_FILE_TEXT, shown)
        .variant(ButtonVariant::Ghost)
        .label_overflow(LabelOverflow::Start)
        .on_activate(move |_, window, cx| open_reference(&path, line, scope.clone(), window, cx))
}

/// A hover listener that records `index` in the slot `field` picks while
/// hovered, and on leave clears it only if it still holds `index`, so
/// moving between two parts never clears the newer one.
fn hover_slot<T: 'static>(
    state: Entity<T>,
    index: usize,
    field: fn(&mut T) -> &mut Option<usize>,
) -> impl Fn(&bool, &mut Window, &mut App) + 'static {
    move |hovered, _, cx| {
        state.update(cx, |state, cx| {
            let slot = field(state);
            if *hovered {
                *slot = Some(index);
            } else if *slot == Some(index) {
                *slot = None;
            } else {
                return;
            }
            cx.notify();
        });
    }
}

/// `count` and a noun, pluralized with an `s`.
fn counted(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}
