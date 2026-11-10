use crate::ui::chrome::account::AVATAR_INSET;
use crate::frame::scramble::Rows;
use crate::frame::sidebar::{COLLAPSED_WIDTH, Event, OPEN_WIDTH};
use crate::frame::tree::ROW_PAD;
use crate::frame::wordmark::LINE_HEIGHTS;
use crate::shell::layout::{CONTROL_INK_INSET, CONTROL_INSET_X, TITLEBAR_HEIGHT};
use crate::shell::titlebar::{LEAD, WORDMARK};
use crate::ui::theme::ArtisanTheme;
use aqua::{Aqua, Style, pct, px};
use aqua_core::{CursorIcon, Insets};
use aqua_text::{FontFamily, TextStyle};
use aqua_ui::{AlignItems, JustifyContent, element::Element};
use std::rc::Rc;

pub const SHEET_PADDING: f32 = 12.0;
pub const SIDEBAR: &str = "artisan-frame-sidebar";
const TREE_GAP: f32 = 24.0;
pub const WORDMARK_FAMILY: &str = "Sigurd";
pub const WORDMARK_SIZE: f32 = 32.8;
pub const WORDMARK_WIDTH: f32 = 1.45 * WORDMARK_SIZE;
pub const WORDMARK_TRACKING: f32 = -0.05;
pub const INK_X: f32 = CONTROL_INSET_X + CONTROL_INK_INSET;
pub const FOOTER_LEFT: f32 = INK_X - AVATAR_INSET;
const WORDMARK_ASCENT: f32 = 0.74;
const WORDMARK_CONTENT: f32 = 0.972;
const WORDMARK_APEX: f32 = 0.729;
pub const WORDMARK_INK_LEFT: f32 = 0.7;
const WORDMARK_INK_RISE: f32 =
    WORDMARK_SIZE * (WORDMARK_APEX - (LINE_HEIGHTS[0] - WORDMARK_CONTENT) / 2.0 - WORDMARK_ASCENT);

pub struct Sidebar {
    pub width: f32,
    pub open: bool,
    pub wordmark: Rows,
    pub emit: Rc<dyn Fn(Event)>,
    pub footer: Option<Element>,
    pub tree: Option<Element>,
}

#[must_use]
pub fn body(
    theme: &ArtisanTheme,
    titlebar: Element,
    sidebar: Sidebar,
    content: Vec<Element>,
    seam: Rc<std::cell::RefCell<crate::frame::seam::motion::Motion>>,
    reduced: bool,
) -> Element {
    Aqua::Box()
        .layout("row")
        .styles([Aqua::Style()
            .flex_grow(1.0)
            .min_width(px(0.0))
            .min_height(px(0.0))])
        .child(sidebar_view(theme, sidebar))
        .child(
            Aqua::Box()
                .layout("column")
                .styles([Aqua::Style().flex_grow(1.0).min_width(px(0.0))])
                .child(crate::frame::seam::surface(*theme, seam, reduced))
                .child(titlebar)
                .child(
                    Aqua::Box()
                        .layout("column")
                        .styles([Aqua::Style()
                            .flex_grow(1.0)
                            .min_height(px(0.0))
                            .padding_insets(Insets {
                                right: SHEET_PADDING,
                                bottom: SHEET_PADDING,
                                ..Insets::default()
                            })])
                        .child(
                            Aqua::Box()
                                .layout("column")
                                .styles([Aqua::Style()
                                    .flex_grow(1.0)
                                    .min_width(px(0.0))
                                    .min_height(px(0.0))])
                                .children(content)
                                .build()
                                .debug_selector(crate::frame::seam::SHEET),
                        )
                        .build(),
                )
                .build(),
        )
        .build()
}

pub fn sidebar_view(theme: &ArtisanTheme, sidebar: Sidebar) -> Element {
    let mark = wordmark(theme, &sidebar);
    Aqua::Box()
        .layout("column")
        .styles([Aqua::Style()
            .width(px(sidebar.width))
            .height(pct(100.0))
            .flex_shrink(0.0)
            .clip(true)])
        .child(
            Aqua::Box()
                .styles([Style::new()
                    .absolute_at(Some(0.0), Some(0.0), Some(0.0), None)
                    .height(px(TITLEBAR_HEIGHT))])
                .build()
                .debug_selector(LEAD),
        )
        .child(
            Aqua::Box()
                .layout("column")
                .styles([Aqua::Style()
                    .flex_shrink(0.0)
                    .align(AlignItems::START)
                    .padding_insets(Insets {
                        left: INK_X - WORDMARK_INK_LEFT,
                        top: TITLEBAR_HEIGHT + WORDMARK_INK_RISE,
                        ..Insets::default()
                    })])
                .child(mark)
                .build(),
        )
        .child(tree_view(sidebar.width, sidebar.tree))
        .child(
            Aqua::Box()
                .layout("column")
                .styles([Aqua::Style().flex_shrink(0.0).padding_insets(Insets {
                    left: FOOTER_LEFT,
                    right: SHEET_PADDING,
                    bottom: SHEET_PADDING,
                    ..Insets::default()
                })])
                .children(sidebar.footer)
                .build(),
        )
        .build()
        .with_key(SIDEBAR)
}

fn tree_view(width: f32, tree: Option<Element>) -> Element {
    let reveal = ((width - COLLAPSED_WIDTH) / (OPEN_WIDTH - COLLAPSED_WIDTH)).clamp(0.0, 1.0);
    let column = Aqua::Box()
        .layout("column")
        .styles([Aqua::Style()
            .flex_grow(1.0)
            .min_height(px(0.0))
            .min_width(px(0.0))
            .opacity(reveal)
            .padding_insets(Insets {
                left: INK_X - ROW_PAD,
                right: SHEET_PADDING,
                top: TREE_GAP,
                bottom: TREE_GAP,
            })])
        .children(tree)
        .build();
    match reveal > 0.0 {
        true => column,
        false => column.exclude_from_hit_testing(),
    }
}

fn wordmark(theme: &ArtisanTheme, sidebar: &Sidebar) -> Element {
    let hover = Rc::clone(&sidebar.emit);
    let focus = Rc::clone(&sidebar.emit);
    let toggle = Rc::clone(&sidebar.emit);
    let rows = sidebar
        .wordmark
        .iter()
        .zip(LINE_HEIGHTS)
        .map(|(row, line_height)| wordmark_row(theme, row, line_height * WORDMARK_SIZE));
    Aqua::Button()
        .styles([Aqua::Style()
            .width(px(WORDMARK_WIDTH))
            .column()
            .align(AlignItems::CENTER)])
        .on_activate(move || toggle(Event::Toggle))
        .children(rows)
        .build()
        .with_key(WORDMARK)
        .debug_selector(WORDMARK)
        .aria_label(if sidebar.open {
            "Hide sidebar"
        } else {
            "Show sidebar"
        })
        .tab_index(0)
        .cursor(CursorIcon::Pointer)
        .on_hover_change(move |inside| hover(Event::Hover(inside)))
        .on_focus_change(move |focused| focus(Event::Hover(focused)))
}

fn wordmark_row(theme: &ArtisanTheme, row: &[Option<char>], line_height: f32) -> Element {
    let style = TextStyle {
        families: vec![FontFamily::Named(WORDMARK_FAMILY.to_owned())],
        size_px: WORDMARK_SIZE,
        line_height_px: line_height,
        letter_spacing_em: Some(WORDMARK_TRACKING),
        ..TextStyle::default()
    };
    let color = theme.colors.text.to_reference_paint();
    Aqua::Box()
        .layout("row")
        .styles([Style::new()
            .height(px(line_height))
            .justify(JustifyContent::CENTER)])
        .children(row.iter().flatten().map(|glyph| {
            Aqua::Text(glyph.to_string())
                .text_style(style.clone())
                .gpui_color(color)
                .build()
        }))
        .build()
}
