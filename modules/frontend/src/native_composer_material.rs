//! Shared translucent surfaces for the native composer and model picker.
//!
//! The source surfaces use a separate backdrop filter, a translucent diagonal
//! material, and a non-interactive highlight layer. Keeping those pieces in
//! one small native helper prevents the picker and composer from drifting into
//! two different opaque approximations of the Electron glass treatment.

#![forbid(unsafe_code)]

use artisan_ui::theme::{Oklch, SurfaceStep};
use gpui::{
    Background, BoxShadow, Div, Styled as _, div, linear_color_stop, linear_gradient, point, px,
};

/// The blur radius used by the Electron `shader-glass-backdrop` utility.
pub(crate) const GLASS_BLUR_RADIUS_PX: f32 = 12.0;

/// Builds the source `shader-glass-material` diagonal face.
///
/// `surface-600` and `surface-800` are the exact neutral ramp entries for
/// `rgb(82 82 91)` and `rgb(39 39 42)`. Their alpha is intentionally applied
/// before the GPUI paint conversion so the backdrop remains visible.
#[must_use]
pub(crate) fn glass_material() -> Background {
    linear_gradient(
        145.0,
        linear_color_stop(SurfaceStep::S600.oklch().with_alpha(0.20).to_paint(), 0.0),
        linear_color_stop(SurfaceStep::S800.oklch().with_alpha(0.14).to_paint(), 1.0),
    )
}

/// Builds the source `shader-glass-highlight` face.
///
/// GPUI's gradient primitive has two stops, so the transparent stop is placed
/// at the same 42% point as the source CSS. The renderer then keeps that
/// transparent color through the rest of the surface rather than painting an
/// opaque lower band over the backdrop.
#[must_use]
pub(crate) fn glass_highlight() -> Background {
    let white = Oklch::new(1.0, 0.0, 0.0);
    linear_gradient(
        180.0,
        linear_color_stop(white.with_alpha(0.05).to_paint(), 0.0),
        linear_color_stop(white.with_alpha(0.0).to_paint(), 0.42),
    )
}

/// Adds the visual-only highlight without installing any pointer handlers.
///
/// Native GPUI has no CSS `pointer-events` style; a plain, handler-free `Div`
/// therefore remains a paint layer while the interactive descendants retain
/// their existing hit testing and outside-dismiss behavior.
#[must_use]
pub(crate) fn glass_highlight_layer(radius: gpui::Pixels) -> Div {
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .rounded(radius)
        .bg(glass_highlight())
}

/// Returns the source `card-glass` shadow stack.
///
/// The inset entries are supported by the pinned GPUI fork. No fallback fill
/// is added here: the material remains translucent when the backdrop is
/// unavailable, and this stack only supplies the edge and elevation treatment.
#[must_use]
pub(crate) fn glass_card_shadows() -> Vec<BoxShadow> {
    let white = Oklch::new(1.0, 0.0, 0.0);
    let black = Oklch::new(0.0, 0.0, 0.0);
    vec![
        glass_shadow(white.with_alpha(0.14), -1.0, 0.0, 0.0, false),
        glass_shadow(white.with_alpha(0.10), 1.0, 0.0, 0.0, true),
        glass_shadow(black.with_alpha(0.22), -1.0, 0.0, 0.0, true),
        glass_shadow(black.with_alpha(0.70), 18.0, 48.0, -18.0, false),
        glass_shadow(white.with_alpha(0.08), 0.0, 0.0, 0.5, false),
    ]
}

fn glass_shadow(
    color: Oklch,
    offset_y: f32,
    blur_radius: f32,
    spread_radius: f32,
    inset: bool,
) -> BoxShadow {
    BoxShadow {
        color: color.to_paint(),
        offset: point(px(0.0), px(offset_y)),
        blur_radius: px(blur_radius),
        spread_radius: px(spread_radius),
        inset,
    }
}
