//! External coverage for the Phase 7 GPUI asset-presentation seam in
//! `artisan_ui`.
//!
//! These tests exercise only the public `artisan_ui::asset_seam` API and pin
//! what this packet requires along two axes. Statically: catalog presentation
//! policy routing (including the authored single-hue brand marks preserved
//! despite monochrome artwork) over representative ids and the whole
//! catalog, adapter round trips with borrowed byte identity and determinism,
//! rejection of unknown, empty, and path-shaped inputs, empty `list`
//! behavior, and why the multicolor route must construct
//! `ImageSource::Resource(Resource::Embedded(..))` explicitly instead of
//! letting upstream string conversion classify catalog keys as URIs.
//! Dynamically: in-memory window lifecycle probes (`#[gpui::test]`) observe
//! ambient Window text-style inputs at the paint phase: nested nearest-wins,
//! cross-frame recoloring on a persistent view, and resolved defaults without
//! ancestors. These sibling probes do not observe the glyph's forwarded color;
//! private tests in `tinted_svg.rs` assert the actual inner Svg slot during
//! delegation and its restoration after normal return. Routing assertions
//! separately pin the full-color policy. The shared `asset_glyph` foundation is exercised
//! directly (the exact call shape `button.rs` uses), not only through the
//! icon recipe layer.

use std::borrow::Cow;
use std::cell::RefCell;
use std::rc::Rc;

use artisan_assets::{AssetId, get};
use artisan_ui::asset_seam::{CatalogAssetSource, Presentation, asset_glyph};
use artisan_ui::icon::{IconSize, IconStyle, IconTint};
use artisan_ui::theme::{ArtisanTheme, ThemeMode};
use gpui::{
    AssetSource, Canvas, Context, Hsla, ImageSource, IntoElement, ParentElement, Render, Resource,
    Styled, TestAppContext, TextStyle, Window, black, canvas, div, px,
};

#[test]
fn routes_representative_ids_by_catalog_metadata() {
    // A Tabler glyph is monochrome (`stroke="currentColor"`): tinted route.
    assert_eq!(
        asset_glyph(AssetId::TABLER_CHECK).presentation(),
        Presentation::Tinted
    );
    // GitLab carries four brand fills that alpha-mask tinting would flatten:
    // full-color route.
    assert_eq!(
        asset_glyph(AssetId::SVGL_GITLAB).presentation(),
        Presentation::FullColor
    );
    // The gradient app icon is multicolor first-party artwork.
    assert_eq!(
        asset_glyph(AssetId::ARTISAN_APP_ICON).presentation(),
        Presentation::FullColor
    );
}

#[test]
fn authored_single_hue_brand_marks_keep_full_color_despite_monochrome_artwork() {
    // Legacy evidence (docs/ui/ASSETS.md §10, engine/presentation.ts): engine
    // `claude` and provider `anthropic` flag `SvglClaudeAILogo` non-inverting,
    // and provider `deepseek` flags `SvglDeepSeekLogo` non-inverting. Both
    // artworks are single-paint (#D97757 / #4D6BFE), so the structural
    // `monochrome` property stays true; presentation policy is independent of
    // it and preserves the authored colors through the FullColor route.
    for id in [AssetId::SVGL_CLAUDE_AI, AssetId::SVGL_DEEPSEEK] {
        let asset = get(id);
        assert!(
            asset.monochrome,
            "{}: artwork-derived monochrome must stay true",
            id.as_str()
        );
        assert_eq!(
            asset.presentation,
            Presentation::FullColor,
            "{}: catalog policy must preserve the authored brand color",
            id.as_str()
        );
        assert_eq!(
            asset_glyph(id).presentation(),
            Presentation::FullColor,
            "{}: seam must route the authored-color mark full-color",
            id.as_str()
        );
    }

    // Control: an ordinary Tabler glyph shares `monochrome == true` with the
    // exceptions yet stays tinted — proving the two properties diverge.
    assert!(get(AssetId::TABLER_CHECK).monochrome);
    assert_eq!(
        asset_glyph(AssetId::TABLER_CHECK).presentation(),
        Presentation::Tinted
    );
}

#[test]
fn derives_the_route_for_every_catalog_id_from_presentation_metadata() {
    // The seam observes `Asset::presentation`; it never re-derives policy
    // from artwork structure. Every one of the 104 ids routes exactly as its
    // catalog record says.
    assert_eq!(AssetId::CONSTANTS.len(), 104);
    for id in AssetId::CONSTANTS {
        assert_eq!(
            asset_glyph(*id).presentation(),
            get(*id).presentation,
            "route for `{}` must come from Asset::presentation",
            id.as_str()
        );
    }
}

#[test]
fn adapter_round_trips_every_catalog_key_to_identical_embedded_bytes() {
    let source = CatalogAssetSource;

    for id in AssetId::CONSTANTS {
        let first = source
            .load(id.as_str())
            .expect("catalog loads never error")
            .expect("every catalog key must resolve");
        assert!(
            matches!(&first, Cow::Borrowed(_)),
            "loads of `{}` must borrow the embedded catalog bytes",
            id.as_str()
        );
        assert_eq!(first.as_ref(), get(*id).source.as_bytes());

        let second = source
            .load(id.as_str())
            .expect("catalog loads never error")
            .expect("every catalog key must resolve");
        assert_eq!(
            first,
            second,
            "loads of `{}` are deterministic",
            id.as_str()
        );
    }
}

#[test]
fn adapter_rejects_unknown_empty_and_path_shaped_inputs() {
    let source = CatalogAssetSource;

    for rejected in [
        "",
        "nope",
        "tabler",
        "tabler.check.svg",
        "svg/tabler/check.svg",
        "../secrets",
        "/etc/passwd",
        "C:\\evil.png",
    ] {
        let resolved = source
            .load(rejected)
            .expect("unknown input still cannot error");
        assert!(
            resolved.is_none(),
            "`{rejected}` must not resolve through the sealed catalog"
        );
    }
}

#[test]
fn adapter_lists_nothing_for_any_path() {
    let source = CatalogAssetSource;

    for path in ["", "/", "assets", "tabler", "tabler.check"] {
        let entries = source.list(path).expect("catalog listing never errors");
        assert!(entries.is_empty(), "`list(\"{path}\")` must stay empty");
    }
}

#[test]
fn bare_string_keys_misclassify_as_uris_but_explicit_embedding_resolves() {
    // Upstream `From<&str>` parses catalog keys as URIs; pinned verbatim so a
    // future GPUI bump that changes this trap surfaces here first.
    let bare: ImageSource = AssetId::SVGL_GITLAB.as_str().into();
    assert!(
        matches!(bare, ImageSource::Resource(Resource::Uri(_))),
        "upstream string conversion must keep misclassifying catalog keys"
    );

    // The seam's contract construction: explicit Embedded classification.
    let explicit = ImageSource::Resource(Resource::Embedded(AssetId::SVGL_GITLAB.as_str().into()));
    assert!(
        matches!(
            &explicit,
            ImageSource::Resource(Resource::Embedded(key))
                if key.as_ref() == AssetId::SVGL_GITLAB.as_str()
        ),
        "the multicolor route must construct Resource::Embedded explicitly"
    );

    let bytes = CatalogAssetSource
        .load(AssetId::SVGL_GITLAB.as_str())
        .expect("embedded loads never error")
        .expect("the embedded key must resolve through the adapter");
    assert_eq!(bytes.as_ref(), get(AssetId::SVGL_GITLAB).source.as_bytes());
}

// ---------------------------------------------------------------------------
// Ambient-color regressions.
//
// Production decision under test. The tinted route applies ONE unconditional
// private scoped-delegation helper (`with_scoped_tint_delegation`) at the
// paint phase: it inspects the real inner Svg's authored text-color
// refinement, reads the live resolved ambient straight from
// `Window::text_style()` when none exists, temporarily mutates the inner
// slot — the only place pinned GPUI reads (`path.zip(style.text.color)`) —
// invokes a closure delegating to real `Svg::paint`, and restores the prior
// refinement exactly.
//
// Executable layers:
//
// 1. White-box private-unit suite (`tests/ui/tinted_svg.rs`, linked under
//    `cfg(test)` as the `tinted_svg_unit_test` target): asserts INSIDE the
//    production helper's own delegation closure against the real inner Svg
//    slot — live ambient mutation, delegation invocation, authored/Muted/
//    caller precedence (unchanged during and after), exact restoration
//    (absent / colorless-with-other-properties / authored), and
//    SAME-instance changed-ambient reuse, plus a no-panic lifecycle smoke.
//    Deleting the helper's mutation fails assertions there directly.
// 2. Real-frame dynamics (`recolored_parents_repaint_the_same_view...`):
//    every post-recolor paint-phase observation resolves the updated
//    ambient input color. This does not observe the glyph's own refinement.
// 3. Input/lifecycle probes (`canvas` siblings) and geometry bounds: pin
//    that the resolved ambient value is live AT THE PAINT PHASE at the
//    glyph's tree position and that layout boxes are unchanged. These are
//    INPUT evidence only: they would still pass if the glyph were removed,
//    so they never stand alone as forwarding proof, and they never observe
//    forwarded SVG color.
//
// Honest limitations. Pinned GPUI 0.2.2 seals every primitive-level
// observable (frame scene, sprite atlas, harness draw hook are
// crate-private; the test App hard-wires its asset source to `()`), so no
// executable check can read the forwarded color out of an actual
// `MonochromeSprite`. The private suite observes the real inner Svg slot
// inside the production helper, verifies restoration after normal return,
// and reuses one underlying adapter across separate lifecycle draws. Removing
// the injection fails those assertions. Static inspection verifies that
// `TintedSvg::paint` uses this helper and delegates to the real `Svg::paint`;
// the lifecycle smoke alone does not establish pixel output. The public
// sibling probes below establish live ambient inputs, not SVG forwarding.
// ---------------------------------------------------------------------------

/// Pure red used as an unambiguous parent text color.
const RED: Hsla = gpui::hsla(0., 1., 0.5, 1.);

/// Pure green, distinct from red and blue in hue.
const GREEN: Hsla = gpui::hsla(1. / 3., 1., 0.5, 1.);

/// Pure blue, distinct from red and green in hue.
const BLUE: Hsla = gpui::hsla(2. / 3., 1., 0.5, 1.);

/// Paint-phase capture of the resolved Window text-style color.
#[derive(Clone, Default)]
struct AmbientCapture(Rc<RefCell<Vec<Hsla>>>);

impl AmbientCapture {
    /// Drains everything recorded so far, in paint order.
    fn drained(&self) -> Vec<Hsla> {
        self.0.borrow_mut().drain(..).collect()
    }
}

/// A sibling probe that records the resolved Window text color when the real
/// paint phase reaches this exact tree position.
fn ambient_probe(capture: &AmbientCapture) -> Canvas<()> {
    let sink = capture.0.clone();
    canvas(
        move |_bounds, _window, _cx| (),
        move |_bounds, (), window, _cx| sink.borrow_mut().push(window.text_style().color),
    )
    .size(px(8.0))
}

/// A glyph plus probe pair sharing one colored ancestor chain.
///
/// The glyph is built through `asset_glyph` directly — the same shared
/// foundation call shape `button.rs` uses for icon content — so these
/// regressions cannot pass via an icon-only path.
struct GlyphHost {
    parent_color: Option<Hsla>,
    capture: AmbientCapture,
}

impl Render for GlyphHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let mut root = div().flex().flex_col();
        if let Some(color) = self.parent_color {
            root = root.text_color(color);
        }
        root.child(asset_glyph(AssetId::TABLER_CHECK).size(px(16.0)))
            .child(ambient_probe(&self.capture))
    }
}

/// Two sibling subtrees: a nested colored chain and a colorless branch, each
/// carrying its own glyph and probe.
struct NestedHost {
    inner: AmbientCapture,
    outer: AmbientCapture,
}

impl Render for NestedHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .text_color(RED)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .text_color(GREEN)
                    .child(asset_glyph(AssetId::TABLER_CHECK).size(px(16.0)))
                    .child(ambient_probe(&self.inner)),
            )
            .child(
                div().flex().flex_col().child(
                    div()
                        .flex()
                        .flex_col()
                        .child(asset_glyph(AssetId::TABLER_X).size(px(16.0)))
                        .child(ambient_probe(&self.outer)),
                ),
            )
    }
}

/// Full-color marks under a colored parent: routing and embedded bytes are
/// independent of text-color refinements.
struct FullColorHost {
    capture: AmbientCapture,
}

impl Render for FullColorHost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_col()
            .text_color(RED)
            .child(asset_glyph(AssetId::SVGL_CLAUDE_AI).size(px(16.0)))
            .child(asset_glyph(AssetId::ARTISAN_APP_ICON).size(px(16.0)))
            .child(ambient_probe(&self.capture))
    }
}

#[gpui::test]
fn colored_parent_resolves_into_the_tinted_route_at_paint_phase(cx: &mut TestAppContext) {
    let capture = AmbientCapture::default();
    let (_view, cx) = cx.add_window_view(|_, _| GlyphHost {
        parent_color: Some(RED),
        capture: capture.clone(),
    });
    cx.run_until_parked();

    // Every paint-phase observation at this position must resolve the parent
    // color — the input the seam must forward into the inner Svg so its
    // `path.zip(style.text.color)` gate opens with RED rather than skipping
    // paint.
    let observed = capture.drained();
    assert!(
        !observed.is_empty() && observed.iter().all(|color| *color == RED),
        "the resolved parent color must be live at the tinted glyph's paint \
         position; got {observed:?}"
    );
}

#[gpui::test]
fn nearest_ancestor_color_wins_and_sibling_subtrees_do_not_leak(cx: &mut TestAppContext) {
    let inner_capture = AmbientCapture::default();
    let outer_capture = AmbientCapture::default();

    let (_view, cx) = cx.add_window_view(|_, _| NestedHost {
        inner: inner_capture.clone(),
        outer: outer_capture.clone(),
    });
    cx.run_until_parked();

    // Nearest applicable refinement wins: the nested glyph sits under
    // [RED, GREEN] and every observation resolves GREEN...
    let inner_observed = inner_capture.drained();
    assert!(
        !inner_observed.is_empty() && inner_observed.iter().all(|color| *color == GREEN),
        "the nearest colored ancestor must win over the outer one; got {inner_observed:?}"
    );
    // ...while the sibling subtree under [RED] still resolves RED: colors do
    // not leak across sibling branches of the text-style stack.
    let outer_observed = outer_capture.drained();
    assert!(
        !outer_observed.is_empty() && outer_observed.iter().all(|color| *color == RED),
        "unaffected sibling subtrees must resolve their own branch only; got {outer_observed:?}"
    );
}

#[gpui::test]
fn recolored_parents_repaint_the_same_view_across_frames_without_stale_values(
    cx: &mut TestAppContext,
) {
    let capture = AmbientCapture::default();
    let (view, cx) = cx.add_window_view(|_, _| GlyphHost {
        parent_color: Some(RED),
        capture: capture.clone(),
    });
    cx.run_until_parked();

    // Recolor the parent of the SAME persistent view and repaint it — the
    // real production recolor path. GPUI re-renders this retained view with
    // fresh adapters; the sibling probe observes updated ambient inputs,
    // not the glyph's inner slot. Actual slot forwarding and adapter reuse
    // are covered separately by the private unit test
    // `same_instance_under_a_changed_parent_forwards_each_new_value`.
    cx.update(|window, app| {
        view.update(app, |host, cx| {
            host.parent_color = Some(BLUE);
            cx.notify();
        });
        window.refresh();
    });
    cx.run_until_parked();

    // Every observation forms a clean timeline: an initial run of frame-one
    // paints resolving the original color (at least one), followed by ONLY
    // paints resolving the UPDATED color from the repainted view.
    let frames = capture.drained();
    let red_prefix = frames.partition_point(|color| *color == RED);
    assert!(
        red_prefix >= 1 && red_prefix < frames.len(),
        "the same persistent view must paint under the original color before \
         the recolor and again after it; got {frames:?}"
    );
    assert!(
        frames[red_prefix..].iter().all(|color| *color == BLUE),
        "post-recolor paints must resolve the UPDATED ambient color; got \
         {frames:?} — any RED there would be a stale ambient observation"
    );
}

#[gpui::test]
fn without_explicit_ancestors_the_resolved_default_text_style_is_live(cx: &mut TestAppContext) {
    let capture = AmbientCapture::default();
    let (_view, cx) = cx.add_window_view(|_, _| GlyphHost {
        parent_color: None,
        capture: capture.clone(),
    });
    cx.run_until_parked();

    // Without an explicit ancestor, the sibling probe still observes the
    // Window's resolved default. The private suite separately verifies
    // forwarding this value into the glyph's real inner slot.
    let expected_default = TextStyle::default().color;
    assert_eq!(
        expected_default,
        black(),
        "pinned GPUI's default text style must stay pure black"
    );
    let observed = capture.drained();
    assert!(
        !observed.is_empty() && observed.iter().all(|color| *color == expected_default),
        "no-ancestor glyphs must see the resolved default Window text style; \
         got {observed:?}"
    );
}

#[test]
fn muted_recipe_colors_are_authored_overrides_sealed_in_the_public_recipe() {
    // Explicit recipe tints win over ambient by construction: the resolved
    // recipe carries a concrete authored color, which `icon` applies as an
    // ordinary refinement and the seam treats as an override it must never
    // overwrite with the ambient value.
    for mode in [ThemeMode::Light, ThemeMode::Dark] {
        let theme = ArtisanTheme::for_mode(mode);
        let muted = IconStyle::resolve(
            theme,
            AssetId::TABLER_CHECK,
            IconSize::Default,
            IconTint::Muted,
        );
        assert_eq!(
            muted.color,
            Some(theme.colors.muted_foreground.to_paint()),
            "{mode:?} Muted recipes must pin their token color, deferring to nothing"
        );

        let inherit = IconStyle::resolve(
            theme,
            AssetId::TABLER_CHECK,
            IconSize::Default,
            IconTint::Inherit,
        );
        assert_eq!(
            inherit.color, None,
            "{mode:?} Inherit recipes must stay unresolved so ambient color is read live"
        );
    }
}

#[gpui::test]
fn full_color_routes_ignore_text_color_refinements_entirely(cx: &mut TestAppContext) {
    // Authored-color policy exceptions (Claude clay, DeepSeek blue) share the
    // monochrome-artwork property with ordinary Tabler glyphs yet must keep
    // the embedded FullColor route under BOTH ambient parents and explicit
    // caller refinements — text color never re-routes or alpha-masks them.
    for id in [
        AssetId::SVGL_CLAUDE_AI,
        AssetId::SVGL_DEEPSEEK,
        AssetId::SVGL_GITLAB,
        AssetId::ARTISAN_APP_ICON,
    ] {
        let glyph = asset_glyph(id).text_color(BLUE);
        assert_eq!(
            glyph.presentation(),
            Presentation::FullColor,
            "{}: explicit text-color refinements must not change the routed pipeline",
            id.as_str()
        );
    }

    let capture = AmbientCapture::default();

    let (_view, cx) = cx.add_window_view(|_, _| FullColorHost {
        capture: capture.clone(),
    });
    cx.run_until_parked();

    // Lifecycle and ambient-input evidence only: the parent remains live
    // around embedded-route marks. This does not measure glyph bounds or
    // pixels; routing is pinned by the assertions above.
    let observed = capture.drained();
    assert!(
        !observed.is_empty() && observed.iter().all(|color| *color == RED),
        "the colored parent remains live around full-color children; got {observed:?}"
    );
}
