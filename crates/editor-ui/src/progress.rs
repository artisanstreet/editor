//! Determinate progress owns one clipped track and accessible value.
use crate::theme::{ArtisanTheme, RadiusStep, RadiusTokens};
use gpui::{
    App, Background, ElementId, Hsla, InteractiveElement as _, IntoElement, ParentElement as _,
    RenderOnce, StatefulInteractiveElement as _, Styled as _, Window, div, px, relative,
};

#[derive(IntoElement)]
pub struct Progress {
    id: ElementId,
    theme: ArtisanTheme,
    fraction: f32,
    fill: Option<Background>,
    track: Option<Hsla>,
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
            fill: None,
            track: None,
        }
    }

    /// Paints the filled share (a colour or gradient) and the track in a
    /// caller's tones, such as a gauge's, in place of the primary on accent 4.
    #[must_use]
    pub fn tones(mut self, fill: impl Into<Background>, track: Hsla) -> Self {
        self.fill = Some(fill.into());
        self.track = Some(track);
        self
    }
}
impl RenderOnce for Progress {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let fill = div()
            .h_full()
            .w(relative(self.fraction))
            .flex_shrink_0()
            // The track's overflow clip is rectangular, so the fill rounds itself.
            .rounded(RadiusTokens::value(RadiusStep::Xs))
            .bg(self
                .fill
                .unwrap_or_else(|| self.theme.colors.primary.to_paint().into()));
        div()
            .id(self.id)
            .role(gpui::Role::ProgressIndicator)
            .aria_label("Progress")
            .aria_value(format!("{:.0}%", self.fraction * 100.))
            .w_full()
            .h(px(8.))
            .rounded(RadiusTokens::value(RadiusStep::Xs))
            .overflow_hidden()
            .bg(self
                .track
                .unwrap_or_else(|| self.theme.colors.accent.step(4).to_paint()))
            .child(fill)
    }
}
