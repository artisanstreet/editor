//! Neutral empty states; feature containers supply their placement.
use crate::theme::{ArtisanTheme, RadiusStep, RadiusTokens};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, IntoElement, ParentElement as _, RenderOnce, SharedString, Styled as _, Window, div, px,
};

#[derive(Clone, Copy, Default)]
pub enum EmptyStateVariant {
    #[default]
    Inline,
    Framed,
}
#[derive(IntoElement)]
pub struct EmptyState {
    theme: ArtisanTheme,
    text: SharedString,
    variant: EmptyStateVariant,
}
impl EmptyState {
    pub fn new(theme: ArtisanTheme, text: impl Into<SharedString>) -> Self {
        Self {
            theme,
            text: text.into(),
            variant: EmptyStateVariant::Inline,
        }
    }
    #[must_use]
    pub const fn variant(mut self, variant: EmptyStateVariant) -> Self {
        self.variant = variant;
        self
    }
}
impl RenderOnce for EmptyState {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let theme = self.theme;
        let framed = matches!(self.variant, EmptyStateVariant::Framed);
        div()
            .w_full()
            .min_w_0()
            .py(px(20.0))
            .px(px(12.0))
            .text_sm()
            .text_color(theme.colors.text_muted.to_paint())
            .whitespace_normal()
            .when(framed, |el| {
                el.min_h(px(72.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(RadiusTokens::value(RadiusStep::Lg))
                    .border_1()
                    .border_color(theme.colors.line.to_paint())
            })
            .child(self.text)
    }
}
