use std::time::Instant;

use aqua_core::{Point, vector::PathCommand};

use crate::ui::motion::{MotionDuration, smooth_out};

#[derive(Clone, Copy, Debug, PartialEq)]
struct Slide {
    from: f32,
    started: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SlidingTab {
    top: f32,
    target: f32,
    height: f32,
    slide: Option<Slide>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bounds {
    pub width: f32,
    pub height: f32,
    pub split: f32,
    pub radius: f32,
}

impl SlidingTab {
    #[must_use]
    pub const fn new(top: f32, height: f32) -> Self {
        Self {
            top,
            target: top,
            height,
            slide: None,
        }
    }

    pub fn aim(&mut self, target: f32, now: Instant) {
        if target != self.target {
            self.target = target;
            self.slide = Some(Slide {
                from: self.top,
                started: now,
            });
        }
    }

    pub fn resize(&mut self, height: f32) {
        self.height = height;
    }

    pub fn snap(&mut self) {
        self.top = self.target;
        self.slide = None;
    }

    #[must_use]
    pub const fn settled(&self) -> bool {
        self.slide.is_none()
    }

    #[must_use]
    pub fn span(&self) -> (f32, f32) {
        (self.top, self.top + self.height)
    }

    pub fn advance(&mut self, now: Instant) {
        let Some(Slide { from, started }) = self.slide else {
            return;
        };
        let progress = now.saturating_duration_since(started).as_secs_f32()
            / MotionDuration::Fast.as_duration().as_secs_f32();
        if progress >= 1.0 {
            self.snap();
        } else {
            self.top = (self.target - from).mul_add(smooth_out(progress), from);
        }
    }

    #[must_use]
    pub fn cutout(&self, bounds: Bounds, header: f32, inset: f32) -> Vec<PathCommand> {
        let mut path = self.outline(bounds);
        if header <= 0.0 {
            return path;
        }
        let left = bounds.split + inset;
        let right = bounds.width - inset;
        let top = inset;
        let bottom = header;
        let radius = (bounds.radius - inset)
            .max(0.0)
            .min((right - left).max(0.0) / 2.0)
            .min((bottom - top).max(0.0) / 2.0);
        path.extend([
            PathCommand::Move(Point::new(left + radius, top)),
            PathCommand::Line(Point::new(right - radius, top)),
            arc(radius, true, right, top + radius),
            PathCommand::Line(Point::new(right, bottom - radius)),
            arc(radius, true, right - radius, bottom),
            PathCommand::Line(Point::new(left + radius, bottom)),
            arc(radius, true, left, bottom - radius),
            PathCommand::Line(Point::new(left, top + radius)),
            arc(radius, true, left + radius, top),
            PathCommand::Close,
        ]);
        path
    }

    #[must_use]
    pub fn outline(&self, bounds: Bounds) -> Vec<PathCommand> {
        let Bounds {
            width,
            height,
            split,
            radius,
        } = bounds;
        let (top, bottom) = self.span();
        let top = top.max(0.0);
        let joint = radius.min(top / 2.0);
        let cap = radius.min(self.height / 3.0);
        vec![
            PathCommand::Move(Point::new(split + joint, 0.0)),
            PathCommand::Line(Point::new(width - radius, 0.0)),
            arc(radius, true, width, radius),
            PathCommand::Line(Point::new(width, height - radius)),
            arc(radius, true, width - radius, height),
            PathCommand::Line(Point::new(split + radius, height)),
            arc(radius, true, split, height - radius),
            PathCommand::Line(Point::new(split, bottom + radius)),
            arc(radius, false, split - radius, bottom),
            PathCommand::Line(Point::new(cap, bottom)),
            arc(cap, true, 0.0, bottom - cap),
            PathCommand::Line(Point::new(0.0, top + cap)),
            arc(cap, true, cap, top),
            PathCommand::Line(Point::new(split - joint, top)),
            arc(joint, false, split, top - joint),
            PathCommand::Line(Point::new(split, joint)),
            arc(joint, true, split + joint, 0.0),
            PathCommand::Close,
        ]
    }
}

fn arc(radius: f32, sweep: bool, x: f32, y: f32) -> PathCommand {
    if radius < 0.01 {
        return PathCommand::Line(Point::new(x, y));
    }
    PathCommand::Arc {
        radii: Point::new(radius, radius),
        rotation_radians: 0.0,
        large: false,
        sweep,
        to: Point::new(x, y),
    }
}
