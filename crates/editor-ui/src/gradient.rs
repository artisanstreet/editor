//! Shared native two-stop gradient fills. Angles follow CSS: 180 degrees points down.

use gpui::{Background, linear_color_stop, linear_gradient};

use crate::theme::{ArtisanTheme, Oklch, Opacity};

/// Top-to-bottom gradient angle.
pub const VERTICAL_ANGLE_DEGREES: f32 = 180.0;

/// Builds a two-stop gradient using GPUI interpolation.
#[must_use]
pub fn vertical_gradient(top: Oklch, bottom: Oklch) -> Background {
    linear_gradient(
        VERTICAL_ANGLE_DEGREES,
        linear_color_stop(top.to_paint(), 0.0),
        linear_color_stop(bottom.to_paint(), 1.0),
    )
}

/// Theme-resolved hover gradient: the text colour, translucent because the
/// pill sits over glass and varying surfaces. Elements take it through
/// [`crate::hover_card::hover_fill`]; charts paint it directly.
#[must_use]
pub fn hover_fill_gradient(theme: &ArtisanTheme) -> Background {
    let (top, bottom) = hover_fill_stops(theme, theme.colors.text);
    vertical_gradient(top, bottom)
}

/// The hover gradient's top and bottom stops in `ink` (the text colour, or
/// a tone of it in another hue).
#[must_use]
pub const fn hover_fill_stops(theme: &ArtisanTheme, ink: Oklch) -> (Oklch, Oklch) {
    let (strong, soft) = (ink.at(Opacity::HoverTop), ink.at(Opacity::HoverBottom));
    // Light falls from above: in dark mode the light text tint is strongest
    // at the top; in light mode the dark tint gathers at the bottom.
    if theme.colors.mode.is_dark() {
        (strong, soft)
    } else {
        (soft, strong)
    }
}
