//! Shared page headers, sections, field rows, notices and navigation.

use crate::card::Card;
use crate::theme::{ArtisanTheme, RadiusStep, RadiusTokens, TextStep, TypographyTokens};
use gpui::{
    AnyElement, App, Div, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    RenderOnce, SharedString, Styled as _, Window, div, px,
};

/// Page recipes own their typography and geometry. Placement belongs to the parent.
#[derive(IntoElement)]
pub struct PageBlock {
    content: Div,
}
impl PageBlock {
    fn new(content: Div) -> Self {
        Self { content }
    }
    pub fn debug_selector(mut self, selector: impl Into<SharedString>) -> Self {
        let selector = selector.into();
        self.content = self.content.debug_selector(move || selector.to_string());
        self
    }
}
impl RenderOnce for PageBlock {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        self.content
    }
}

#[derive(IntoElement)]
pub struct Notice {
    content: Div,
}
impl Notice {
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.content = self.content.child(div().mt(px(8.0)).child(action));
        self
    }
    pub fn debug_selector(mut self, selector: impl Into<SharedString>) -> Self {
        let selector = selector.into();
        self.content = self.content.debug_selector(move || selector.to_string());
        self
    }
}
impl RenderOnce for Notice {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        self.content
    }
}

/// A bounded plain-text excerpt for external prompt variants and comparisons.
pub fn text_preview(theme: &ArtisanTheme, text: &str) -> PageBlock {
    PageBlock::new(
        div()
            .px(px(12.0))
            .py(px(10.0))
            .rounded(RadiusTokens::value(RadiusStep::Md))
            .bg(theme.colors.foreground.with_alpha(0.04).to_paint())
            .text_xs()
            .text_color(theme.colors.muted_foreground.to_paint())
            .whitespace_normal()
            .children(
                preview_lines(text)
                    .into_iter()
                    .map(|line| div().min_h(px(16.0)).child(line)),
            ),
    )
}

fn preview_lines(text: &str) -> Vec<String> {
    const MAX_BYTES: usize = 4096;
    const MAX_LINES: usize = 8;
    let prefix = &text[..text.floor_char_boundary(text.len().min(MAX_BYTES))];
    let mut lines = prefix.lines();
    let mut shown: Vec<_> = lines.by_ref().take(MAX_LINES).map(str::to_owned).collect();
    let truncated = prefix.len() < text.len() || lines.next().is_some();
    if shown.iter().all(|line| line.trim().is_empty()) {
        shown = vec!["(empty)".to_owned()];
    }
    if truncated {
        shown.push("…".to_owned());
    }
    shown
}

/// Maximum width of a settings or catalog page.
pub const CONTENT_MAX_WIDTH: f32 = 896.0;
/// Navigation rail width.
pub const NAV_RAIL_WIDTH: f32 = 176.0;
/// Gap between navigation and content.
pub const NAV_CONTENT_GAP: f32 = 56.0;

/// The page header: a 20 px title over a muted line.
pub fn header(
    theme: &ArtisanTheme,
    title: impl Into<SharedString>,
    description: impl Into<SharedString>,
) -> PageBlock {
    PageBlock::new(
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .text_size(TypographyTokens::size(TextStep::Xl))
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
            ),
    )
}

/// One section block: the heading with an optional action at its
/// baseline, an optional intro, then the body. Name it with
/// [`PageBlock::debug_selector`].
pub fn section_shell(
    theme: &ArtisanTheme,
    title: &str,
    intro: Option<&str>,
    action: Option<AnyElement>,
    body: impl IntoElement,
) -> PageBlock {
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
    let mut section = div().flex().flex_col().mt(px(40.0)).child(head);
    if let Some(intro) = intro {
        section = section.child(
            div()
                .mt(px(4.0))
                .text_sm()
                .text_color(theme.colors.muted_foreground.to_paint())
                .child(intro.to_owned()),
        );
    }
    PageBlock::new(section.child(div().mt(px(12.0)).child(body)))
}

/// One card: the compact card with a hairline between
/// its bands.
pub fn card(theme: &ArtisanTheme, blocks: Vec<AnyElement>) -> Card {
    Card::bands(*theme, blocks)
}

/// One row: a title over a description, and an optional
/// trailing control.
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
    PageBlock::new(row)
}

/// A muted 12 px line under a section.
pub fn footnote(theme: &ArtisanTheme, text: impl Into<SharedString>) -> PageBlock {
    PageBlock::new(
        div()
            .mt(px(10.0))
            .text_xs()
            .text_color(theme.colors.muted_foreground.to_paint())
            .child(text.into()),
    )
}

/// A muted line inside a card band.
pub fn note(theme: &ArtisanTheme, text: impl Into<SharedString>) -> PageBlock {
    PageBlock::new(
        div()
            .w_full()
            .py(px(12.0))
            .text_sm()
            .text_color(theme.colors.muted_foreground.to_paint())
            .child(text.into()),
    )
}

/// What failed and why, in the destructive colour.
pub fn error_text(theme: &ArtisanTheme, text: impl Into<SharedString>) -> PageBlock {
    PageBlock::new(
        div()
            .w_full()
            .py(px(12.0))
            .text_sm()
            .text_color(theme.colors.destructive.to_paint())
            .child(text.into()),
    )
}

/// The rail's `Settings` heading.
pub fn nav_heading(theme: &ArtisanTheme) -> PageBlock {
    PageBlock::new(
        div()
            .px(px(8.0))
            .pb(px(12.0))
            .text_sm()
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme.colors.foreground.to_paint())
            .child("Settings"),
    )
}

/// One uppercase group label (`ARTISAN`, `ENGINES`).
pub fn nav_group_label(theme: &ArtisanTheme, label: &'static str) -> PageBlock {
    PageBlock::new(
        div()
            .px(px(8.0))
            .pt(px(20.0))
            .pb(px(6.0))
            .text_xs()
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme.colors.muted_foreground.to_paint())
            .child(label),
    )
}

/// An application warning or failed operation, with shared notice geometry.
pub fn notice(theme: &ArtisanTheme, text: impl Into<SharedString>) -> Notice {
    let tone = theme.colors.destructive;
    Notice {
        content: div()
            .mt(px(24.0))
            .px(px(12.0))
            .py(px(10.0))
            .rounded(RadiusTokens::value(RadiusStep::Lg))
            .bg(tone.with_alpha(0.10).to_paint())
            .text_sm()
            .text_color(tone.to_paint())
            .child(text.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::button::Button;
    use gpui::{Context, Render, TestAppContext};
    struct Page;
    impl Render for Page {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let theme = ArtisanTheme::resolved();
            div()
                .w_full()
                .flex()
                .flex_col()
                .child(
                    row(
                        &theme,
                        "Setting",
                        "An explanation",
                        Some(
                            div()
                                .w(px(144.0))
                                .child(
                                    Button::text("choice", theme, "Choose")
                                        .debug_selector("page-control"),
                                )
                                .into_any_element(),
                        ),
                    )
                    .debug_selector("page-field"),
                )
                .child(
                    notice(&theme, "Could not load this page.")
                        .action(Button::text("retry", theme, "Retry").debug_selector("page-retry"))
                        .debug_selector("page-notice"),
                )
        }
    }
    #[gpui::test]
    fn field_controls_stay_beside_their_text_and_notices_keep_actions_inside(
        cx: &mut TestAppContext,
    ) {
        let (_, cx) = cx.add_window_view(|_, _| Page);
        for width in [240.0, 480.0] {
            cx.simulate_resize(gpui::size(px(width), px(400.0)));
            let field = cx.debug_bounds("page-field").unwrap();
            let control = cx.debug_bounds("page-control").unwrap();
            assert_eq!(field.size.width, px(width));
            assert!(
                control.top() >= field.top() && control.bottom() <= field.bottom(),
                "the control shares the row with its text at {width} px"
            );
            assert_eq!(control.right(), field.right(), "the control ends the row");
        }
        let notice = cx.debug_bounds("page-notice").unwrap();
        let retry = cx.debug_bounds("page-retry").unwrap();
        assert!(retry.left() >= notice.left() && retry.right() <= notice.right());
        assert!(retry.bottom() <= notice.bottom());
    }
}

#[cfg(test)]
mod preview_tests {
    use super::preview_lines;
    #[test]
    fn large_external_text_has_a_bounded_utf8_preview() {
        let long_line = "🎨".repeat(100_000);
        let shown = preview_lines(&long_line);
        assert_eq!(shown.len(), 2);
        assert_eq!(shown[0].len(), 4096);
        assert_eq!(shown[1], "…");
        let many_lines = "first\n".repeat(100_000);
        let shown = preview_lines(&many_lines);
        assert_eq!(shown.len(), 9);
        assert_eq!(shown.last().unwrap(), "…");
        assert_eq!(preview_lines(" \n\t"), ["(empty)"]);
        assert_eq!(preview_lines("a\nb"), ["a", "b"]);
    }
}
