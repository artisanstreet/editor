//! The settings surface's chrome, ported from the previous Editor's
//! `native_settings/render/chrome.rs`: the page header, a section block,
//! the card and its rows, and the nav rail's heading, group labels, rows
//! and anchor list.

use crate::page::Anchor;
use editor_ui::card::{CardStyle, compact_card, compact_card_content};
use editor_ui::theme::ArtisanTheme;
use gpui::{
    AnyElement, Div, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, Styled as _, div, px,
};

/// Content width: legacy `max-w-4xl`.
pub(crate) const CONTENT_MAX_WIDTH: f32 = 896.0;
/// Nav rail width: legacy `md:w-44`.
pub(crate) const NAV_RAIL_WIDTH: f32 = 176.0;
/// Gap between the rail and the content: legacy `md:gap-14`.
pub(crate) const NAV_CONTENT_GAP: f32 = 56.0;

/// The page header (`header.svelte`): a 20 px title over a muted line.
pub(crate) fn header(
    theme: &ArtisanTheme,
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
) -> Div {
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .text_size(px(20.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.colors.foreground.to_paint())
                .child(title.into()),
        )
        .child(
            div()
                .mt(px(6.0))
                .text_sm()
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(description.into()),
        )
}

/// One section block (`section.svelte`): the heading with an optional
/// action at its baseline, an optional intro, then the body.
pub(crate) fn section_shell(
    theme: &ArtisanTheme,
    anchor: &str,
    title: &str,
    intro: Option<&str>,
    action: Option<AnyElement>,
    body: impl IntoElement,
) -> Div {
    let selector = format!("settings-section-{anchor}");
    let mut head = div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap(px(16.0))
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.colors.foreground.to_paint())
                .child(title.to_owned()),
        );
    if let Some(action) = action {
        head = head.child(action);
    }
    let mut section = div()
        .flex()
        .flex_col()
        .mt(px(40.0))
        .debug_selector(move || selector.clone())
        .child(head);
    if let Some(intro) = intro {
        section = section.child(
            div()
                .mt(px(4.0))
                .text_sm()
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(intro.to_owned()),
        );
    }
    section.child(div().mt(px(12.0)).child(body))
}

/// One card (`card.svelte`): the compact card with a hairline between
/// its bands.
pub(crate) fn card(theme: &ArtisanTheme, blocks: Vec<AnyElement>) -> Div {
    let style = CardStyle::resolve(*theme);
    let border = theme.colors.border.to_paint();
    let mut card = compact_card(style).w_full().gap(px(0.0)).py(px(0.0));
    for (index, block) in blocks.into_iter().enumerate() {
        let mut band = compact_card_content(style).w_full().child(block);
        if index > 0 {
            band = band.border_t_1().border_color(border);
        }
        card = card.child(band);
    }
    card
}

/// One row (`row.svelte`): a title over a description, and an optional
/// trailing control.
pub(crate) fn row(
    theme: &ArtisanTheme,
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    control: Option<AnyElement>,
) -> Div {
    let description = description.into();
    let mut text = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .gap(px(2.0))
        .child(
            div()
                .text_sm()
                .text_color(theme.colors.foreground.to_paint())
                .child(title.into()),
        );
    if !description.is_empty() {
        text = text.child(
            div()
                .text_xs()
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(description),
        );
    }
    let mut row = div()
        .w_full()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .gap(px(24.0))
        .py(px(14.0))
        .child(text);
    if let Some(control) = control {
        row = row.child(div().flex_shrink_0().child(control));
    }
    row
}

/// A muted line inside a card band (`py` given by the caller's state).
pub(crate) fn note(theme: &ArtisanTheme, text: impl Into<SharedString>) -> Div {
    div()
        .w_full()
        .py(px(12.0))
        .text_sm()
        .text_color(theme.colors.muted_foreground.to_paint())
        .child(text.into())
}

/// A muted 12 px line under a section (the App icon and Typography notes).
pub(crate) fn footnote(theme: &ArtisanTheme, text: impl Into<SharedString>) -> Div {
    div()
        .mt(px(10.0))
        .text_xs()
        .text_color(theme.colors.muted_foreground.to_paint())
        .child(text.into())
}

/// What failed and why, in the destructive colour.
pub(crate) fn error_text(theme: &ArtisanTheme, text: impl Into<SharedString>) -> Div {
    div()
        .w_full()
        .py(px(12.0))
        .text_sm()
        .text_color(theme.colors.destructive.to_paint())
        .child(text.into())
}

/// The rail's `Settings` heading.
pub(crate) fn nav_heading(theme: &ArtisanTheme) -> Div {
    div()
        .px(px(8.0))
        .pb(px(12.0))
        .text_sm()
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.colors.foreground.to_paint())
        .child("Settings")
}

/// One uppercase group label (`ARTISAN`, `ENGINES`).
pub(crate) fn nav_group_label(theme: &ArtisanTheme, label: &'static str) -> Div {
    div()
        .px(px(8.0))
        .pt(px(20.0))
        .pb(px(6.0))
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.colors.muted_foreground.to_paint())
        .child(label)
}

/// One rail row (`nav.svelte` `nav_link`): the open page wears the muted
/// well and the foreground at medium weight.
pub(crate) fn nav_link(theme: &ArtisanTheme, label: impl Into<SharedString>, active: bool) -> Div {
    let row = div()
        .flex()
        .flex_row()
        .items_center()
        .h(px(28.0))
        .px(px(8.0))
        .rounded(px(6.0))
        .gap(px(8.0))
        .text_sm()
        .cursor_pointer();
    if active {
        row.bg(theme.colors.muted.to_paint())
            .text_color(theme.colors.foreground.to_paint())
            .font_weight(FontWeight::MEDIUM)
            .child(label.into())
    } else {
        row.text_color(theme.colors.muted_foreground.to_paint())
            .child(label.into())
    }
}

/// The anchors under the open page's rail row, on the group rule.
pub(crate) fn anchor_list(theme: &ArtisanTheme, anchors: &[Anchor]) -> Div {
    if anchors.is_empty() {
        return div();
    }
    let mut list = div()
        .flex()
        .flex_col()
        .ml(px(15.0))
        .pl(px(12.0))
        .my(px(4.0))
        .border_l_1()
        .border_color(theme.colors.border.to_paint());
    for anchor in anchors {
        let selector = format!("settings-anchor-{}", anchor.hash);
        list = list.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .h(px(24.0))
                .px(px(8.0))
                .rounded(px(6.0))
                .text_xs()
                .text_color(theme.colors.muted_foreground.to_paint())
                .debug_selector(move || selector.clone())
                .child(anchor.label),
        );
    }
    list
}
