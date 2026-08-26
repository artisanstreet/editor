//! Typed asset-presentation seam between the sealed [`artisan_assets`]
//! catalog and pinned `GPUI` 0.2.2.
//!
//! Static UI artwork enters GPUI through exactly two verified pipelines, and
//! the choice between them belongs to catalog metadata, never to callers:
//!
//! - **Tinted** assets rasterize through GPUI's alpha-mask `svg()`
//!   renderer. Pinned `Svg::paint` only paints when the *element's own*
//!   computed style carries a text color (`path.zip(style.text.color)`),
//!   while an ancestor's `.text_color(..)` lives solely on the Window
//!   text-style stack around child painting. The seam therefore wraps the
//!   inner `Svg` in a private delegating element (`TintedSvg`) that, at
//!   the paint phase, reads the *actual* resolved Window text color at that
//!   tree position and forwards it into the inner element's own computed
//!   style on the delegated render path.
//!   Explicit authored tints (a caller's
//!   `text_color` refinement or an [`IconStyle`](crate::icon::IconStyle)
//!   `Muted` recipe) always win and are never touched; the forwarded value
//!   is transient — resolved fresh every frame and unwound after the
//!   delegated paint — so a later parent recolor can never go stale through
//!   a frozen refinement.
//! - **Full-color** assets render through `img()` with an explicitly
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
//!   derived from `Asset::presentation` inside [`asset_glyph`] and cannot be
//!   forged from outside the catalog expansion.
//! - The adapter returns only `Ok` values. GPUI re-exports the result type
//!   its [`AssetSource`] signatures require (`gpui::Result`), and the
//!   stateless design never constructs an error, so no first-party `anyhow`
//!   dependency arises.
//! - Element-level color and layout behavior (alpha-mask tinting, sprite
//!   caching, SVG rasterization scale) is owned by pinned GPUI internals and
//!   is not reimplemented here.

use std::borrow::Cow;
use std::panic::Location;

use artisan_assets::AssetId;
use gpui::{
    App, AssetSource, Bounds, Element, ElementId, GlobalElementId, Hitbox, ImageSource, Img,
    InspectorElementId, IntoElement, LayoutId, Pixels, Resource, SharedString, StyleRefinement,
    Styled, Svg, TextStyleRefinement, Window, img, svg,
};

// White-box unit tests for this module's private tinted-route helper live
// externally in `tests/ui/tinted_svg.rs`; they are compiled as a child
// module of `asset_seam` only under `cfg(test)` (see the
// `tinted_svg_unit_test` target), which is what grants them access to the
// private items they exercise. No test implementation lives here.
#[cfg(test)]
#[path = "../../../tests/ui/tinted_svg.rs"]
mod tinted_svg_unit;

/// Which GPUI pipeline presents a cataloged asset.
///
/// The typed policy itself lives in the sealed catalog
/// (`artisan_assets::Presentation`, with the evidenced exceptions to the
/// monochrome default documented there); this re-export keeps the seam's
/// public surface self-contained. The choice for any given id is fixed by
/// catalog metadata: callers can observe it (tests, layout decisions) but
/// cannot construct or influence the route an id takes.
pub use artisan_assets::Presentation;

/// The already-routed GPUI element behind an [`AssetGlyph`].
///
/// Private by design: the branch is chosen from catalog metadata when the
/// glyph is constructed, and consumers reach it only through the styled
/// [`IntoElement`] conversion below.
enum GlyphRoute {
    /// Tintable alpha-mask element for a monochrome asset, wrapped so the
    /// ambient Window text color reaches the inner [`Svg`] at paint time.
    Tinted(TintedSvg),
    /// Full-color element backed by explicitly embedded bytes.
    FullColor(Img),
}

/// Delegating tinted-route element around the seam's alpha-mask [`Svg`].
///
/// Pinned GPUI 0.2.2 computes an element's style from its own refinements
/// only (`Interactivity::compute_style_internal`) and gates `Svg::paint`
/// behind `path.zip(style.text.color)`. An ancestor's `.text_color(..)`
/// refinement is pushed onto the Window text-style stack *around* child
/// painting, so a bare child `Svg` never observes it and silently skips
/// painting even though its parent renders colored text.
///
/// This wrapper closes exactly that gap through ONE unconditional private
/// delegation helper ([`with_scoped_tint_delegation`]), which owns the
/// whole per-pass behavior:
///
/// - `request_layout`, `prepaint`, element identity, and the source
///   location are forwarded unchanged, preserving layout, hitboxes, and
///   cross-frame element-state addressing.
/// - At `paint`, when the glyph carries **no** authored text color, the
///   actual resolved color of the Window text-style stack at this tree
///   position — including the resolved default text style when no ancestor
///   set one — is injected into the inner `Svg`'s own text refinement just
///   before delegating to its real [`Element::paint`], then unwound
///   afterwards.
/// - An authored refinement (caller `.text_color(..)` or a resolved `Muted`
///   recipe) wins outright: it stays byte-exact DURING delegation as well
///   as afterward, and normal last-refinement-wins behavior is preserved.
/// - Any injected value is strictly transient. It lives only for the
///   duration of one delegated paint call and is re-resolved from the live
///   stack every pass, so recolored ancestors can never go stale through a
///   previously frozen value.
struct TintedSvg {
    svg: Svg,
}

/// Scoped-delegation helper for the tinted route's delegated paint pass.
///
/// This is the ONE unconditional production path used by
/// `TintedSvg::paint`; it owns the whole per-pass behavior:
///
/// 1. Inspects the REAL inner [`Svg`]'s authored text-color refinement.
/// 2. When no authored color exists, resolves the ambient value directly
///    from the live `Window::text_style()` at this paint pass — never from
///    a caller-supplied surrogate — including the resolved default text
///    style when no ancestor refined one, and temporarily mutates the
///    actual inner slot. Authored own/caller/Muted colors are left
///    unchanged DURING delegation as well as afterward.
/// 3. Invokes `delegate` exactly once with that actual Svg and the native
///    contexts; `TintedSvg::paint` supplies a closure that unconditionally
///    delegates to the real `Svg::paint`.
/// 4. Restores the pre-call refinement exactly: an absent slot is removed
///    again, a pre-existing colorless refinement keeps its other authored
///    properties with its color back at [`None`], and a resolved ambient
///    value never becomes a frozen authored override on subsequent reuse.
///
/// The white-box suite in `tests/ui/tinted_svg.rs` (linked under
/// `cfg(test)`) inspects the actual inner refinement inside this exact
/// closure, so breaking the mutation, the precedence, or the restoration
/// fails executable assertions there.
fn with_scoped_tint_delegation<R>(
    svg: &mut Svg,
    window: &mut Window,
    cx: &mut App,
    delegate: impl FnOnce(&mut Svg, &mut Window, &mut App) -> R,
) -> R {
    // Authored inspection on the real slot decides the pass behavior.
    let authored = svg.text_style().as_ref().and_then(|text| text.color);

    if authored.is_some() {
        // Authored own/caller/Muted color wins outright: the slot already
        // carries it, opens the gate itself, and must remain byte-exact
        // through and after delegation. Delegate without touching anything.
        return delegate(svg, window, cx);
    }

    // Live resolved ambient, read from the Window right here at this paint
    // pass — including the resolved default text style when no ancestor
    // refined one.
    let had_text_refinement = svg.text_style().is_some();
    svg.text_style()
        .get_or_insert_with(TextStyleRefinement::default)
        .color = Some(window.text_style().color);

    let result = delegate(svg, window, cx);

    // Exact unwind of the temporary injection: an absent slot is removed
    // again; a pre-existing colorless refinement keeps its other authored
    // properties while its color returns to None.
    match (had_text_refinement, svg.text_style()) {
        (true, Some(text)) => text.color = None,
        _ => *svg.text_style() = None,
    }

    result
}

impl Element for TintedSvg {
    type RequestLayoutState = ();
    type PrepaintState = Option<Hitbox>;

    fn id(&self) -> Option<ElementId> {
        self.svg.id()
    }

    fn source_location(&self) -> Option<&'static Location<'static>> {
        self.svg.source_location()
    }

    fn request_layout(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        self.svg.request_layout(global_id, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.svg
            .prepaint(global_id, inspector_id, bounds, request_layout, window, cx)
    }

    fn paint(
        &mut self,
        global_id: Option<&GlobalElementId>,
        inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        // Single unconditional production path: the helper decides authored
        // versus live ambient, applies any temporary tint on the real inner
        // Svg, delegates exactly once via the closure below to the REAL
        // `Svg::paint`, and restores the prior refinement exactly.
        with_scoped_tint_delegation(&mut self.svg, window, cx, |svg, window, cx| {
            svg.paint(
                global_id,
                inspector_id,
                bounds,
                request_layout,
                prepaint,
                window,
                cx,
            );
        });
    }
}

impl IntoElement for TintedSvg {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Styled for TintedSvg {
    fn style(&mut self) -> &mut StyleRefinement {
        self.svg.style()
    }
}

/// One cataloged asset prepared for its metadata-derived GPUI pipeline.
///
/// Construct only through [`asset_glyph`]. The value styles like any GPUI
/// element (sizing for both routes, `text_color` tinting the monochrome
/// route) and drops into element trees as a child.
pub struct AssetGlyph(GlyphRoute);

/// Prepares `id` for presentation along its catalog-derived route.
///
/// Assets whose catalog policy is [`Presentation::Tinted`] rasterize through
/// the alpha-mask route and paint in GPUI text color;
/// [`Presentation::FullColor`] assets keep their authored colors over
/// explicitly embedded bytes. The policy is catalog metadata
/// (`artisan_assets::get(id).presentation`), not a caller argument.
#[must_use]
pub fn asset_glyph(id: AssetId) -> AssetGlyph {
    match artisan_assets::get(id).presentation {
        Presentation::Tinted => AssetGlyph(GlyphRoute::Tinted(TintedSvg {
            svg: svg().path(id.as_str()),
        })),
        Presentation::FullColor => {
            // Explicitly embedded: `img(key)` alone would misclassify the key
            // as a URI and attempt an HTTP fetch.
            AssetGlyph(GlyphRoute::FullColor(img(ImageSource::Resource(
                Resource::Embedded(SharedString::from(id.as_str())),
            ))))
        }
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
