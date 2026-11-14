use crate::frame::scramble::Rows;
use crate::frame::sidebar::{COLLAPSED_WIDTH, Event, OPEN_WIDTH};
use crate::frame::wordmark::LINE_HEIGHTS;
use crate::shell::layout::{CONTROL_INK_INSET, CONTROL_INSET_X, TITLEBAR_HEIGHT};
use crate::shell::titlebar::{LEAD, WORDMARK};
use crate::ui::chrome::account::{AVATAR_INSET, TRIGGER_HEIGHT};
use crate::ui::kit::{HitArea, Space, Spaced, TextRole};
use crate::ui::theme::ArtisanTheme;
use aqua_core::Role;
use aqua_ui::element::{Box as UiBox, Element, Text};
use aqua_ui::style::{Length, Style};
use aqua_ui::{AlignItems, JustifyContent};
use std::rc::Rc;

pub const SHEET_PADDING: f32 = Space::S12.px();
pub const SIDEBAR: &str = "artisan-frame-sidebar";
const TREE_GAP: f32 = Space::S24.px();
pub const WORDMARK_FAMILY: &str = "Sigurd";
pub const WORDMARK_SIZE: f32 = 32.8;
pub const WORDMARK_WIDTH: f32 = 1.45 * WORDMARK_SIZE;
pub const WORDMARK_TRACKING: f32 = -0.05;
const WORDMARK_HEIGHT: f32 =
    WORDMARK_SIZE * (LINE_HEIGHTS[0] + LINE_HEIGHTS[1] + LINE_HEIGHTS[2]);
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
    pub search: Option<Element>,
    pub tree: Option<Element>,
}

#[must_use]
pub fn body(
    theme: &ArtisanTheme,
    titlebar: Element,
    sidebar: Sidebar,
    overlay: Element,
    content: Vec<Element>,
    seam: Rc<std::cell::RefCell<crate::frame::seam::motion::Motion>>,
    reduced: bool,
) -> Element {
    let sheet = UiBox::new()
        .layout("column")
        .style(grow())
        .children(content)
        .build()
        .debug_selector(crate::frame::seam::SHEET);
    UiBox::new()
        .layout("row")
        .style(grow())
        .child(sidebar_view(theme, sidebar))
        .child(
            UiBox::new()
                .layout("column")
                .style(Style::new().flex_grow(1.0).min_width(Length::Pct(0.0)))
                .child(crate::frame::seam::surface(*theme, seam, reduced))
                .child(titlebar)
                .child(
                    UiBox::new()
                        .style(grow())
                        .child(placed(sheet, 0.0, 0.0, SHEET_PADDING, SHEET_PADDING))
                        .build(),
                )
                .build(),
        )
        .child(overlay)
        .build()
}

#[must_use]
pub fn search_anchor() -> crate::frame::search::Anchor {
    let rail = crate::frame::search::rail_width();
    let left = (COLLAPSED_WIDTH - rail) / 2.0;
    crate::frame::search::Anchor {
        left,
        top: TITLEBAR_HEIGHT + WORDMARK_INK_RISE + WORDMARK_HEIGHT + TREE_GAP,
        width: OPEN_WIDTH - left - SHEET_PADDING,
        bottom: TRIGGER_HEIGHT + SHEET_PADDING,
    }
}

fn grow() -> Style {
    Style::new()
        .flex_grow(1.0)
        .min_width(Length::Pct(0.0))
        .min_height(Length::Pct(0.0))
}

fn placed(child: Element, left: f32, top: f32, right: f32, bottom: f32) -> Element {
    UiBox::new()
        .layout("column")
        .style(Style::new().absolute_at(Some(left), Some(top), Some(right), Some(bottom)))
        .child(child)
        .build()
}

pub fn sidebar_view(theme: &ArtisanTheme, sidebar: Sidebar) -> Element {
    let mark = wordmark(theme, &sidebar);
    let mark_top = TITLEBAR_HEIGHT + WORDMARK_INK_RISE;
    let footer_height = match sidebar.footer {
        Some(_) => TRIGGER_HEIGHT + SHEET_PADDING,
        None => SHEET_PADDING,
    };
    UiBox::new()
        .layout("column")
        .width(sidebar.width)
        .style(
            Style::new()
                .height(Length::Pct(100.0))
                .flex_shrink(0.0)
                .clip(true),
        )
        .child(
            UiBox::new()
                .height(TITLEBAR_HEIGHT)
                .style(Style::new().absolute_at(Some(0.0), Some(0.0), Some(0.0), None))
                .build()
                .debug_selector(LEAD),
        )
        .child(
            UiBox::new()
                .height(mark_top + WORDMARK_HEIGHT)
                .style(Style::new().flex_shrink(0.0))
                .child(
                    UiBox::new()
                        .style(Style::new().absolute_at(
                            Some(INK_X - WORDMARK_INK_LEFT),
                            Some(mark_top),
                            None,
                            None,
                        ))
                        .child(mark)
                        .build(),
                )
                .build(),
        )
        .child(tree_view(sidebar.width, sidebar.search, sidebar.tree))
        .child(rail_view(theme, sidebar.width, &sidebar.emit))
        .child(
            UiBox::new()
                .height(footer_height)
                .style(Style::new().flex_shrink(0.0))
                .children(
                    sidebar
                        .footer
                        .map(|footer| placed(footer, FOOTER_LEFT, 0.0, SHEET_PADDING, SHEET_PADDING)),
                )
                .build(),
        )
        .build()
        .with_key(SIDEBAR)
}

fn tree_view(width: f32, search: Option<Element>, tree: Option<Element>) -> Element {
    let reveal = reveal(width);
    let row = crate::sidebar::geometry::TOGGLE.pad().x.px();
    let column = UiBox::new()
        .layout("column")
        .style(grow().opacity(reveal))
        .children((search.is_some() || tree.is_some()).then(|| {
            placed(
                UiBox::new()
                    .layout("column")
                    .spaced(Space::S16)
                    .style(grow())
                    .children(search)
                    .children(tree)
                    .build(),
                INK_X - row,
                TREE_GAP,
                SHEET_PADDING,
                TREE_GAP,
            )
        }))
        .build();
    match reveal > 0.0 {
        true => column,
        false => column.exclude_from_hit_testing(),
    }
}

fn rail_view(theme: &ArtisanTheme, width: f32, emit: &Rc<dyn Fn(Event)>) -> Element {
    let shown = 1.0 - reveal(width);
    let anchor = search_anchor();
    let rail = UiBox::new()
        .style(
            Style::new()
                .absolute_at(Some(anchor.left), Some(anchor.top), None, None)
                .opacity(shown),
        )
        .child(crate::frame::search::rail(theme, emit))
        .build();
    match shown > 0.0 {
        true => rail,
        false => rail.exclude_from_hit_testing(),
    }
}

fn reveal(width: f32) -> f32 {
    ((width - COLLAPSED_WIDTH) / (OPEN_WIDTH - COLLAPSED_WIDTH)).clamp(0.0, 1.0)
}

fn wordmark(theme: &ArtisanTheme, sidebar: &Sidebar) -> Element {
    let hover = Rc::clone(&sidebar.emit);
    let focus = Rc::clone(&sidebar.emit);
    let toggle = Rc::clone(&sidebar.emit);
    let rows = UiBox::new()
        .layout("column")
        .style(Style::new().align(AlignItems::CENTER))
        .children(
            sidebar
                .wordmark
                .iter()
                .zip(LINE_HEIGHTS)
                .map(|(row, line_height)| wordmark_row(theme, row, line_height * WORDMARK_SIZE)),
        )
        .build();
    let label = if sidebar.open {
        "Hide sidebar"
    } else {
        "Show sidebar"
    };
    UiBox::new()
        .width(WORDMARK_WIDTH)
        .height(WORDMARK_HEIGHT)
        .child(
            HitArea::new(WORDMARK, Role::Button, label)
                .child(rows)
                .on_activate(move || toggle(Event::Toggle))
                .build()
                .debug_selector(WORDMARK)
                .tab_index(0)
                .on_hover_change(move |inside| hover(Event::Hover(inside)))
                .on_focus_change(move |focused| focus(Event::Hover(focused))),
        )
        .build()
}

#[expect(
    clippy::disallowed_methods,
    reason = "the wordmark is a logotype set in its own face, not UI text"
)]
fn wordmark_row(theme: &ArtisanTheme, row: &[Option<char>], line_height: f32) -> Element {
    let mut style = TextRole::Heading.style(theme);
    style.families = vec![aqua_text::FontFamily::Named(WORDMARK_FAMILY.to_owned())];
    style.size_px = WORDMARK_SIZE;
    style.line_height_px = line_height;
    style.letter_spacing_em = Some(WORDMARK_TRACKING);
    let color = theme.colors.text.to_reference_text_color();
    UiBox::new()
        .layout("row")
        .height(line_height)
        .style(Style::new().justify(JustifyContent::CENTER))
        .children(row.iter().flatten().map(|glyph| {
            Text::new(glyph.to_string())
                .text_style(style.clone())
                .gpui_source_color(color)
                .build()
        }))
        .build()
}
