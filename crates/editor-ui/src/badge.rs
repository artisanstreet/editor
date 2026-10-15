//! Noninteractive outline badges own their frame and typography.
use crate::theme::{ArtisanTheme, RadiusStep, RadiusTokens, SurfaceStep, ThemeMode};
use gpui::{
    App, FontWeight, IntoElement, ParentElement as _, RenderOnce, SharedString, Styled as _,
    Window, div,
};

#[derive(IntoElement)]
pub struct Badge {
    theme: ArtisanTheme,
    label: SharedString,
}
impl Badge {
    pub fn new(theme: ArtisanTheme, label: impl Into<SharedString>) -> Self {
        Self {
            theme,
            label: label.into(),
        }
    }
}
impl RenderOnce for Badge {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let theme = self.theme;
        let surface = match theme.mode {
            ThemeMode::Light => SurfaceStep::S100,
            ThemeMode::Dark => SurfaceStep::S900,
        };
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .h(theme.density.badge_height)
            .px(theme.spacing.steps(2.))
            .py(theme.spacing.steps(0.5))
            .rounded(RadiusTokens::value(RadiusStep::X4l))
            .border_1()
            .border_color(theme.colors.border.to_paint())
            .bg(surface.oklch().to_paint())
            .text_color(theme.colors.foreground.to_paint())
            .text_size(theme.typography.label_text)
            .line_height(theme.spacing.steps(4.))
            .font_weight(FontWeight::MEDIUM)
            .whitespace_nowrap()
            .overflow_hidden()
            .child(self.label)
    }
}
