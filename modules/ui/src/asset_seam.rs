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
//!   is transient — resolved fresh every frame and restored after the
//!   delegated paint returns normally — so a later parent recolor cannot go stale through
//!   a frozen refinement.
//! - **Full-color** assets use the local [`FullColorSvg`] element. It resolves
//!   the catalog bytes explicitly, parses each SVG once, and asks GPUI's public
//!   [`gpui::SvgRenderer`] to produce a bounded, size-specific raster at the
//!   actual laid-out glyph size. This avoids routing bare catalog keys through
//!   the general image loader, where they can be interpreted as URIs.
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
//! - Element-level color and layout behavior (alpha-mask tinting and sprite
//!   compositing) remains owned by pinned GPUI internals. Full-color SVG
//!   parsing and its bounded size-specific raster cache are the one deliberate
//!   exception, because intrinsic-size rasterization is too coarse for these
//!   small catalog marks.

use std::borrow::Cow;
use std::panic::Location;

use artisan_assets::AssetId;
use gpui::{
    App, AssetSource, Bounds, Element, ElementId, GlobalElementId, Hitbox, InspectorElementId,
    IntoElement, LayoutId, Pixels, SharedString, StyleRefinement, Styled, Svg, TextStyleRefinement,
    Window, point, px, size, svg,
};

#[path = "full_color_svg.rs"]
mod full_color_svg;
use self::full_color_svg::FullColorSvg;

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
    /// Full-color element backed by explicitly resolved catalog bytes.
    FullColor(FullColorSvg),
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
///   before delegating to its real [`Element::paint`], then restored
///   after normal return. Panic-unwind restoration is not guaranteed.
/// - An authored refinement (caller `.text_color(..)` or a resolved `Muted`
///   recipe) wins outright: it stays byte-exact DURING delegation as well
///   as afterward, and normal last-refinement-wins behavior is preserved.
/// - Any injected value is strictly transient. It lives only for the
///   duration of one delegated paint call and is re-resolved from the live
///   stack every pass, so recolored ancestors can never go stale through a
///   previously frozen value.
///
/// Paint fits the intrinsic catalog aspect inside the laid-out square without
/// touching layout or hitboxes: the tinted vendor route rasterizes
/// width-only (`SvgSize::Size` scales intrinsic height from the requested
/// width), so a tall `viewBox` such as Cursor `0 0 466.73 532.09` would paint
/// ~18.24 px tall in a 16 px box and center-overflow. The paint phase below
/// instead delegates the inner `Svg` with a centered `contain` rect derived
/// from catalog `view_box` metadata (see [`contained_tinted_bounds`]).
struct TintedSvg {
    svg: Svg,
    asset_id: AssetId,
}

/// Parses catalog `viewBox` metadata (`"min-x min-y width height"`) into
/// intrinsic dimensions. Returns `None` for absent, malformed, non-finite,
/// or non-positive values so callers fall back to the laid-out bounds
/// unchanged.
fn view_box_dims(view_box: Option<&str>) -> Option<(f32, f32)> {
    let mut parts = view_box?.split_whitespace();
    let _min_x: f32 = parts.next()?.parse().ok()?;
    let _min_y: f32 = parts.next()?.parse().ok()?;
    let width: f32 = parts.next()?.parse().ok()?;
    let height: f32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
        return None;
    }
    Some((width, height))
}

/// Contains the intrinsic catalog aspect inside `outer` and centers it,
/// preserving the laid-out box, hitbox, and origin symmetry.
///
/// Square artwork returns `outer` unchanged; tall artwork stays height-bound
/// (narrowed and horizontally centered), wide artwork stays width-bound.
/// Malformed or absent metadata returns `outer` unchanged.
///
/// Rounding note: the rect is exact in logical pixels; the vendor
/// `paint_svg` path snaps bounds and ceils the supersampled tile, so the
/// final sprite can exceed this rect by at most one device pixel
/// (~0.5 logical px at 1x) while staying centered — far below the ~2.24 px
/// tall overflow this replaces for a 16 px Cursor box.
fn contained_tinted_bounds(outer: Bounds<Pixels>, view_box: Option<&str>) -> Bounds<Pixels> {
    let Some((intrinsic_width, intrinsic_height)) = view_box_dims(view_box) else {
        return outer;
    };
    let outer_width = f32::from(outer.size.width);
    let outer_height = f32::from(outer.size.height);
    if !outer_width.is_finite()
        || !outer_height.is_finite()
        || outer_width <= 0.0
        || outer_height <= 0.0
    {
        return outer;
    }
    let intrinsic_ratio = intrinsic_width / intrinsic_height;
    let outer_ratio = outer_width / outer_height;
    let inner = if outer_ratio > intrinsic_ratio {
        size(px(outer_height * intrinsic_ratio), px(outer_height))
    } else {
        size(px(outer_width), px(outer_width / intrinsic_ratio))
    };
    Bounds::new(
        outer.origin
            + point(
                (outer.size.width - inner.width) / 2.0,
                (outer.size.height - inner.height) / 2.0,
            ),
        inner,
    )
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
    // The fork's `Styled::text_style` returns the live refinement directly
    // (an empty refinement means "absent slot").
    let authored = svg.text_style().color;

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
    svg.text_style().color = Some(window.text_style().color);

    let result = delegate(svg, window, cx);

    // Exact unwind of the temporary injection: an absent slot is reset to
    // the empty refinement again; a pre-existing colorless refinement keeps
    // its other authored properties while its color returns to None.
    if had_text_refinement {
        svg.text_style().color = None;
    } else {
        *svg.text_style() = TextStyleRefinement::default();
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
        // Layout, prepaint, and hitboxes keep the original square `bounds`;
        // only the delegated inner paint is contained to the intrinsic
        // catalog aspect so a tall viewBox cannot overflow its box.
        let inner = contained_tinted_bounds(bounds, artisan_assets::get(self.asset_id).view_box);
        with_scoped_tint_delegation(&mut self.svg, window, cx, |svg, window, cx| {
            svg.paint(
                global_id,
                inspector_id,
                inner,
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
            asset_id: id,
        })),
        Presentation::FullColor => AssetGlyph(GlyphRoute::FullColor(FullColorSvg::new(id))),
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
