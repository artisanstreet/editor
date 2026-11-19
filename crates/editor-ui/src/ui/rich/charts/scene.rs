use std::cell::Cell;
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

#[derive(Clone)]
struct LocalPath {
    commands: Vec<PathCommand>,
    min: Point,
    max: Point,
}

impl LocalPath {
    fn new(size: (f32, f32), points: &[(f32, f32)], close: bool) -> Self {
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
        Self { commands, min, max }
    }

    fn item(
        &self,
        bounds: Rect,
        style: GpuiPathStyle,
        color: GpuiHsla,
        outset: f32,
    ) -> DisplayItem {
        DisplayItem::GpuiPath {
            transformed_window_logical_bounds: Rect::from_edges(
                bounds.x + self.min.x - outset,
                bounds.y + self.min.y - outset,
                bounds.x + self.max.x + outset,
                bounds.y + self.max.y + outset,
            ),
            path: Arc::new(GpuiPath {
                commands_local: self.commands.clone(),
                origin_window_logical: Point::new(bounds.x, bounds.y),
                element_transform: Transform2D::IDENTITY,
                style,
                background: GpuiBackground::Solid(color),
            }),
        }
    }
}

type CachedPaths = ((u32, u32), Vec<Option<LocalPath>>);

#[derive(Default)]
pub struct SceneCache {
    paths: std::cell::RefCell<Option<CachedPaths>>,
}

impl std::fmt::Debug for SceneCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SceneCache").finish_non_exhaustive()
    }
}

impl SceneCache {
    fn path<R>(
        &self,
        size: (f32, f32),
        shape: usize,
        points: &[(f32, f32)],
        close: bool,
        use_path: impl FnOnce(&LocalPath) -> R,
    ) -> R {
        let key = (size.0.to_bits(), size.1.to_bits());
        let mut paths = self.paths.borrow_mut();
        if paths.as_ref().is_none_or(|(cached, _)| *cached != key) {
            *paths = Some((key, Vec::new()));
        }
        let (_, entries) = paths.as_mut().expect("invariant: the cache was just sized");
        if entries.len() <= shape {
            entries.resize(shape + 1, None);
        }
        use_path(entries[shape].get_or_insert_with(|| LocalPath::new(size, points, close)))
    }
}

pub fn source_scene(bounds: Rect, shapes: &[Shape]) -> Vec<DisplayItem> {
    cached_scene(bounds, shapes, &SceneCache::default())
}

#[expect(
    clippy::too_many_lines,
    reason = "one match converting every chart shape kind into cached display items"
)]
pub fn cached_scene(bounds: Rect, shapes: &[Shape], cache: &SceneCache) -> Vec<DisplayItem> {
    let size = (bounds.w, bounds.h);
    let mut items = vec![DisplayItem::PushClip {
        bounds,
        radius: 0.0,
    }];
    for (index, shape) in shapes.iter().enumerate() {
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
                items.push(cache.path(size, index, points, false, |path| {
                    path.item(bounds, style, *color, width.abs() * 2.0)
                }));
            }
            Shape::Area { points, color } if points.len() >= 3 => {
                items.push(cache.path(size, index, points, true, |path| {
                    path.item(
                        bounds,
                        GpuiPathStyle::Fill {
                            rule: FillRule::EvenOdd,
                        },
                        *color,
                        0.0,
                    )
                }));
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
    items.push(DisplayItem::PopClip);
    items
}
