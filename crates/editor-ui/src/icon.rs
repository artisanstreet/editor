//! Themed catalog icons with shared size and tint recipes.
//! Monochrome icons inherit or resolve their tint; full-color artwork stays unchanged.
//! Interactive parents own accessible labels.

// The `Icon*` prefix is deliberate: these types are the crate's public icon
// vocabulary and read better fully qualified at call sites.
#![allow(clippy::module_name_repetitions)]

use editor_assets::AssetId;
use gpui::{Hsla, Pixels, Styled};

use crate::asset_seam::{AssetGlyph, Presentation, asset_glyph};
use crate::theme::ArtisanTheme;

/// Shared icon sizes.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum IconSize {
    /// Standard control icon, 16 px.
    #[default]
    Default,
    /// Compact control icon, 14 px.
    Compact,
}

impl IconSize {
    /// The shared 4 px spacing-token step count for this edge
    /// ([`crate::theme::SpacingTokens::BASE_PX`]).
    #[must_use]
    pub const fn spacing_steps(self) -> f32 {
        match self {
            Self::Default => 4.0,
            Self::Compact => 3.5,
        }
    }
}

/// Paint treatment resolved for a tintable (monochrome) glyph.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum IconTint {
    /// Inherit the current GPUI text color at paint time.
    #[default]
    Inherit,
    /// Use the theme muted foreground.
    Muted,
}

/// Resolved geometry and paint bound to one catalog asset.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IconStyle {
    /// Sealed catalog identity this recipe renders. Private: external code
    /// cannot construct an `IconStyle` (and therefore cannot pair a style
    /// with an unsealed or mismatched id); runtime strings reach an
    /// [`AssetId`] only through the catalog's validating lookup path.
    id: AssetId,
    /// Exact square edge derived from the shared spacing token.
    pub edge: Pixels,
    /// Catalog-derived pipeline the asset will render through.
    pub presentation: Presentation,
    /// Resolved explicit paint; `None` means "inherit ambient text color".
    /// Always `None` for [`Presentation::FullColor`] artwork regardless of
    /// the requested tint.
    pub color: Option<Hsla>,
}

impl IconStyle {
    /// Resolves the exact recipe for the sealed `id` from shared theme
    /// tokens and catalog metadata.
    ///
    /// The edge comes from the theme's own spacing token (never a local
    /// literal), and the pipeline observation goes through the seam's public
    /// [`asset_glyph`] constructor query, so style resolution and element
    /// construction can never disagree about the route.
    #[must_use]
    pub fn resolve(theme: ArtisanTheme, id: AssetId, size: IconSize, tint: IconTint) -> Self {
        let presentation = asset_glyph(id).presentation();
        let color = match (presentation, tint) {
            (Presentation::Tinted, IconTint::Muted) => {
                Some(theme.colors.muted_foreground.to_paint())
            }
            (Presentation::Tinted, IconTint::Inherit) | (Presentation::FullColor, _) => None,
        };
        Self {
            id,
            edge: theme.spacing.steps(size.spacing_steps()),
            presentation,
            color,
        }
    }

    /// Returns the sealed identity this recipe renders.
    #[must_use]
    pub const fn asset_id(&self) -> AssetId {
        self.id
    }
}

/// Builds the routed glyph element for the identity sealed in `style`.
///
/// There is deliberately no `(id, style)` form: the rendered asset always
/// comes from the recipe itself. The returned value styles like any GPUI
/// element (so call-site refinements such as `.opacity(0.5)` chain on top)
/// and drops into element trees as a child, exactly like [`asset_glyph`]
/// itself.
#[must_use]
pub fn icon(style: IconStyle) -> AssetGlyph {
    let mut glyph = asset_glyph(style.id).size(style.edge);
    if let Some(color) = style.color {
        glyph = glyph.text_color(color);
    }
    glyph
}
