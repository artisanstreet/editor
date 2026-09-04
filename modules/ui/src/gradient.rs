//! First-party linear-gradient fills over GPUI's native paint primitive.
//!
//! The wave brief assumed pinned GPUI 0.2.2 had no gradient primitive; it
//! does: [`gpui::linear_gradient`] builds a two-stop [`gpui::Background`]
//! painted by the platform shaders on every backend (Blade, Windows, Metal:
//! `gradient_color` in `shaders.wgsl`, `shaders.hlsl`, `shaders.metal`), and
//! [`gpui::Styled::bg`] accepts it through `impl Into<gpui::Fill>`
//! (`styled.rs:372–379`, `style.rs:847–851`). No layered-fill approximation
//! is needed — every legacy gradient this wave reproduces is exactly
//! two-stop, which is all the primitive supports (a third stop would need a
//! different technique; none of the audited surfaces uses one).
//!
//! Angle convention is CSS (`color.rs:758–776`, MDN `linear-gradient`): `0.0`
//! points up (`to top`), so a top-to-bottom face — the legacy `to bottom` —
//! is `180.0`, with stops at `0.0` (top) and `1.0` (bottom). These helpers
//! are strictly additive: existing widgets keep their flat fills and adopt a
//! gradient by chaining `.bg(…)` over the returned value.

use gpui::{Background, linear_color_stop, linear_gradient};

use crate::theme::{ArtisanTheme, Oklch, SurfaceStep};

/// Gradient-line angle for a top-to-bottom face (legacy `to bottom`).
pub const VERTICAL_ANGLE_DEGREES: f32 = 180.0;

/// Builds a top-to-bottom two-stop gradient from legacy source colors.
///
/// Stops sit at the face edges (`0.0` top, `1.0` bottom); interpolation runs
/// in the primitive's Oklab space (`ColorSpace::Oklab` in gpui-ce's
/// `linear_gradient`), which keeps the mid-stop more vibrant than the legacy
/// sRGB-interpolated CSS faces — an intended fidelity gain, not a regression.
#[must_use]
pub fn vertical_gradient(top: Oklch, bottom: Oklch) -> Background {
    linear_gradient(
        VERTICAL_ANGLE_DEGREES,
        linear_color_stop(top.to_paint(), 0.0),
        linear_color_stop(bottom.to_paint(), 1.0),
    )
}

/// Home-panel card face: `from-surface-125 to-surface-75`, top to bottom.
///
/// Both stops are mode-independent ramp constants, so this face is identical
/// in light and dark themes.
#[must_use]
pub fn home_panel_gradient(theme: ArtisanTheme) -> Background {
    let _ = theme;
    vertical_gradient(SurfaceStep::S125.oklch(), SurfaceStep::S75.oklch())
}

/// Hover-pill face: the named `--hover-surface-fill` vertical gradient
/// (`theme.css:244–248`), resolved against this theme's foreground.
///
/// This is the same pair [`crate::theme::InteractionTokens`] records as
/// `hover_fill_top`/`hover_fill_bottom`; prefer this constructor wherever a
/// background fill (rather than token comparison) is needed.
#[must_use]
pub fn hover_fill_gradient(theme: ArtisanTheme) -> Background {
    vertical_gradient(
        theme.interaction.hover_fill_top,
        theme.interaction.hover_fill_bottom,
    )
}

/// Composer send-button resting face: `surface-25 → surface-100`, top to
/// bottom (`utilities.css:1121`). Mode-independent ramp constants, like the
/// home-panel face.
#[must_use]
pub fn send_face_gradient(theme: ArtisanTheme) -> Background {
    let _ = theme;
    vertical_gradient(SurfaceStep::S25.oklch(), SurfaceStep::S100.oklch())
}

// The wave-2 fidelity suite lives in `tests/ui/` and is compiled here as a
// white-box child under `cfg(test)` — the same linkage `asset_seam.rs`
// uses for `tinted_svg.rs` — so it can reach these helpers through the
// crate without a Cargo manifest change (which is outside this wave).
#[cfg(test)]
#[path = "../../../tests/ui/typography_gradient.rs"]
mod typography_gradient_test;
