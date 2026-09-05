//! Shared translucent surfaces for the native composer and model picker.
//!
//! The source surfaces use a separate backdrop filter, a translucent diagonal
//! material, and a non-interactive highlight layer. Keeping those pieces in
//! one small native helper prevents the picker and composer from drifting into
//! two different opaque approximations of the Electron glass treatment.

#![forbid(unsafe_code)]

use artisan_ui::theme::{ArtisanTheme, Oklch, SurfaceStep};
use gpui::{
    Background, BoxShadow, Div, Hsla, Pixels, Styled as _, div, linear_color_stop, linear_gradient,
    point, px,
};

/// The two strengths exposed by the Electron shader-glass surface.
///
/// Keeping strength in the shared helper makes it impossible for the picker,
/// its option menus, and the composer to silently drift into three different
/// material implementations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GlassStrength {
    /// The quiet treatment used by the composer surface.
    Quiet,
    /// The stronger treatment used by picker and tooltip surfaces.
    Strong,
}

/// The quiet blur radius from `shader-glass-backdrop`.
pub(crate) const GLASS_QUIET_BLUR_RADIUS_PX: f32 = 12.0;
/// The strong blur radius from `shader-glass-backdrop`.
pub(crate) const GLASS_STRONG_BLUR_RADIUS_PX: f32 = 20.0;

/// Resolves a source surface's backdrop blur radius.
#[must_use]
pub(crate) fn glass_blur_radius(strength: GlassStrength) -> Pixels {
    px(match strength {
        GlassStrength::Quiet => GLASS_QUIET_BLUR_RADIUS_PX,
        GlassStrength::Strong => GLASS_STRONG_BLUR_RADIUS_PX,
    })
}

/// Native-only foreground lift requested for the dark canvas.
///
/// Electron's shader-glass material does not add this base layer. Native uses
/// the theme foreground at 10% opacity underneath the translucent source
/// gradient so dark details remain legible while the backdrop stays visible.
/// This is deliberately local to glass surfaces and does not alter the global
/// theme palette.
#[must_use]
pub(crate) fn glass_foreground_base(theme: ArtisanTheme) -> Hsla {
    theme.colors.foreground.with_alpha(0.10).to_paint()
}

/// Builds the source `shader-glass-material` diagonal face.
///
/// `surface-600` and `surface-800` are the exact neutral ramp entries for
/// `rgb(82 82 91)` and `rgb(39 39 42)`. Their alpha is intentionally applied
/// before the GPUI paint conversion so the backdrop remains visible.
#[must_use]
pub(crate) fn glass_material(strength: GlassStrength) -> Background {
    let (from_alpha, to_alpha) = match strength {
        GlassStrength::Quiet => (0.20, 0.14),
        GlassStrength::Strong => (0.28, 0.20),
    };
    linear_gradient(
        145.0,
        linear_color_stop(
            SurfaceStep::S600.oklch().with_alpha(from_alpha).to_paint(),
            0.0,
        ),
        linear_color_stop(
            SurfaceStep::S800.oklch().with_alpha(to_alpha).to_paint(),
            1.0,
        ),
    )
}

/// Builds the source `shader-glass-highlight` face.
///
/// GPUI's gradient primitive has two stops, so the transparent stop is placed
/// at the strength-specific point from the source CSS. The renderer then keeps
/// that transparent color through the rest of the surface rather than painting
/// an opaque lower band over the backdrop.
#[must_use]
pub(crate) fn glass_highlight(strength: GlassStrength) -> Background {
    let (alpha, transparent_at) = match strength {
        GlassStrength::Quiet => (0.05, 0.42),
        GlassStrength::Strong => (0.08, 0.44),
    };
    let white = Oklch::new(1.0, 0.0, 0.0);
    linear_gradient(
        180.0,
        linear_color_stop(white.with_alpha(alpha).to_paint(), 0.0),
        linear_color_stop(white.with_alpha(0.0).to_paint(), transparent_at),
    )
}

/// Adds the visual-only material without installing any pointer handlers.
#[must_use]
pub(crate) fn glass_material_layer(strength: GlassStrength, radius: Pixels) -> Div {
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .rounded(radius)
        .bg(glass_material(strength))
}

/// Adds the visual-only highlight without installing any pointer handlers.
///
/// Native GPUI has no CSS `pointer-events` style; a plain, handler-free `Div`
/// therefore remains a paint layer while the interactive descendants retain
/// their existing hit testing and outside-dismiss behavior.
#[must_use]
pub(crate) fn glass_highlight_layer(strength: GlassStrength, radius: Pixels) -> Div {
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .rounded(radius)
        .bg(glass_highlight(strength))
}

/// Returns the source `card` shadow stack used by engine and option cards.
///
/// This is intentionally separate from [`glass_card_shadows`]. The source
/// engine strip is a regular card, not a card-glass surface; using the latter
/// there creates the doubled bright/dark rims visible in the native picker.
#[must_use]
pub(crate) fn card_shadows(theme: ArtisanTheme) -> Vec<BoxShadow> {
    theme
        .elevation
        .card_shadow
        .into_iter()
        .map(|layer| layer.to_box_shadow())
        .collect()
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
