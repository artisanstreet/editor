//! The multiline text field recipe the text editor paints with.

use gpui::{Hsla, Pixels, px};

use crate::theme::{ArtisanTheme, RadiusStep, RadiusTokens, SurfaceStep, ThemeMode};

const BORDER_WIDTH_PX: f32 = 1.0;
const DISABLED_OPACITY: f32 = 0.5;

/// Theme-resolved geometry and paint for one textarea state.
///
/// The `border`, `focus_border`, and `focus_ring` fields already include the
/// invalid-state decision.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TextareaStyle {
    /// Minimum control height: the legacy `min-h-16`, 64 px.
    pub min_height: Pixels,
    /// Horizontal control padding: the legacy `px-3`, 12 px.
    pub horizontal_padding: Pixels,
    /// Vertical control padding: the legacy `py-3`, 12 px.
    pub vertical_padding: Pixels,
    /// Rounded corner: the legacy `rounded-xl`, 14 px ramp step.
    pub corner_radius: Pixels,
    /// One-pixel textarea border.
    pub border_width: Pixels,
    /// Theme surface 100 in light mode and surface 900 in dark mode.
    pub background: Hsla,
    /// Normal value text color (`--foreground`).
    pub foreground: Hsla,
    /// Empty value text color (`--muted-foreground`).
    pub placeholder_foreground: Hsla,
    /// Current border, including the invalid-state branch.
    pub border: Hsla,
    /// Border color while the explicitly visible focus state is active.
    pub focus_border: Hsla,
    /// Ring color while the explicitly visible focus state is active.
    pub focus_ring: Hsla,
    /// Focus ring width: the legacy `[3px]` treatment.
    pub focus_ring_width: Pixels,
    /// Native desktop textarea text size (`md:text-sm`, 14 px).
    pub text_size: Pixels,
    /// Desktop textarea leading, 20 px.
    pub line_height: Pixels,
    /// Disabled presentation opacity from the legacy wrapper.
    pub disabled_opacity: f32,
    /// Whether this recipe was resolved for an invalid value.
    pub invalid: bool,
}

impl TextareaStyle {
    /// Resolves the reached textarea recipe from the shared theme tokens.
    #[must_use]
    pub fn resolve(theme: ArtisanTheme, invalid: bool) -> Self {
        let surface = match theme.mode {
            ThemeMode::Light => SurfaceStep::S100,
            ThemeMode::Dark => SurfaceStep::S900,
        };

        let (border, focus_border, focus_ring) = if invalid {
            let border = match theme.mode {
                ThemeMode::Light => theme.colors.destructive.to_paint(),
                ThemeMode::Dark => theme.colors.destructive.with_alpha(0.5).to_paint(),
            };
            let ring = match theme.mode {
                ThemeMode::Light => theme.interaction.invalid_ring_color.to_paint(),
                ThemeMode::Dark => theme.colors.destructive.with_alpha(0.4).to_paint(),
            };
            (border, border, ring)
        } else {
            (
                theme.colors.input.to_paint(),
                theme.colors.ring.to_paint(),
                theme.interaction.focus_ring_color.to_paint(),
            )
        };

        Self {
            min_height: theme.spacing.steps(16.0),
            horizontal_padding: theme.spacing.steps(3.0),
            vertical_padding: theme.spacing.steps(3.0),
            corner_radius: RadiusTokens::value(RadiusStep::Xl),
            border_width: px(BORDER_WIDTH_PX),
            background: theme.surfaces.value(surface).to_paint(),
            foreground: theme.colors.foreground.to_paint(),
            placeholder_foreground: theme.colors.muted_foreground.to_paint(),
            border,
            focus_border,
            focus_ring,
            focus_ring_width: theme.interaction.focus_ring_width,
            text_size: theme.typography.editor_text_desktop,
            line_height: px(20.0),
            disabled_opacity: DISABLED_OPACITY,
            invalid,
        }
    }
}

impl From<ArtisanTheme> for TextareaStyle {
    fn from(theme: ArtisanTheme) -> Self {
        Self::resolve(theme, false)
    }
}
