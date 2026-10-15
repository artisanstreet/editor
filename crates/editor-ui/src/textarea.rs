//! Private field chrome consumed by the text editor.
use crate::theme::{ArtisanTheme, RadiusStep, RadiusTokens, SurfaceStep, ThemeMode};
use gpui::{Hsla, Pixels, px};

pub(crate) struct TextareaStyle {
    pub horizontal_padding: Pixels,
    pub vertical_padding: Pixels,
    pub corner_radius: Pixels,
    pub border_width: Pixels,
    pub background: Hsla,
    pub placeholder_foreground: Hsla,
    pub border: Hsla,
    pub focus_border: Hsla,
    pub disabled_opacity: f32,
}
impl TextareaStyle {
    pub(crate) fn resolve(theme: ArtisanTheme) -> Self {
        let surface = match theme.mode {
            ThemeMode::Light => SurfaceStep::S100,
            ThemeMode::Dark => SurfaceStep::S900,
        };
        Self {
            horizontal_padding: theme.spacing.steps(3.),
            vertical_padding: theme.spacing.steps(3.),
            corner_radius: RadiusTokens::value(RadiusStep::Xl),
            border_width: px(1.),
            background: surface.oklch().to_paint(),
            placeholder_foreground: theme.colors.muted_foreground.to_paint(),
            border: theme.colors.input.to_paint(),
            focus_border: theme.colors.ring.to_paint(),
            disabled_opacity: 0.5,
        }
    }
}
