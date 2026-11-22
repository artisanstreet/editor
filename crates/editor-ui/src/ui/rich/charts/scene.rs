use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use aqua_core::geometry::{CornerRadii, Insets};
use aqua_core::scene::DisplayItem;
use aqua_core::vector::{FillRule, GpuiPath, GpuiPathStyle, LineCap, LineJoin, PathCommand};
use aqua_core::{GpuiBackground, GpuiHsla, Point, Rect, Transform2D};

pub(crate) type Probe = Rc<Cell<Option<Rect>>>;

#[derive(Clone, Debug)]
pub enum Shape {
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
    Point::new(bounds.w.mul_add(x, bounds.x), bounds.h.mul_add(y, bounds.y))
}

fn local_path(
    size: (f32, f32),
    points: &[(f32, f32)],
    close: bool,
    style: GpuiPathStyle,
    color: GpuiHsla,
    outset: f32,
) -> DisplayItem {
    let local = Rect {
        x: 0.0,
        y: 0.0,
        w: size.0,
        h: size.1,
    };
    let mut commands = Vec::with_capacity(points.len() + usize::from(close));
    let mut min = Point::new(f32::INFINITY, f32::INFINITY);
    let mut max = Point::new(f32::NEG_INFINITY, f32::NEG_INFINITY);
    for (index, point) in points.iter().enumerate() {
        let point = at(local, *point);
        min = Point::new(min.x.min(point.x), min.y.min(point.y));
        max = Point::new(max.x.max(point.x), max.y.max(point.y));
        commands.push(if index == 0 {
            PathCommand::Move(point)
        } else {
            PathCommand::Line(point)
        });
    }
    if close {
        commands.push(PathCommand::Close);
    }
    DisplayItem::GpuiPath {
        transformed_window_logical_bounds: Rect::from_edges(
            min.x - outset,
            min.y - outset,
            max.x + outset,
            max.y + outset,
        ),
        path: Arc::new(GpuiPath {
            commands_local: commands,
            origin_window_logical: Point::new(0.0, 0.0),
            element_transform: Transform2D::IDENTITY,
            style,
            background: GpuiBackground::Solid(color),
        }),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Clip {
    Bounds,
    Overflow,
}

struct Local {
    size: (u32, u32),
    items: Vec<DisplayItem>,
}

pub struct Scene {
    draw: Box<dyn Fn((f32, f32)) -> Vec<DisplayItem>>,
    clip: Clip,
    local: RefCell<Option<Local>>,
}

impl std::fmt::Debug for Scene {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scene")
            .field("clip", &self.clip)
            .finish_non_exhaustive()
    }
}

impl Scene {
    pub fn of_shapes(shapes: impl Into<Rc<[Shape]>>, clip: Clip) -> Self {
        let shapes = shapes.into();
        Self::drawn(move |size| shape_items(size, &shapes), clip)
    }

    pub(crate) fn drawn(
        draw: impl Fn((f32, f32)) -> Vec<DisplayItem> + 'static,
        clip: Clip,
    ) -> Self {
        Self {
            draw: Box::new(draw),
            clip,
            local: RefCell::new(None),
        }
    }

    pub fn paint(&self, bounds: Rect) -> Vec<DisplayItem> {
        let size = (bounds.w.to_bits(), bounds.h.to_bits());
        let mut local = self.local.borrow_mut();
        if local.as_ref().is_none_or(|drawn| drawn.size != size) {
            *local = Some(Local {
                size,
                items: (self.draw)((bounds.w, bounds.h)),
            });
        }
        let items = &local
            .as_ref()
            .expect("invariant: the local scene was just drawn")
            .items;
        let clipped = self.clip == Clip::Bounds;
        let mut painted = Vec::with_capacity(items.len() + 4);
        if clipped {
            painted.push(DisplayItem::PushClip {
                bounds,
                radius: 0.0,
            });
        }
        painted.push(DisplayItem::PushTransform(Transform2D::translate(
            bounds.x, bounds.y,
        )));
        painted.extend(items.iter().cloned());
        painted.push(DisplayItem::PopTransform);
        if clipped {
            painted.push(DisplayItem::PopClip);
        }
        painted
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "one match converting every chart shape kind into local display items"
)]
pub(crate) fn shape_items(size: (f32, f32), shapes: &[Shape]) -> Vec<DisplayItem> {
    let bounds = Rect {
        x: 0.0,
        y: 0.0,
        w: size.0,
        h: size.1,
    };
    let mut items = Vec::with_capacity(shapes.len());
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
                items.push(local_path(
                    size,
                    points,
                    false,
                    style,
                    *color,
                    width.abs() * 2.0,
                ));
            }
            Shape::Area { points, color } if points.len() >= 3 => {
                items.push(local_path(
                    size,
                    points,
                    true,
                    GpuiPathStyle::Fill {
                        rule: FillRule::EvenOdd,
                    },
                    *color,
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
                    background: *fill,
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
    items
}
