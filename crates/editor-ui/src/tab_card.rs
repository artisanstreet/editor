//! Darker glass shell with a browser-tab extension on its top-left edge.
//!
//! The shell wraps a glass card in a few pixels of darker glass so the frame
//! reads as a thick border, and grows a tab above its top-left corner for a
//! ghost control. Rounded rectangles cannot express the concave fillet where
//! the tab meets the body, so the whole silhouette is painted as one path:
//! one fill means no seams between tab and body. The shell and the tab
//! measure themselves during prepaint, and the silhouette paints from those
//! bounds, so the tab follows whatever its content needs.

use std::{cell::Cell, rc::Rc};

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, Background, Bounds, BoxShadow, ContentMask, Corners, Div, Filter, Hsla,
    IntoElement, ParentElement as _, PathBuilder, Pixels, Styled as _, Window, canvas, div,
    linear_color_stop, linear_gradient, point, px, relative, size,
};

use crate::glass::{GlassStrength, card_shadows, glass_blur_radius};
use crate::theme::{ArtisanTheme, Oklch, SurfaceStep, ThemeMode};

/// Padding between the shell edge and the wrapped card.
pub const TAB_CARD_PADDING_PX: f32 = 4.0;
/// Height of the tab above the body.
pub const TAB_CARD_TAB_HEIGHT_PX: f32 = 36.0;
/// Rounding of the tab's two top corners.
const TAB_RADIUS_PX: f32 = 14.0;
/// Concave fillet where the tab's right edge meets the body.
const FILLET_RADIUS_PX: f32 = 12.0;
/// Horizontal room the tab leaves around its content.
const TAB_PADDING_X_PX: f32 = 4.0;

/// Wraps `body` in the darker glass shell with `tab` on its top-left edge.
///
/// `inner_radius` is the wrapped card's corner radius; the shell's outer
/// corners add the padding so both stay concentric.
#[must_use]
pub fn tab_card(tab: impl IntoElement, body: impl IntoElement, inner_radius: Pixels) -> Div {
    tab_card_with_strip(tab, None, body, inner_radius)
}

/// A lighter strip filling the empty space beside the tab.
///
/// The shell's shape never changes for it: the tab keeps its rounded corner
/// and its fillet into the body, the body keeps its rounded top-right corner,
/// and all keep their edge. The strip is poured into the space around them:
/// the notch outside the tab's corner, the curve of the fillet, and down into
/// the gap outside the body's corner, so it never paints under the shell and
/// its right edge runs on into the body's.
pub struct TabCardStrip {
    /// How far the strip has grown out of the tab, 0 to 1, already eased.
    /// Its leading edge is rounded; over the last stretch it follows the
    /// body's corner down, so at 1 it fills the space completely.
    pub reveal: f32,
    /// What the strip shows; it is laid out over the strip's full width and
    /// clipped to the grown part.
    pub content: AnyElement,
}

/// [`tab_card`] with an optional [`TabCardStrip`] beside the tab.
#[must_use]
pub fn tab_card_with_strip(
    tab: impl IntoElement,
    strip: Option<TabCardStrip>,
    body: impl IntoElement,
    inner_radius: Pixels,
) -> Div {
    let radius = inner_radius + px(TAB_CARD_PADDING_PX);
    let reveal = strip
        .as_ref()
        .map_or(0.0, |strip| strip.reveal.clamp(0.0, 1.0));

    let blur = glass_blur_radius(GlassStrength::Strong);
    let card_edge = card_shadows(&ArtisanTheme::for_mode(ThemeMode::Dark));
    let shell_bounds = Rc::new(Cell::new(None::<Bounds<Pixels>>));
    let shell_probe = Rc::clone(&shell_bounds);

    // Paint order is the whole trick. Backdrop blur samples whatever is
    // already painted, so the blur layers paint first, then the silhouette,
    // then the content. Blurring after the silhouette smears its rim, which
    // is why the strip's blur paints with the silhouette rather than as a
    // layer of its own.
    let tab = div()
        .relative()
        .h(px(TAB_CARD_TAB_HEIGHT_PX))
        .px(px(TAB_PADDING_X_PX))
        .flex()
        .items_center()
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .rounded_t(px(TAB_RADIUS_PX))
                .backdrop_blur(blur),
        )
        // Spans the tab, so its bounds give the tab width; it paints the
        // whole silhouette from the shell bounds probed above.
        .child(
            canvas(
                |bounds, _, _| bounds.size.width,
                move |_, tab_width, window, _| {
                    if let Some(bounds) = shell_bounds.get() {
                        paint_silhouette(
                            bounds, tab_width, radius, reveal, blur, &card_edge, window,
                        );
                    }
                },
            )
            .absolute()
            .size_full(),
        )
        .child(tab);

    div()
        .relative()
        .w_full()
        .flex()
        .flex_col()
        .items_start()
        .child(
            canvas(move |bounds, _, _| shell_probe.set(Some(bounds)), |_, (), _, _| {})
                .absolute()
                .size_full(),
        )
        // The shadow and the body's blur sit under the body area.
        .child(
            div()
                .absolute()
                .top(px(TAB_CARD_TAB_HEIGHT_PX))
                .left_0()
                .right_0()
                .bottom_0()
                .rounded_tr(radius)
                .rounded_b(radius)
                .shadow(shell_shadow()),
        )
        // A grown strip fills the gap outside the body's top-right corner,
        // and the strip's own blur stops at the body's top edge, so the
        // body's blur squares that corner off to back it. It paints before
        // the silhouette, so it blurs only what lies behind the shell.
        .child(
            div()
                .absolute()
                .top(px(TAB_CARD_TAB_HEIGHT_PX))
                .left_0()
                .right_0()
                .bottom_0()
                .when(reveal < 1.0, |layer| layer.rounded_tr(radius))
                .rounded_b(radius)
                .backdrop_blur(blur),
        )
        .child(
            div()
                .w_full()
                .flex()
                .flex_row()
                .child(tab)
                .children(strip.map(strip_layer)),
        )
        .child(
            div()
                .w_full()
                .p(px(TAB_CARD_PADDING_PX))
                .child(body.into_any_element()),
        )
}

/// The strip's content, laid out to the right of the tab over the strip's
/// full width and clipped to the grown part.
fn strip_layer(strip: TabCardStrip) -> Div {
    let reveal = strip.reveal.clamp(0.0, 1.0);
    div()
        .relative()
        .flex_1()
        .min_w_0()
        .h(px(TAB_CARD_TAB_HEIGHT_PX))
        .child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left_0()
                .w(relative(reveal))
                .overflow_hidden()
                .child(strip.content),
        )
}

/// The composer's diagonal glass recipe, two steps down the surface ramp and
/// denser, so the shell reads darker than the card it frames.
fn shell_material() -> Background {
    linear_gradient(
        145.0,
        linear_color_stop(SurfaceStep::S800.oklch().with_alpha(0.62).to_paint(), 0.0),
        linear_color_stop(SurfaceStep::S900.oklch().with_alpha(0.62).to_paint(), 1.0),
    )
}

/// The strip's glass: two steps lighter than the shell at the same density,
/// so it reads as a raised second tab rather than a different material.
fn strip_material() -> Background {
    linear_gradient(
        145.0,
        linear_color_stop(SurfaceStep::S700.oklch().with_alpha(0.62).to_paint(), 0.0),
        linear_color_stop(SurfaceStep::S800.oklch().with_alpha(0.62).to_paint(), 1.0),
    )
}

fn shell_highlight() -> Background {
    let white = Oklch::new(1.0, 0.0, 0.0);
    linear_gradient(
        180.0,
        linear_color_stop(white.with_alpha(0.035).to_paint(), 0.0),
        linear_color_stop(white.with_alpha(0.0).to_paint(), 0.35),
    )
}

/// Backing under the wrapped card (`#0e0e0e`); raise it to lift the card.
fn card_well() -> Hsla {
    // Oklab lightness of sRGB #0e0e0e.
    Oklch::new(0.164, 0.0, 0.0).to_paint()
}

/// Strength of the `@utility card` edge layers relative to the token (8%).
/// Halved: the wrapped card already carries the strong glass edge.
const RIM_ALPHA: f32 = 0.04;

/// The `card` hairline ring: dark mode's `--highlight` (`surface-50`).
fn shell_ring() -> Hsla {
    SurfaceStep::S50.oklch().with_alpha(RIM_ALPHA).to_paint()
}

/// The `card` top light: white on the upward-facing edges, fading out by the
/// body's top edge so the sides and bottom carry only the ring.
fn shell_top_light() -> Background {
    let white = Oklch::new(1.0, 0.0, 0.0);
    linear_gradient(
        180.0,
        linear_color_stop(white.with_alpha(RIM_ALPHA).to_paint(), 0.0),
        linear_color_stop(white.with_alpha(0.0).to_paint(), 0.4),
    )
}

fn shell_shadow() -> Vec<BoxShadow> {
    vec![BoxShadow {
        color: Oklch::new(0.0, 0.0, 0.0).with_alpha(0.6).to_paint(),
        offset: point(px(0.0), px(18.0)),
        blur_radius: px(48.0),
        spread_radius: px(-18.0),
        inset: false,
    }]
}

/// Traces the tab and body outline clockwise from the tab's top-left corner.
fn trace_silhouette(
    bounds: Bounds<Pixels>,
    tab_width: Pixels,
    radius: Pixels,
    builder: &mut PathBuilder,
) {
    let (x0, y0) = (bounds.origin.x, bounds.origin.y);
    let (x1, y1) = (x0 + bounds.size.width, y0 + bounds.size.height);
    let tab_radius = px(TAB_RADIUS_PX);
    let fillet = px(FILLET_RADIUS_PX);
    let tab_x1 = x0 + tab_width;
    let body_y0 = y0 + px(TAB_CARD_TAB_HEIGHT_PX);
    let radii = |value: Pixels| point(value, value);

    builder.move_to(point(x0, y0 + tab_radius));
    builder.arc_to(
        radii(tab_radius),
        Pixels::ZERO,
        false,
        true,
        point(x0 + tab_radius, y0),
    );
    builder.line_to(point(tab_x1 - tab_radius, y0));
    builder.arc_to(
        radii(tab_radius),
        Pixels::ZERO,
        false,
        true,
        point(tab_x1, y0 + tab_radius),
    );
    builder.line_to(point(tab_x1, body_y0 - fillet));
    // The fillet is concave, so it sweeps against the outline's direction.
    builder.arc_to(
        radii(fillet),
        Pixels::ZERO,
        false,
        false,
        point(tab_x1 + fillet, body_y0),
    );
    builder.line_to(point(x1 - radius, body_y0));
    builder.arc_to(
        radii(radius),
        Pixels::ZERO,
        false,
        true,
        point(x1, body_y0 + radius),
    );
    builder.line_to(point(x1, y1 - radius));
    builder.arc_to(
        radii(radius),
        Pixels::ZERO,
        false,
        true,
        point(x1 - radius, y1),
    );
    builder.line_to(point(x0 + radius, y1));
    builder.arc_to(
        radii(radius),
        Pixels::ZERO,
        false,
        true,
        point(x0, y1 - radius),
    );
    builder.close();
}

/// Where the strip's leading edge stands at `reveal`: from one tab corner's
/// width past the tab to the shell's right edge. `None` before it grows.
fn strip_right(bounds: Bounds<Pixels>, tab_width: Pixels, reveal: f32) -> Option<Pixels> {
    let start = bounds.origin.x + tab_width + px(TAB_RADIUS_PX);
    let end = bounds.origin.x + bounds.size.width;
    (reveal > 0.0 && end > start).then(|| start + (end - start) * reveal.min(1.0))
}

/// The strip's leading edge at `right`: the radius of its top corner, and
/// where its bottom meets the body.
struct StripEnd {
    top: Pixels,
    bottom: StripBottom,
}

enum StripBottom {
    /// Short of the body's corner: a rounded corner sitting on the body's
    /// top edge. It tightens to square as the edge nears the corner.
    Rounded(Pixels),
    /// Over the body's corner: the edge runs down to the corner's curve,
    /// this far below the body's top, and the bottom follows the curve back.
    /// At the shell's right edge it is the whole radius, so the strip's
    /// right edge runs straight on into the body's.
    Down(Pixels),
}

fn strip_end(bounds: Bounds<Pixels>, tab_width: Pixels, radius: Pixels, right: Pixels) -> StripEnd {
    let tab_x1 = bounds.origin.x + tab_width;
    let corner_x = bounds.origin.x + bounds.size.width - radius;
    let top = px(TAB_RADIUS_PX).min((right - tab_x1) / 2.0);
    let bottom = if right > corner_x {
        let r = f32::from(radius);
        let dx = f32::from(right - corner_x).min(r);
        StripBottom::Down(px(r - (r * r - dx * dx).max(0.0).sqrt()))
    } else {
        StripBottom::Rounded(top.min(corner_x - right))
    };
    StripEnd { top, bottom }
}

/// Traces the strip clockwise: its own top and leading edge, then back
/// along the body's corner and top, around the fillet, up the tab's side,
/// and over the tab's corner, each shared edge the silhouette's own segment
/// run backwards, so the two never overlap.
fn trace_strip(
    bounds: Bounds<Pixels>,
    tab_width: Pixels,
    radius: Pixels,
    right: Pixels,
    builder: &mut PathBuilder,
) {
    let y0 = bounds.origin.y;
    let tab_radius = px(TAB_RADIUS_PX);
    let fillet = px(FILLET_RADIUS_PX);
    let tab_x1 = bounds.origin.x + tab_width;
    let corner_x = bounds.origin.x + bounds.size.width - radius;
    let body_y0 = y0 + px(TAB_CARD_TAB_HEIGHT_PX);
    let radii = |value: Pixels| point(value, value);

    builder.move_to(point(tab_x1 - tab_radius, y0));
    trace_strip_end(bounds, tab_width, radius, right, builder);
    if let StripBottom::Down(_) = strip_end(bounds, tab_width, radius, right).bottom {
        // Back along the body's corner, against the silhouette's direction.
        builder.arc_to(
            radii(radius),
            Pixels::ZERO,
            false,
            false,
            point(corner_x, body_y0),
        );
    }
    builder.line_to(point(tab_x1 + fillet, body_y0));
    builder.arc_to(
        radii(fillet),
        Pixels::ZERO,
        false,
        true,
        point(tab_x1, body_y0 - fillet),
    );
    builder.line_to(point(tab_x1, y0 + tab_radius));
    builder.arc_to(
        radii(tab_radius),
        Pixels::ZERO,
        false,
        false,
        point(tab_x1 - tab_radius, y0),
    );
    builder.close();
}

/// Continues a path from the strip's top-left along its top, over its top
/// corner, and down its leading edge to where it meets the body: on the
/// body's top edge, or on the body's corner once over it.
fn trace_strip_end(
    bounds: Bounds<Pixels>,
    tab_width: Pixels,
    radius: Pixels,
    right: Pixels,
    builder: &mut PathBuilder,
) {
    let y0 = bounds.origin.y;
    let body_y0 = y0 + px(TAB_CARD_TAB_HEIGHT_PX);
    let radii = |value: Pixels| point(value, value);
    let end = strip_end(bounds, tab_width, radius, right);

    builder.line_to(point(right - end.top, y0));
    builder.arc_to(
        radii(end.top),
        Pixels::ZERO,
        false,
        true,
        point(right, y0 + end.top),
    );
    match end.bottom {
        StripBottom::Rounded(bottom) => {
            builder.line_to(point(right, body_y0 - bottom));
            if bottom > Pixels::ZERO {
                builder.arc_to(
                    radii(bottom),
                    Pixels::ZERO,
                    false,
                    true,
                    point(right - bottom, body_y0),
                );
            }
        }
        StripBottom::Down(drop) => builder.line_to(point(right, body_y0 + drop)),
    }
}

/// Traces the wrapped card's area counter-clockwise, punching it out of the
/// silhouette fill. The card is translucent glass: painting the shell under
/// it would lighten it, so the card keeps its own backdrop instead.
fn trace_card_hole(bounds: Bounds<Pixels>, radius: Pixels, builder: &mut PathBuilder) {
    let pad = px(TAB_CARD_PADDING_PX);
    let x0 = bounds.origin.x + pad;
    let y0 = bounds.origin.y + px(TAB_CARD_TAB_HEIGHT_PX) + pad;
    let x1 = bounds.origin.x + bounds.size.width - pad;
    let y1 = bounds.origin.y + bounds.size.height - pad;
    let r = radius - pad;
    let radii = point(r, r);

    builder.move_to(point(x0 + r, y0));
    builder.arc_to(radii, Pixels::ZERO, false, false, point(x0, y0 + r));
    builder.line_to(point(x0, y1 - r));
    builder.arc_to(radii, Pixels::ZERO, false, false, point(x0 + r, y1));
    builder.line_to(point(x1 - r, y1));
    builder.arc_to(radii, Pixels::ZERO, false, false, point(x1, y1 - r));
    builder.line_to(point(x1, y0 + r));
    builder.arc_to(radii, Pixels::ZERO, false, false, point(x1 - r, y0));
    builder.close();
}

/// Blurs what lies behind the strip, from the tab's edge to its leading edge
/// and down to the body's top, rounded like the strip's own end.
///
/// Translucent glass without it reads darker. The notch outside the tab's
/// corner stays out: covering it would blur the tab's corner twice. Below
/// the body's top, the body's blur backs the strip once it has grown.
fn paint_strip_blur(
    bounds: Bounds<Pixels>,
    tab_width: Pixels,
    radius: Pixels,
    right: Pixels,
    blur: Pixels,
    window: &mut Window,
) {
    let tab_x1 = bounds.origin.x + tab_width;
    let end = strip_end(bounds, tab_width, radius, right);
    let bottom = match end.bottom {
        StripBottom::Rounded(bottom) => bottom,
        StripBottom::Down(_) => Pixels::ZERO,
    };
    window.paint_backdrop_filter(
        Bounds {
            origin: point(tab_x1, bounds.origin.y),
            size: size(right - tab_x1, px(TAB_CARD_TAB_HEIGHT_PX)),
        },
        Corners {
            top_left: Pixels::ZERO,
            top_right: end.top,
            bottom_right: bottom,
            bottom_left: Pixels::ZERO,
        },
        &[Filter::Blur(blur)],
    );
}

/// The `@utility card` stack along the strip's own edges: its top and its
/// leading edge, down to where it meets the body.
///
/// The stack paints outside a rounded rectangle from the tab's corner to the
/// strip's leading edge. The tab and the body carry the shell's own edge, so
/// a mask keeps the stack off the rectangle's left and bottom sides and
/// nothing stacks.
fn paint_strip_card_edge(
    bounds: Bounds<Pixels>,
    tab_width: Pixels,
    radius: Pixels,
    right: Pixels,
    shadows: &[BoxShadow],
    window: &mut Window,
) {
    let y0 = bounds.origin.y;
    let left = bounds.origin.x + tab_width - px(TAB_RADIUS_PX);
    let body_y0 = y0 + px(TAB_CARD_TAB_HEIGHT_PX);
    let end = strip_end(bounds, tab_width, radius, right);
    let (bottom, bottom_radius) = match end.bottom {
        StripBottom::Rounded(bottom) => (body_y0, bottom),
        StripBottom::Down(drop) => (body_y0 + drop, Pixels::ZERO),
    };
    // Room for the stack's blur above and to the right.
    let reach = px(64.0);
    let mask = ContentMask {
        bounds: Bounds::from_corners(point(left, y0 - reach), point(right + reach, bottom)),
    };
    window.with_content_mask(Some(mask), |window| {
        window.paint_drop_shadows(
            Bounds::from_corners(point(left, y0), point(right, bottom)),
            Corners {
                top_left: Pixels::ZERO,
                top_right: end.top,
                bottom_right: bottom_radius,
                bottom_left: Pixels::ZERO,
            },
            shadows,
        );
    });
}

fn paint_silhouette(
    bounds: Bounds<Pixels>,
    tab_width: Pixels,
    radius: Pixels,
    reveal: f32,
    blur: Pixels,
    card_edge: &[BoxShadow],
    window: &mut Window,
) {
    if tab_width <= Pixels::ZERO {
        return;
    }
    // The strip fills the space around the shell and paints first: where
    // their edges meet, the shell's fill and ring land on top. Its blur
    // paints before its fill, so it blurs what lies behind the strip and
    // not the strip itself.
    let strip = strip_right(bounds, tab_width, reveal);
    if let Some(right) = strip {
        paint_strip_blur(bounds, tab_width, radius, right, blur, window);
        let mut fill = PathBuilder::fill();
        trace_strip(bounds, tab_width, radius, right, &mut fill);
        if let Ok(path) = fill.build() {
            window.paint_path(path, strip_material());
        }
        let mut highlight = PathBuilder::fill();
        trace_strip(bounds, tab_width, radius, right, &mut highlight);
        if let Ok(path) = highlight.build() {
            window.paint_path(path, shell_highlight());
        }
    }
    // The wrapped card is translucent glass. Over pure black it reads too
    // dark to separate from the page, and over the shell material it reads
    // washed out, so it gets its own near-black well.
    let mut well = PathBuilder::fill();
    trace_card_hole(bounds, radius, &mut well);
    if let Ok(path) = well.build() {
        window.paint_path(path, card_well());
    }
    let mut fill = PathBuilder::fill();
    trace_silhouette(bounds, tab_width, radius, &mut fill);
    trace_card_hole(bounds, radius, &mut fill);
    if let Ok(path) = fill.build() {
        window.paint_path(path, shell_material());
    }
    let mut highlight = PathBuilder::fill();
    trace_silhouette(bounds, tab_width, radius, &mut highlight);
    trace_card_hole(bounds, radius, &mut highlight);
    if let Ok(path) = highlight.build() {
        window.paint_path(path, shell_highlight());
    }
    // Inset by half a pixel so the hairlines stay inside the fill.
    let inset = Bounds {
        origin: bounds.origin + point(px(0.5), px(0.5)),
        size: size(bounds.size.width - px(1.0), bounds.size.height - px(1.0)),
    };
    // The `@utility card` edge, traced along the silhouette: a 0.5 px ring
    // plus a 0.5 px top light, both at reduced strength.
    let shell_edge = |builder: &mut PathBuilder| {
        trace_silhouette(inset, tab_width - px(1.0), radius - px(0.5), builder);
    };
    let mut ring = PathBuilder::stroke(px(0.5));
    shell_edge(&mut ring);
    if let Ok(path) = ring.build() {
        window.paint_path(path, shell_ring());
    }
    let mut top_light = PathBuilder::stroke(px(0.5));
    shell_edge(&mut top_light);
    if let Ok(path) = top_light.build() {
        window.paint_path(path, shell_top_light());
    }
    if let Some(right) = strip {
        paint_strip_card_edge(bounds, tab_width, radius, right, card_edge, window);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell() -> Bounds<Pixels> {
        Bounds {
            origin: point(px(0.0), px(0.0)),
            size: size(px(800.0), px(200.0)),
        }
    }

    #[test]
    fn no_strip_before_it_grows() {
        assert_eq!(strip_right(shell(), px(120.0), 0.0), None);
    }

    #[test]
    fn the_strip_grows_from_past_the_tab_to_the_shell_edge() {
        let start = strip_right(shell(), px(120.0), 0.001).expect("strip");
        assert!(start > px(120.0) + px(TAB_RADIUS_PX) - px(0.01));
        assert_eq!(strip_right(shell(), px(120.0), 1.0), Some(px(800.0)));
    }

    #[test]
    fn a_grown_strip_runs_its_right_edge_on_into_the_body() {
        let end = strip_end(shell(), px(120.0), px(24.0), px(800.0));
        assert_eq!(end.top, px(TAB_RADIUS_PX));
        assert!(matches!(end.bottom, StripBottom::Down(drop) if drop == px(24.0)));
    }

    #[test]
    fn the_leading_corner_squares_off_as_it_reaches_the_body_corner() {
        let rounded = |right: f32| match strip_end(shell(), px(120.0), px(24.0), px(right)).bottom {
            StripBottom::Rounded(radius) => Some(radius),
            StripBottom::Down(_) => None,
        };
        assert_eq!(rounded(400.0), Some(px(TAB_RADIUS_PX)));
        assert_eq!(rounded(770.0), Some(px(6.0)));
        assert_eq!(rounded(776.0), Some(Pixels::ZERO));
        assert_eq!(rounded(777.0), None);
    }

    #[test]
    fn a_grown_strip_reaches_the_shell_edge_and_no_further() {
        for step in 1..=100_u8 {
            let right = strip_right(shell(), px(120.0), f32::from(step) / 100.0).expect("strip");
            assert!(right <= px(800.0));
        }
    }
}
