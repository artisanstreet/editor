//! The settings rail's own rows and anchor list, ported from the previous
//! Editor's `native_settings/render/chrome.rs`. Page headers, sections,
//! cards and rows are the shared `editor_ui::page` recipes.

use crate::page::Anchor;
use editor_ui::theme::{ArtisanTheme, RadiusStep, RadiusTokens};
use gpui::{
    Div, FontWeight, InteractiveElement as _, ParentElement as _, SharedString, Styled as _, div,
    px,
};

/// One rail row (`nav.svelte` `nav_link`): the open page wears the muted
/// well and the foreground at medium weight.
pub(crate) fn nav_link(theme: &ArtisanTheme, label: impl Into<SharedString>, active: bool) -> Div {
    let row = div()
        .flex()
        .flex_row()
        .items_center()
        .h(px(28.0))
        .px(px(8.0))
        .rounded(RadiusTokens::value(RadiusStep::Sm))
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
                .rounded(RadiusTokens::value(RadiusStep::Sm))
                .text_xs()
                .text_color(theme.colors.muted_foreground.to_paint())
                .debug_selector(move || selector.clone())
                .child(anchor.label),
        );
    }
    list
}
