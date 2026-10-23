//! Private field chrome consumed by the text editor.
use crate::theme::{ArtisanTheme, Opacity, RadiusStep, RadiusTokens};
use gpui::{Hsla, Pixels, px};

pub struct TextareaStyle {
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
    pub(crate) fn resolve(theme: &ArtisanTheme) -> Self {
        Self {
            horizontal_padding: theme.spacing.steps(3.),
            vertical_padding: theme.spacing.steps(3.),
            corner_radius: RadiusTokens::value(RadiusStep::Xl),
            border_width: px(1.),
            background: theme.colors.surface.to_paint(),
            placeholder_foreground: theme.colors.text_faint.to_paint(),
            border: theme.colors.line_strong.to_paint(),
            focus_border: theme.colors.ring.to_paint(),
            disabled_opacity: Opacity::Disabled.alpha(),
        }
    }
}
