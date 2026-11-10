use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use aqua_core::geometry::{CornerRadii, Insets};
use aqua_core::scene::DisplayItem;
use aqua_core::vector::{FillRule, GpuiPath, GpuiPathStyle, LineCap, LineJoin, PathCommand};
use aqua_core::{GpuiBackground, GpuiHsla, Point, Rect, Transform2D};

pub(crate) type Probe = Rc<Cell<Option<Rect>>>;

#[derive(Clone, Debug)]
pub(crate) enum Shape {
    Line {
        points: Vec<(f32, f32)>,
        width: f32,
        color: GpuiHsla,
        dash: bool,
    },
    Area {
        points: Vec<(f32, f32)>,
        color: GpuiHsla,
    },
    Rect {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
        color: GpuiHsla,
    },
    HoverBand {
        x0: f32,
        x1: f32,
        fill: GpuiBackground,
        ring: GpuiHsla,
    },
    Dot {
        x: f32,
        y: f32,
        radius: f32,
        color: GpuiHsla,
    },
}

pub(crate) fn fractions(probe: &Probe, position: Point) -> Option<(f32, f32)> {
    let bounds = probe.get()?;
    if bounds.w <= 0.0 || bounds.h <= 0.0 {
        return None;
    }
    Some((
        (position.x - bounds.x) / bounds.w,
        (position.y - bounds.y) / bounds.h,
    ))
}

fn at(bounds: Rect, (x, y): (f32, f32)) -> Point {
    let x = if x.is_finite() { x } else { 0.0 };
    let y = if y.is_finite() { y } else { 0.0 };
    Point::new(bounds.x + bounds.w * x, bounds.y + bounds.h * y)
}

pub(crate) fn source_scene(bounds: Rect, shapes: &[Shape]) -> Vec<DisplayItem> {
    let mut items = vec![DisplayItem::PushClip {
        bounds,
        radius: 0.0,
    }];
    for shape in shapes {
        match shape {
            Shape::Line {
                points,
                width,
                color,
                dash,
            } if points.len() >= 2 => {
                let style = GpuiPathStyle::Stroke {
                    width: *width,
                    cap: LineCap::Butt,
                    join: LineJoin::Miter,
                    dash: if *dash { vec![4.0, 3.0] } else { Vec::new() },
                    dash_offset: 0.0,
                };
                items.push(path_item(
                    bounds,
                    points,
                    style,
                    *color,
                    false,
                    width.abs() * 2.0,
                ));
            }
            Shape::Area { points, color } if points.len() >= 3 => {
                items.push(path_item(
                    bounds,
                    points,
                    GpuiPathStyle::Fill {
                        rule: FillRule::EvenOdd,
                    },
                    *color,
                    true,
                    0.0,
                ));
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
                items.push(DisplayItem::GpuiBackground {
                    bounds: Rect {
                        x: a.x,
                        y: a.y,
                        w: (b.x - a.x).max(1.0),
                        h: (b.y - a.y).max(1.0),
                    },
                    corners: CornerRadii::uniform(0.0),
                    background: GpuiBackground::Solid(*color),
                });
            }
            Shape::HoverBand { x0, x1, fill, ring } => {
                let a = at(bounds, (x0.min(*x1), 0.0));
                let b = at(bounds, (x0.max(*x1), 1.0));
                let band = Rect {
                    x: a.x,
                    y: a.y,
                    w: (b.x - a.x).max(1.0),
                    h: b.y - a.y,
                };
                let corners = CornerRadii::uniform(6.0);
                items.push(DisplayItem::GpuiBackground {
                    bounds: band,
                    corners,
                    background: fill.clone(),
                });
                items.push(DisplayItem::GpuiBorder {
                    bounds: band,
                    corners,
                    widths: Insets::uniform(0.5),
                    color: *ring,
                    dashed: false,
                });
            }
            Shape::Dot {
                x,
                y,
                radius,
                color,
            } => {
                let center = at(bounds, (*x, *y));
                items.push(DisplayItem::GpuiBackground {
                    bounds: Rect {
                        x: center.x - radius,
                        y: center.y - radius,
                        w: radius * 2.0,
                        h: radius * 2.0,
                    },
                    corners: CornerRadii::uniform(*radius),
                    background: GpuiBackground::Solid(*color),
                });
            }
            Shape::Line { .. } | Shape::Area { .. } => {}
        }
    }
    items.push(DisplayItem::PopClip);
    items
}

fn path_item(
    bounds: Rect,
    points: &[(f32, f32)],
    style: GpuiPathStyle,
    color: GpuiHsla,
    close: bool,
    outset: f32,
) -> DisplayItem {
    let local_bounds = Rect {
        x: 0.0,
        y: 0.0,
        w: bounds.w,
        h: bounds.h,
    };
    let points: Vec<Point> = points
        .iter()
        .map(|point| at(local_bounds, *point))
        .collect();
    let mut commands = Vec::with_capacity(points.len() + usize::from(close));
    commands.push(PathCommand::Move(points[0]));
    commands.extend(points.iter().skip(1).copied().map(PathCommand::Line));
    if close {
        commands.push(PathCommand::Close);
    }
    let min_x = points
        .iter()
        .map(|point| point.x)
        .fold(f32::INFINITY, f32::min);
    let max_x = points
        .iter()
        .map(|point| point.x)
        .fold(f32::NEG_INFINITY, f32::max);
    let min_y = points
        .iter()
        .map(|point| point.y)
        .fold(f32::INFINITY, f32::min);
    let max_y = points
        .iter()
        .map(|point| point.y)
        .fold(f32::NEG_INFINITY, f32::max);
    DisplayItem::GpuiPath {
        transformed_window_logical_bounds: Rect::from_edges(
            bounds.x + min_x - outset,
            bounds.y + min_y - outset,
            bounds.x + max_x + outset,
            bounds.y + max_y + outset,
        ),
        path: Arc::new(GpuiPath {
            commands_local: commands,
            origin_window_logical: Point::new(bounds.x, bounds.y),
            element_transform: Transform2D::IDENTITY,
            style,
            background: GpuiBackground::Solid(color),
        }),
    }
}
