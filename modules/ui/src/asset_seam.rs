//! Typed asset-presentation seam between the sealed [`artisan_assets`]
//! catalog and pinned `GPUI` 0.2.2.
//!
//! Static UI artwork enters GPUI through exactly two verified pipelines, and
//! the choice between them belongs to catalog metadata, never to callers:
//!
//! - **Monochrome** assets rasterize through GPUI's alpha-mask `svg()`
//!   renderer and take their painted color from GPUI text color, so theme
//!   tinting stays an ordinary [`gpui::Styled`] refinement at the call site.
//! - **Multicolor** assets render through `img()` with an explicitly
//!   constructed [`ImageSource::Resource`] of the [`gpui::Resource::Embedded`]
//!   variant. The explicit variant matters: upstream `From<&str>`
//!   classifies bare catalog keys such as `"svgl.gitlab"` as URIs (they parse
//!   as `http_client::Uri`), which would send rendering off to HTTP instead
//!   of the embedded bytes.
//!
//! Both pipelines resolve their bytes through [`CatalogAssetSource`], the
//! ui-owned, stateless [`AssetSource`] installed during app assembly with
//! `Application::with_assets`. The adapter maps GPUI's path-keyed lookups
//! onto `artisan_assets::lookup`; arbitrary filesystem strings, unknown keys,
//! and empty inputs all resolve to `Ok(None)` exactly like the upstream `()`
//! source, and `list` reports nothing because GPUI has no in-tree caller for
//! it.
//!
//! Deliberate limits:
//!
//! - The public API accepts only [`AssetId`]. There is no string, path, or
//!   caller-supplied tint flag anywhere on the primary surface; the route is
//!   derived from `Asset::monochrome` inside [`asset_glyph`] and cannot be
//!   forged from outside the catalog expansion.
//! - The adapter returns only `Ok` values. GPUI re-exports the result type
//!   its [`AssetSource`] signatures require (`gpui::Result`), and the
//!   stateless design never constructs an error, so no first-party `anyhow`
//!   dependency arises.
//! - Element-level color and layout behavior (alpha-mask tinting, sprite
//!   caching, SVG rasterization scale) is owned by pinned GPUI internals and
//!   is not reimplemented here.

use std::borrow::Cow;

use artisan_assets::AssetId;
use gpui::{
    AssetSource, ImageSource, Img, IntoElement, Resource, SharedString, StyleRefinement, Styled,
    Svg, img, svg,
};

/// Which GPUI pipeline presents a cataloged asset.
///
/// Derived exclusively from catalog metadata; callers can observe the choice
/// (tests, layout decisions) but cannot construct or influence it.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Presentation {
    /// Alpha-masked `svg()` rendering, painted in GPUI text color.
    Tinted,
    /// Full-color `img()` rendering over explicitly embedded bytes.
    FullColor,
}

/// The already-routed GPUI element behind an [`AssetGlyph`].
///
/// Private by design: the branch is chosen from catalog metadata when the
/// glyph is constructed, and consumers reach it only through the styled
/// [`IntoElement`] conversion below.
enum GlyphRoute {
    /// Tintable alpha-mask element for a monochrome asset.
    Tinted(Svg),
    /// Full-color element backed by explicitly embedded bytes.
    FullColor(Img),
}

/// One cataloged asset prepared for its metadata-derived GPUI pipeline.
///
/// Construct only through [`asset_glyph`]. The value styles like any GPUI
/// element (sizing for both routes, `text_color` tinting the monochrome
/// route) and drops into element trees as a child.
pub struct AssetGlyph(GlyphRoute);

/// Prepares `id` for presentation along its catalog-derived route.
///
/// Monochrome assets (per `artisan_assets::get(id).monochrome`) take the
/// tinted alpha-mask route; everything else keeps its authored colors.
#[must_use]
pub fn asset_glyph(id: AssetId) -> AssetGlyph {
    if artisan_assets::get(id).monochrome {
        AssetGlyph(GlyphRoute::Tinted(svg().path(id.as_str())))
    } else {
        // Explicitly embedded: `img(key)` alone would misclassify the key as
        // a URI and attempt an HTTP fetch.
        AssetGlyph(GlyphRoute::FullColor(img(ImageSource::Resource(
            Resource::Embedded(SharedString::from(id.as_str())),
        ))))
    }
}

impl AssetGlyph {
    /// The pipeline derived for the presented asset.
    #[must_use]
    pub const fn presentation(&self) -> Presentation {
        match &self.0 {
            GlyphRoute::Tinted(_) => Presentation::Tinted,
            GlyphRoute::FullColor(_) => Presentation::FullColor,
        }
    }
}

impl Styled for AssetGlyph {
    fn style(&mut self) -> &mut StyleRefinement {
        match &mut self.0 {
            GlyphRoute::Tinted(element) => element.style(),
            GlyphRoute::FullColor(element) => element.style(),
        }
    }
}

impl IntoElement for AssetGlyph {
    type Element = gpui::AnyElement;

    fn into_element(self) -> Self::Element {
        match self.0 {
            GlyphRoute::Tinted(element) => element.into_any_element(),
            GlyphRoute::FullColor(element) => element.into_any_element(),
        }
    }
}

/// Stateless, ui-owned view of the sealed catalog as a GPUI asset source.
///
/// Install once during app assembly (`Application::with_assets`). Every
/// lookup is a direct `artisan_assets::lookup` binary search over embedded
/// `&'static str` sources, so results are borrowed, byte-identical, and
/// deterministic; unknown, empty, or filesystem-shaped keys return `Ok(None)`
/// rather than an error, mirroring the upstream `()` source.
#[derive(Clone, Copy, Debug, Default)]
pub struct CatalogAssetSource;

impl AssetSource for CatalogAssetSource {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        Ok(artisan_assets::lookup(path)
            .ok()
            .map(|asset| Cow::Borrowed(asset.source.as_bytes())))
    }

    fn list(&self, _path: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}
