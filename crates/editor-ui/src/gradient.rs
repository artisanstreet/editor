//! Shared native two-stop gradient fills. Angles follow CSS: 180 degrees points down.

use gpui::{Background, linear_color_stop, linear_gradient};

use crate::theme::{ArtisanTheme, Oklch};

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

/// Theme-resolved hover gradient.
#[must_use]
pub fn hover_fill_gradient(theme: ArtisanTheme) -> Background {
    vertical_gradient(
        theme.interaction.hover_fill_top,
        theme.interaction.hover_fill_bottom,
    )
}
