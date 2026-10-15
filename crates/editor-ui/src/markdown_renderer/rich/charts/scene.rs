//! What a plot paints: shapes in plot fractions, painted on one canvas.
//!
//! Layout code produces [`Shape`]s whose coordinates are fractions of the
//! plot (0..1 across, 0..1 down), so it stays pure and testable; the canvas
//! maps them onto whatever bounds the plot gets. The canvas also records
//! those bounds in a [`Probe`] so mouse handlers can map window positions
//! back into fractions.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    Bounds, ContentMask, Hsla, IntoElement, PathBuilder, Pixels, Point, Styled as _, Window,
    canvas, fill, point, px, size,
};

/// The plot's bounds as of the last paint.
pub(super) type Probe = Rc<Cell<Option<Bounds<Pixels>>>>;

/// Corner radius of a hovered column, matching the app's hover pill.
const HOVER_BAND_RADIUS: Pixels = px(6.0);

/// The hover fill over the column from `x0` to `x1`.
pub(super) fn hover_band(theme: &crate::theme::ArtisanTheme, x0: f32, x1: f32) -> Shape {
    Shape::HoverBand {
        x0,
        x1,
        fill: crate::gradient::hover_fill_gradient(*theme),
        ring: theme.colors.foreground.with_alpha(0.13).to_paint(),
    }
}

/// A fresh, empty probe.
pub(super) fn probe() -> Probe {
    Rc::new(Cell::new(None))
}

/// The plot width as of the last paint, or `fallback` before the first.
pub(super) fn probed_width(probe: &Probe, fallback: f32) -> f32 {
    probe
        .get()
        .map_or(fallback, |bounds| f32::from(bounds.size.width))
}

/// `position` as plot fractions, unclamped; `None` before the first paint.
pub(super) fn fractions(probe: &Probe, position: Point<Pixels>) -> Option<(f32, f32)> {
    let bounds = probe.get()?;
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    Some((
        f32::from(position.x - bounds.origin.x) / width,
        f32::from(position.y - bounds.origin.y) / height,
    ))
}

/// One painted shape, in plot fractions.
#[derive(Clone, Debug)]
pub(super) enum Shape {
    /// A polyline; dashed rules use `dash`.
    Line {
        points: Vec<(f32, f32)>,
        width: f32,
        color: Hsla,
        dash: bool,
    },
    /// A filled polygon.
    Area {
        points: Vec<(f32, f32)>,
        color: Hsla,
    },
    /// A filled rectangle between two corners, at least a pixel wide.
    Rect {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        color: Hsla,
    },
    /// The app's hover fill over a full-height column from `x0` to `x1`:
    /// the hover gradient with its hairline ring, rounded.
    HoverBand {
        x0: f32,
        x1: f32,
        fill: gpui::Background,
        ring: Hsla,
    },
    /// A dot of `radius` pixels.
    Dot {
        x: f32,
        y: f32,
        radius: f32,
        color: Hsla,
    },
}

/// A horizontal rule across the plot at fraction `y`.
pub(super) fn hrule(y: f32, color: Hsla, dash: bool) -> Shape {
    Shape::Line {
        points: vec![(0.0, y), (1.0, y)],
        width: 1.0,
        color,
        dash,
    }
}

/// A vertical rule down the plot at fraction `x`.
pub(super) fn vrule(x: f32, color: Hsla, dash: bool) -> Shape {
    Shape::Line {
        points: vec![(x, 0.0), (x, 1.0)],
        width: 1.0,
        color,
        dash,
    }
}

/// A canvas filling its parent that paints `shapes` clipped to itself and
/// records its bounds in `probe`.
pub(super) fn plot_canvas(shapes: impl Into<Rc<[Shape]>>, probe: Probe) -> impl IntoElement {
    let shapes = shapes.into();
    canvas(
        move |bounds, _, _| probe.set(Some(bounds)),
        move |bounds, (), window, _| {
            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                for shape in shapes.iter() {
                    paint(bounds, shape, window);
                }
            });
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

fn at(bounds: Bounds<Pixels>, (x, y): (f32, f32)) -> Point<Pixels> {
    let x = if x.is_finite() { x } else { 0.0 };
    let y = if y.is_finite() { y } else { 0.0 };
    point(
        bounds.origin.x + bounds.size.width * x,
        bounds.origin.y + bounds.size.height * y,
    )
}

fn trace(bounds: Bounds<Pixels>, points: &[(f32, f32)], builder: &mut PathBuilder) {
    let mut points = points.iter();
    if let Some(first) = points.next() {
        builder.move_to(at(bounds, *first));
    }
    for next in points {
        builder.line_to(at(bounds, *next));
    }
}

fn paint(bounds: Bounds<Pixels>, shape: &Shape, window: &mut Window) {
    match shape {
        Shape::Line {
            points,
            width,
            color,
            dash,
        } => {
            if points.len() < 2 {
                return;
            }
            let mut builder = PathBuilder::stroke(px(*width));
            if *dash {
                builder = builder.dash_array(&[px(4.0), px(3.0)]);
            }
            trace(bounds, points, &mut builder);
            if let Ok(path) = builder.build() {
                window.paint_path(path, *color);
            }
        }
        Shape::Area { points, color } => {
            if points.len() < 3 {
                return;
            }
            let mut builder = PathBuilder::fill();
            trace(bounds, points, &mut builder);
            builder.close();
            if let Ok(path) = builder.build() {
                window.paint_path(path, *color);
            }
        }
        Shape::Rect {
            x0,
            y0,
            x1,
            y1,
            color,
        } => {
            let a = at(bounds, (x0.min(*x1), y0.min(*y1)));
            let b = at(bounds, (x0.max(*x1), y0.max(*y1)));
            let width = (b.x - a.x).max(px(1.0));
            let height = (b.y - a.y).max(px(1.0));
            window.paint_quad(fill(Bounds::new(a, size(width, height)), *color));
        }
        Shape::HoverBand { x0, x1, fill, ring } => {
            let a = at(bounds, (x0.min(*x1), 0.0));
            let b = at(bounds, (x0.max(*x1), 1.0));
            let width = (b.x - a.x).max(px(1.0));
            let band = Bounds::new(a, size(width, b.y - a.y));
            window.paint_quad(gpui::quad(
                band,
                HOVER_BAND_RADIUS,
                *fill,
                px(0.5),
                *ring,
                gpui::BorderStyle::Solid,
            ));
        }
        Shape::Dot {
            x,
            y,
            radius,
            color,
        } => {
            let center = at(bounds, (*x, *y));
            let r = px(*radius);
            let origin = point(center.x - r, center.y - r);
            window.paint_quad(
                fill(Bounds::new(origin, size(r * 2.0, r * 2.0)), *color).corner_radii(r),
            );
        }
    }
}
