//! Themed catalog-icon presentation for the first Artisan workflow.
//!
//! This module is the reusable sizing-and-tint layer over the audited
//! [`crate::asset_seam`]: the seam already owns *which* GPUI pipeline presents
//! a sealed catalog asset (alpha-mask `svg()` tinted by text color versus
//! full-color embedded `img()`), and this module owns the two presentation
//! decisions every reached call site makes on top of it — the square edge and
//! the paint color.
//!
//! Every value is transcribed from the documented first workflow (project
//! picker → thread list & creation → composer → messages/transcript;
//! `docs/ui/INVENTORY.md` §6) rather than invented:
//!
//! - **16 px edges** ([`IconSize::Default`], Tailwind `size-4`) are the
//!   control-content standard across the reached surfaces: the picker's
//!   selected-row check and New-project glyph
//!   (`routes/components/project-selector.svelte:212,238`), composer send /
//!   stop / new-thread glyphs (`composer/controls.svelte:140,170,172`),
//!   steering-lip edit/discard glyphs (`composer/steering-lip.svelte:43,53`),
//!   and the command-palette search addon
//!   (`lib/components/ui/command/command-input.svelte:31`). The shared button
//!   recipe pins the same 16 px edge for its catalog icons.
//! - **14 px edges** ([`IconSize::Compact`], Tailwind `size-3.5`) are reached
//!   where a glyph sits inside tighter chrome: the picker trigger chevron
//!   (`project-selector.svelte:151`) and the attachment remove glyph
//!   (`composer/attachment-tray.svelte:48`).
//! - **Muted tint** ([`IconTint::Muted`]) reproduces
//!   `text-muted-foreground` on decorative picker rows
//!   (`project-selector.svelte:151,212,236`); **inherit** keeps the legacy
//!   `currentColor` behavior where call sites let the surrounding control
//!   paint its own color (`controls.svelte:135–172`,
//!   `attachment-tray.svelte:44`).
//!
//! Multicolor artwork is never tinted: the route choice belongs to catalog
//! metadata (`docs/ui/ASSETS.md` §10 monochrome derivation; seam module
//! docs), so a requested tint resolves to `None` for full-color marks exactly
//! because alpha-mask tinting would corrupt brand artwork upstream.
//!
//! Deliberate limits:
//!
//! - No icon catalog, alias table, or dynamic loading exists here or in the
//!   seam; identity is always a sealed [`AssetId`], and [`IconStyle`] seals
//!   the id it was resolved for so a recipe can never be rendered against a
//!   different asset.
//! - No accessibility metadata lives on a bare icon: every reached first-
//!   workflow glyph is decorative (`aria-hidden="true"` at each cited site),
//!   and accessible names belong to the interactive controls that contain
//!   them (see [`crate::button::AccessibleLabel`]).
//! - Opacity treatments such as the 50%-alpha search addon are plain
//!   [`gpui::Styled`] refinements at the call site, not enum variants.

// The `Icon*` prefix is deliberate: these types are the crate's public icon
// vocabulary and read better fully qualified at call sites.
#![allow(clippy::module_name_repetitions)]

use artisan_assets::AssetId;
use gpui::{Hsla, Pixels, Styled};

use crate::asset_seam::{AssetGlyph, Presentation, asset_glyph};
use crate::theme::ArtisanTheme;

/// Square icon edge lengths reached by the documented first workflow.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum IconSize {
    /// Tailwind `size-4`: four steps of the shared spacing token, 16 px —
    /// the control-content default across every cited call surface.
    #[default]
    Default,
    /// Tailwind `size-3.5`: three-and-a-half steps, 14 px — reached only
    /// inside tighter chrome (picker trigger, attachment remove).
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
    /// Inherit the ambient GPUI text color, matching the legacy
    /// `currentColor` inheritance inside colored controls.
    #[default]
    Inherit,
    /// The mode-resolved `--muted-foreground` token used by decorative
    /// picker rows (`project-selector.svelte:151,212,236`).
    Muted,
}

/// Resolved paint and geometry for one icon configuration.
///
/// This record is public so behavioral tests and the future component gallery
/// can compare exact native semantics without reaching into GPUI internals,
/// mirroring [`crate::button::ButtonStyle`] and [`crate::card::CardStyle`].
///
/// The resolved asset identity is sealed inside the record: [`IconStyle`]
/// values are obtainable only through [`IconStyle::resolve`], and
/// [`icon`] renders exactly the id the recipe was resolved for, so a style
/// computed for one asset can never be paired with another.
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
