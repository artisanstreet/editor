//! White-box unit tests for `asset_seam`'s private tinted-route delegation
//! helper.
//!
//! This file is linked under `cfg(test)` as a child module of
//! `crate::asset_seam` (see the linkage there and the
//! `tinted_svg_unit_test` Bazel target), which is what grants access to the
//! private `TintedSvg` wrapper and the ONE unconditional scoped-delegation
//! helper exercised here. These are tests OF the real production code — not
//! a duplicate of it.
//!
//! Every scenario drives the actual helper against an actual inner `Svg`
//! and asserts INSIDE the supplied delegation closure, so breaking the
//! ambient mutation, authored precedence, or exact restoration fails an
//! assertion directly. Ambient scopes come from REAL parent composition:
//! probes are drawn under `div().text_color(..)` parents, so scopes are
//! pushed by GPUI's own paint path exactly as production pushes them (a
//! manual `Window::with_text_style` call outside a lifecycle phase would
//! panic). One underlying adapter instance is deliberately shared across
//! two full-lifecycle draws to prove changed parents forward fresh values
//! without freezing. The final test runs `TintedSvg::paint` itself through
//! the public lifecycle; its closure body unconditionally delegates to the
//! real `Svg::paint` and is verified line-by-line in static review.
//!
//! Honest scope note: pinned GPUI seals the frame scene, sprite atlas, and
//! the test App's asset source, so a forwarded color cannot be read back
//! out of painted primitives. What these tests prove executably is the
//! live-resolution, precedence, restoration, reuse, and delegation contract
//! of the production path itself.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::{
    App, Bounds, Element, ElementId, FontWeight, GlobalElementId, Hsla, InspectorElementId,
    IntoElement, LayoutId, ParentElement, Pixels, Styled, TestAppContext, TextStyle, Window, div,
    point, px, size,
};

use crate::asset_seam::{AssetGlyph, asset_glyph};
use crate::icon::{IconSize, IconStyle, IconTint, icon};
use crate::theme::{ArtisanTheme, ThemeMode};

use super::{GlyphRoute, TintedSvg, with_scoped_tint_delegation};

/// Pure red used as an unambiguous ambient input.
const RED: Hsla = Hsla {
    h: 0.,
    s: 1.,
    l: 0.5,
    a: 1.,
};

/// Pure green, distinct from red and blue in hue.
const GREEN: Hsla = Hsla {
    h: 1. / 3.,
    s: 1.,
    l: 0.5,
    a: 1.,
};

/// Pure blue, distinct from red and green in hue.
const BLUE: Hsla = Hsla {
    h: 2. / 3.,
    s: 1.,
    l: 0.5,
    a: 1.,
};

/// The effective text color on the real inner Svg slot right now.
fn slot_color(svg: &mut super::Svg) -> Option<Hsla> {
    svg.text_style().as_ref().and_then(|text| text.color)
}

/// Seed the inner slot of a tinted adapter with an authored text color,
/// exactly like a caller's `.text_color(..)` or a Muted recipe application.
fn author_color(svg: &mut TintedSvg, color: Hsla) {
    svg.text_style()
        .get_or_insert_with(gpui::TextStyleRefinement::default)
        .color = Some(color);
}

/// A fresh production tinted adapter over a real routed catalog asset.
fn fresh_tinted() -> TintedSvg {
    TintedSvg {
        svg: gpui::svg()
            .path(crate::AssetId::TABLER_CHECK.as_str())
            .size(px(16.0)),
    }
}

/// Extracts the real tinted-route adapter out of a production
/// `AssetGlyph` value built through the public `asset_glyph`/`icon` routes
/// (private fields are reachable from this cfg(test) child module).
fn tinted_of(glyph: &mut AssetGlyph) -> &mut TintedSvg {
    match &mut glyph.0 {
        GlyphRoute::Tinted(tinted) => tinted,
        GlyphRoute::FullColor(_) => panic!("test fixture must route tinted"),
    }
}

/// Shared observation channels recorded inside the helper's delegation
/// closure and immediately after it returns, read back by the test once the
/// real lifecycle draw has completed.
#[derive(Clone, Default)]
struct PassObservation {
    seen_inside: Rc<RefCell<Vec<Hsla>>>,
    delegated_count: Rc<Cell<u32>>,
    post_color: Rc<RefCell<Option<Hsla>>>,
    post_slot_present: Rc<Cell<bool>>,
}

impl PassObservation {
    /// Runs the ONE production delegation helper on the adapter and records
    /// the in-closure forwarded color plus the exact post-pass slot state.
    fn run_pass(&self, tinted: &mut TintedSvg, window: &mut Window, app: &mut App) {
        let seen = self.seen_inside.clone();
        let delegated = self.delegated_count.clone();
        let post_color = self.post_color.clone();
        let post_present = self.post_slot_present.clone();

        with_scoped_tint_delegation(
            &mut tinted.svg,
            window,
            app,
            |svg: &mut super::Svg, _window: &mut Window, _app: &mut App| {
                // Inside the exact delegation closure the REAL inner slot
                // carries whatever this pass forwards. Deleting the
                // helper's mutation or breaking precedence fails these
                // captured values back in the test.
                seen.borrow_mut()
                    .push(slot_color(svg).expect("the scoped pass must have tinted the slot"));
                delegated.set(delegated.get() + 1);
            },
        );

        post_color.replace(slot_color(&mut tinted.svg));
        post_present.set(tinted.svg.text_style().is_some());
    }

    /// Draws one probe through a full real lifecycle under the given
    /// colored-parent composition (outermost color first), sharing the
    /// supplied underlying adapter instance.
    fn draw_under(
        &self,
        cx: &mut TestAppContext,
        tinted: &Rc<RefCell<TintedSvg>>,
        colors: &[Hsla],
    ) {
        let observed = self.clone();
        let shared_tinted = tinted.clone();
        let colors = colors.to_vec();

        let cx = cx.add_empty_window();
        cx.draw(
            point(px(0.0), px(0.0)),
            size(px(16.0), px(16.0)),
            move |_window, _app| {
                let observed = observed.clone();
                let shared_tinted = shared_tinted.clone();
                let colors = colors.clone();

                let probe = DelegationProbe {
                    action: Box::new(move |window: &mut Window, app: &mut App| {
                        observed.run_pass(&mut shared_tinted.borrow_mut(), window, app);
                    }),
                };

                let mut root = div().size(px(16.0)).child(probe);
                for color in colors {
                    root = root.text_color(color);
                }
                root
            },
        );
    }
}

/// A cfg(test)-internal element whose real paint phase runs one scoped
/// delegation pass supplied by the test fixture.
struct DelegationProbe {
    action: PaintAction,
}

/// The per-pass delegation action a test fixture supplies to its probe.
type PaintAction = Box<dyn Fn(&mut Window, &mut App)>;

impl Element for DelegationProbe {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        (window.request_layout(gpui::Style::default(), [], cx), ())
    }

    fn prepaint(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut App,
    ) {
    }

    fn paint(
        &mut self,
        _global_id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        app: &mut App,
    ) {
        (self.action)(window, app);
    }
}

impl IntoElement for DelegationProbe {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

#[gpui::test]
fn colored_parent_ambient_is_read_live_inside_the_delegation_closure(cx: &mut TestAppContext) {
    let observation = PassObservation::default();
    let svg = Rc::new(RefCell::new(fresh_tinted()));

    observation.draw_under(cx, &svg, &[RED]);

    assert_eq!(
        observation.delegated_count.get(),
        1,
        "the helper must invoke its delegation closure exactly once"
    );
    assert_eq!(
        observation.seen_inside.take(),
        vec![RED],
        "the live resolved parent color must be forwarded into the real \
         inner slot before delegating"
    );
    assert_eq!(
        observation.post_color.take(),
        None,
        "an absent refinement must be absent again afterward"
    );
    assert!(!observation.post_slot_present.get());
}

#[gpui::test]
fn no_explicit_parent_resolves_the_window_default_inside_the_closure(cx: &mut TestAppContext) {
    let observation = PassObservation::default();
    let svg = Rc::new(RefCell::new(fresh_tinted()));

    observation.draw_under(cx, &svg, &[]);

    assert_eq!(
        observation.seen_inside.take(),
        vec![TextStyle::default().color],
        "with no explicit ancestor the resolved DEFAULT Window text style \
         is what the helper forwards"
    );
    assert_eq!(observation.post_color.take(), None);
    assert!(!observation.post_slot_present.get());
}

#[gpui::test]
fn nested_scopes_resolve_the_nearest_color_and_restore_outer_resolution(cx: &mut TestAppContext) {
    // Two sibling probes in one frame: the first sits under nested
    // [RED > GREEN] scopes, the second only under [RED], demonstrating both
    // nearest-wins and that popping the inner scope restores outer
    // resolution.
    let nested_observation = PassObservation::default();
    let outer_observation = PassObservation::default();
    let nested_svg = Rc::new(RefCell::new(fresh_tinted()));
    let outer_svg = Rc::new(RefCell::new(fresh_tinted()));

    let nested_observed = nested_observation.clone();
    let nested_shared = nested_svg.clone();
    let outer_observed = outer_observation.clone();
    let outer_shared = outer_svg.clone();

    let cx = cx.add_empty_window();
    cx.draw(
        point(px(0.0), px(0.0)),
        size(px(32.0), px(16.0)),
        move |_window, _app| {
            let nested_probe = DelegationProbe {
                action: Box::new(move |window: &mut Window, app: &mut App| {
                    nested_observed.run_pass(&mut nested_shared.borrow_mut(), window, app);
                }),
            };
            let outer_probe = DelegationProbe {
                action: Box::new(move |window: &mut Window, app: &mut App| {
                    outer_observed.run_pass(&mut outer_shared.borrow_mut(), window, app);
                }),
            };

            div()
                .flex()
                .size(px(32.0))
                .text_color(RED)
                .child(div().size(px(8.0)).text_color(GREEN).child(nested_probe))
                .child(div().size(px(8.0)).child(outer_probe))
        },
    );

    assert_eq!(
        nested_observation.seen_inside.take(),
        vec![GREEN],
        "the nearest colored ancestor must win inside delegation"
    );
    assert_eq!(
        outer_observation.seen_inside.take(),
        vec![RED],
        "the sibling subtree under only the outer scope must still resolve RED"
    );
    assert_eq!(nested_observation.post_color.take(), None);
    assert_eq!(outer_observation.post_color.take(), None);
    assert!(!nested_observation.post_slot_present.get());
    assert!(!outer_observation.post_slot_present.get());
}

#[gpui::test]
fn same_instance_under_a_changed_parent_forwards_each_new_value(cx: &mut TestAppContext) {
    let observation = PassObservation::default();
    let svg = Rc::new(RefCell::new(fresh_tinted()));

    // First repaint under RED...
    observation.draw_under(cx, &svg, &[RED]);
    // ...then a changed-parent repaint of the SAME underlying adapter
    // instance under BLUE.
    observation.draw_under(cx, &svg, &[BLUE]);

    assert_eq!(
        observation.seen_inside.take(),
        vec![RED, BLUE],
        "the SAME underlying Svg must forward each changed ambient value, \
         never freezing the first resolution"
    );
    assert_eq!(
        observation.post_color.take(),
        None,
        "no frozen refinement may survive the changed-parent passes"
    );
    assert!(!observation.post_slot_present.get());
}

#[gpui::test]
fn pre_existing_colorless_refinement_preserves_its_property_and_none_color(
    cx: &mut TestAppContext,
) {
    let observation = PassObservation::default();
    let svg = Rc::new(RefCell::new(fresh_tinted()));
    svg.borrow_mut()
        .text_style()
        .get_or_insert_with(Default::default)
        .font_weight = Some(FontWeight::BOLD);

    observation.draw_under(cx, &svg, &[RED]);

    assert_eq!(
        observation.seen_inside.take(),
        vec![RED],
        "the real RED scope supplies the forwarded ambient"
    );
    assert_eq!(
        observation.post_color.take(),
        None,
        "the ambient value must never become authored state"
    );
    assert!(
        observation.post_slot_present.get(),
        "a pre-existing refinement must survive the scoped pass"
    );
    let refinement = svg
        .borrow_mut()
        .text_style()
        .as_ref()
        .expect("refinement presence recorded above")
        .clone();
    assert_eq!(
        refinement.font_weight,
        Some(FontWeight::BOLD),
        "unrelated authored properties must be preserved exactly"
    );
}

#[gpui::test]
fn authored_color_wins_over_conflicting_ambient_during_and_after_delegation(
    cx: &mut TestAppContext,
) {
    let observation = PassObservation::default();
    let svg = Rc::new(RefCell::new(fresh_tinted()));
    author_color(&mut svg.borrow_mut(), GREEN);

    observation.draw_under(cx, &svg, &[RED]);

    assert_eq!(
        observation.seen_inside.take(),
        vec![GREEN],
        "the authored color must reach delegation unchanged despite the \
         conflicting ambient"
    );
    assert_eq!(
        observation.post_color.take(),
        Some(GREEN),
        "the authored color must remain unchanged after delegation"
    );
    assert!(observation.post_slot_present.get());
}

#[gpui::test]
fn muted_recipe_via_the_production_icon_route_keeps_its_token_under_conflicting_ambient(
    cx: &mut TestAppContext,
) {
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    let muted_token = theme.colors.muted_foreground.to_paint();

    // The ACTUAL production icon route: resolve the recipe and build the
    // glyph through `icon`; its real inner adapter is exercised below.
    let glyph = Rc::new(RefCell::new(icon(IconStyle::resolve(
        theme,
        crate::AssetId::TABLER_CHECK,
        IconSize::Default,
        IconTint::Muted,
    ))));

    let observation = PassObservation::default();
    let observed = observation.clone();
    let glyph_shared = glyph.clone();

    let cx = cx.add_empty_window();
    cx.draw(
        point(px(0.0), px(0.0)),
        size(px(16.0), px(16.0)),
        move |_window, _app| {
            let observed = observed.clone();
            let glyph_shared = glyph_shared.clone();
            let probe = DelegationProbe {
                action: Box::new(move |window: &mut Window, app: &mut App| {
                    let mut glyph = glyph_shared.borrow_mut();
                    observed.run_pass(tinted_of(&mut glyph), window, app);
                }),
            };
            div().size(px(16.0)).text_color(RED).child(probe)
        },
    );

    assert_eq!(
        observation.seen_inside.take(),
        vec![muted_token],
        "the Muted recipe's authored token must reach delegation instead of \
         the conflicting ambient"
    );
    assert_eq!(
        observation.post_color.take(),
        Some(muted_token),
        "the Muted token must remain unchanged after delegation"
    );
    assert!(observation.post_slot_present.get());
}

#[gpui::test]
fn caller_refinements_via_asset_glyph_apply_last_wins_under_ambient(cx: &mut TestAppContext) {
    // Shared-foundation caller refinements: the LAST one wins over both
    // earlier callers and any ambient.
    let glyph = Rc::new(RefCell::new(
        asset_glyph(crate::AssetId::TABLER_CHECK)
            .text_color(RED)
            .text_color(BLUE),
    ));

    let observation = PassObservation::default();
    let observed = observation.clone();
    let glyph_shared = glyph.clone();

    let cx = cx.add_empty_window();
    cx.draw(
        point(px(0.0), px(0.0)),
        size(px(16.0), px(16.0)),
        move |_window, _app| {
            let observed = observed.clone();
            let glyph_shared = glyph_shared.clone();
            let probe = DelegationProbe {
                action: Box::new(move |window: &mut Window, app: &mut App| {
                    let mut glyph = glyph_shared.borrow_mut();
                    observed.run_pass(tinted_of(&mut glyph), window, app);
                }),
            };
            div().size(px(16.0)).text_color(GREEN).child(probe)
        },
    );

    assert_eq!(
        observation.seen_inside.take(),
        vec![BLUE],
        "the last caller refinement must win inside delegation despite the \
         GREEN ambient"
    );
    assert_eq!(
        observation.post_color.take(),
        Some(BLUE),
        "the winning caller color must remain afterward"
    );
    assert!(observation.post_slot_present.get());
}

/// Full-lifecycle execution smoke: `TintedSvg::paint` runs its ONE
/// unconditional helper whose closure delegates to the real `Svg::paint`.
/// Proves successful execution only — no hitbox, frame-scene, or pixel
/// observation is claimed or attempted.
#[gpui::test]
fn real_paint_lifecycle_executes_through_the_helper_without_panicking(cx: &mut TestAppContext) {
    let cx = cx.add_empty_window();

    let tinted = fresh_tinted();
    cx.draw(
        point(px(0.0), px(0.0)),
        size(px(16.0), px(16.0)),
        |_window, _app| div().size(px(16.0)).text_color(RED).child(tinted),
    );
}
