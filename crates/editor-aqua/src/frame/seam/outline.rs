use aqua_core::vector::PathCommand;
use crate::frame::strip::RADIUS as TAB;
use crate::ui::kit::Radius;
use aqua_core::{Point, Rect};

fn arc(radius: f32, sweep: bool, x: f32, y: f32) -> PathCommand {
    match radius > 0.0 {
        true => PathCommand::Arc {
            radii: Point::new(radius, radius),
            rotation_radians: 0.0,
            large: false,
            sweep,
            to: Point::new(x, y),
        },
        false => PathCommand::Line(Point::new(x, y)),
    }
}

pub fn trace(body: Rect, tab: Option<Rect>, inset: f32) -> Vec<PathCommand> {
    let x0 = body.x + inset;
    let y0 = body.y + inset;
    let x1 = (body.right() - inset).max(x0);
    let y1 = (body.bottom() - inset).max(y0);
    let radius = (Radius::Shell.px() - inset)
        .max(0.0)
        .min((x1 - x0) * 0.5)
        .min((y1 - y0) * 0.5);
    let line = |x, y| PathCommand::Line(Point::new(x, y));
    let tab = tab.filter(|tab| tab.right() > x0 && tab.x < x1 && tab.y + inset < y0);
    let mut path = Vec::new();
    let (left_corner, right_corner) = match tab {
        Some(tab) => {
            let left = (tab.x + inset).clamp(x0, x1);
            let right = (tab.right() - inset).clamp(left, x1);
            let top = tab.y + inset;
            let cap = (TAB.px() - inset)
                .max(0.0)
                .min((right - left) * 0.5)
                .min((y0 - top) * 0.5);
            let left_fillet = (TAB.px() + inset).min((left - x0) * 0.5).min((y0 - top) * 0.5);
            let right_fillet = (TAB.px() + inset).min((x1 - right) * 0.5).min((y0 - top) * 0.5);
            let left_corner = radius.min((left - x0 - left_fillet).max(0.0));
            let right_corner = radius.min((x1 - right - right_fillet).max(0.0));
            path.extend([
                PathCommand::Move(Point::new(x0 + left_corner, y0)),
                line(left - left_fillet, y0),
                arc(left_fillet, false, left, y0 - left_fillet),
                line(left, top + cap),
                arc(cap, true, left + cap, top),
                line(right - cap, top),
                arc(cap, true, right, top + cap),
                line(right, y0 - right_fillet),
                arc(right_fillet, false, right + right_fillet, y0),
            ]);
            (left_corner, right_corner)
        }
        None => {
            path.push(PathCommand::Move(Point::new(x0 + radius, y0)));
            (radius, radius)
        }
    };
    path.extend([
        line(x1 - right_corner, y0),
        arc(right_corner, true, x1, y0 + right_corner),
        line(x1, y1 - radius),
        arc(radius, true, x1 - radius, y1),
        line(x0 + radius, y1),
        arc(radius, true, x0, y1 - radius),
        line(x0, y0 + left_corner),
        arc(left_corner, true, x0 + left_corner, y0),
        PathCommand::Close,
    ]);
    path
}

pub fn split(body: Rect, tab: Option<Rect>, inset: f32) -> (Vec<PathCommand>, Rect) {
    let full = trace(body, tab, inset);
    let x0 = body.x + inset;
    let x1 = (body.right() - inset).max(x0);
    let y0 = body.y + inset;
    let y1 = (body.bottom() - inset).max(y0);
    let radius = (Radius::Shell.px() - inset)
        .max(0.0)
        .min((x1 - x0) * 0.5)
        .min((y1 - y0) * 0.5);
    let cut = (y0 + radius + 1.0).min(y1 - radius);
    if cut <= y0 {
        return (full, Rect::ZERO);
    }
    let mut top = full[..full.len() - 7].to_vec();
    top.extend([
        PathCommand::Line(Point::new(x1, cut)),
        PathCommand::Line(Point::new(x0, cut)),
    ]);
    top.extend_from_slice(&full[full.len() - 3..]);
    (top, Rect::from_edges(x0, cut - 1.0, x1, y1))
}
