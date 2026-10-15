//! Determinate progress owns one clipped track and accessible value.
use crate::theme::{ArtisanTheme, RadiusStep, RadiusTokens};
use gpui::{
    App, ElementId, InteractiveElement as _, IntoElement, ParentElement as _, RenderOnce,
    StatefulInteractiveElement as _, Styled as _, Window, div, px, relative,
};

#[derive(IntoElement)]
pub struct Progress {
    id: ElementId,
    theme: ArtisanTheme,
    fraction: f32,
}
impl Progress {
    pub fn new(id: impl Into<ElementId>, theme: ArtisanTheme, fraction: f32) -> Self {
        Self {
            id: id.into(),
            theme,
            fraction: if fraction.is_finite() {
                fraction.clamp(0., 1.)
            } else {
                0.
            },
        }
    }
}
impl RenderOnce for Progress {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let fill = div()
            .h_full()
            .w(relative(self.fraction))
            .flex_shrink_0()
            .bg(self.theme.colors.primary.to_paint());
        div()
            .id(self.id)
            .role(gpui::Role::ProgressIndicator)
            .aria_label("Progress")
            .aria_value(format!("{:.0}%", self.fraction * 100.))
            .w_full()
            .h(px(8.))
            .rounded(RadiusTokens::value(RadiusStep::Xs))
            .overflow_hidden()
            .bg(self.theme.colors.primary.with_alpha(0.2).to_paint())
            .child(fill)
    }
}
