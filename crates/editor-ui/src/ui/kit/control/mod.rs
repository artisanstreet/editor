pub mod caption;
pub mod hit_area;
pub mod icon_button;
pub mod key;
pub mod link;
pub mod tabs;
pub mod trigger;

use crate::ui::theme::{ArtisanTheme, Opacity};
use aqua::ShadowLayer;
use aqua_ui::style::Style;

#[must_use]
pub fn focus_ring(theme: &ArtisanTheme) -> Style {
    Style::new()
        .border_of(1.0, theme.colors.ring.to_paint())
        .shadows(vec![ShadowLayer {
            dx: 0.0,
            dy: 0.0,
            blur: 0.0,
            spread: theme.interaction.focus_ring_width,
            inset: false,
            color: theme.colors.ring.at(Opacity::FocusRing).to_paint(),
        }])
}
