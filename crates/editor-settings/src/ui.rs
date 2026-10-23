//! The settings pages' own recipes. Pages sit on the shell's true black in
//! one centred column; each group of settings is a quiet-glass card, the
//! face code cards wear in the transcript, with glass hairlines between
//! its rows.

use editor_ui::glass::{
    GlassStrength, glass_card_shadows, glass_foreground_base, glass_highlight_layer,
    glass_material_layer, glass_menu_line,
};
pub use editor_ui::page::PageBlock;
use editor_ui::theme::{
    ArtisanTheme, Elevation, RadiusStep, RadiusTokens, TextStep, TypographyTokens,
};
use gpui::{
    AnyElement, Div, FontWeight, IntoElement, ParentElement as _, SharedString, Styled as _, div,
    px,
};

/// The page column's widest; settings read as one narrow form.
pub const COLUMN_MAX_WIDTH: f32 = 680.0;
/// Space between sections.
const SECTION_GAP: f32 = 44.0;
/// A card's inner horizontal padding.
const CARD_INSET: f32 = 18.0;

/// The page's title over a muted line.
pub fn header(
    theme: &ArtisanTheme,
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
) -> PageBlock {
    PageBlock::new(
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .text_size(TypographyTokens::size(TextStep::Xl))
                    .line_height(px(28.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.colors.text.to_paint())
                    .child(title.into()),
            )
            .child(
                div()
                    .text_size(TypographyTokens::size(TextStep::Sm))
                    .line_height(px(20.0))
                    .text_color(theme.colors.text_muted.to_paint())
                    .child(description.into()),
            ),
    )
}

/// One section: its heading (with an action at the right), an optional
/// intro, then the body. Name it with [`PageBlock::debug_selector`].
pub fn section_shell(
    theme: &ArtisanTheme,
    title: &str,
    intro: Option<&str>,
    action: Option<AnyElement>,
    body: impl IntoElement,
) -> PageBlock {
    let mut heading = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .gap(px(4.0))
        .child(
            div()
                .text_size(TypographyTokens::size(TextStep::Sm))
                .line_height(px(20.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.colors.text.to_paint())
                .child(title.to_owned()),
        );
    if let Some(intro) = intro {
        heading = heading.child(
            div()
                .text_size(TypographyTokens::size(TextStep::XsMid))
                .line_height(px(18.0))
                .text_color(theme.colors.text_muted.to_paint())
                .child(intro.to_owned()),
        );
    }
    let head = div()
        .flex()
        .flex_row()
        .items_end()
        .justify_between()
        .gap(px(16.0))
        .px(px(4.0))
        .child(heading)
        .children(action.map(|action| div().flex_shrink_0().child(action)));
    PageBlock::new(
        div()
            .flex()
            .flex_col()
            .mt(px(SECTION_GAP))
            .gap(px(14.0))
            .child(head)
            .child(body),
    )
}

/// One group of settings: a quiet-glass card with a hairline between its
/// bands.
pub fn card(theme: &ArtisanTheme, blocks: Vec<AnyElement>) -> Div {
    let radius = RadiusTokens::value(RadiusStep::X2l);
    let mut card = div()
        .relative()
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .rounded(radius)
        .bg(glass_foreground_base(theme))
        .shadow(glass_card_shadows(theme, Elevation::Raised))
        .text_color(theme.colors.text.to_paint())
        .child(glass_material_layer(theme, GlassStrength::Quiet, radius))
        .child(glass_highlight_layer(theme, GlassStrength::Quiet, radius));
    for (index, block) in blocks.into_iter().enumerate() {
        if index > 0 {
            card = card.child(
                div()
                    .mx(px(CARD_INSET))
                    .h(px(1.0))
                    .bg(glass_menu_line(theme)),
            );
        }
        card = card.child(div().relative().w_full().px(px(CARD_INSET)).child(block));
    }
    card
}

/// One setting: its name over what it does, and its control at the right.
pub fn row(
    theme: &ArtisanTheme,
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
    control: Option<AnyElement>,
) -> PageBlock {
    let description = description.into();
    let mut text = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .gap(px(3.0))
        .child(
            div()
                .text_size(TypographyTokens::size(TextStep::Sm))
                .line_height(px(20.0))
                .text_color(theme.colors.text.to_paint())
                .child(title.into()),
        );
    if !description.is_empty() {
        text = text.child(
            div()
                .text_size(TypographyTokens::size(TextStep::XsMid))
                .line_height(px(18.0))
                .text_color(theme.colors.text_muted.to_paint())
                .child(description),
        );
    }
    PageBlock::new(
        div()
            .w_full()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(32.0))
            .py(px(16.0))
            .child(text)
            .children(control.map(|control| div().flex_shrink_0().child(control))),
    )
}

/// A muted line inside a card.
pub fn note(theme: &ArtisanTheme, text: impl Into<SharedString>) -> PageBlock {
    PageBlock::new(
        div()
            .w_full()
            .py(px(16.0))
            .text_size(TypographyTokens::size(TextStep::Sm))
            .line_height(px(20.0))
            .text_color(theme.colors.text_muted.to_paint())
            .child(text.into()),
    )
}

/// What failed and why, in the destructive colour, under the card it
/// belongs to.
pub fn error_text(theme: &ArtisanTheme, text: impl Into<SharedString>) -> PageBlock {
    PageBlock::new(
        div()
            .w_full()
            .px(px(4.0))
            .text_size(TypographyTokens::size(TextStep::XsMid))
            .line_height(px(18.0))
            .text_color(theme.colors.destructive_text.to_paint())
            .child(text.into()),
    )
}
