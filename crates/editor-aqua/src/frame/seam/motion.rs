use crate::frame::glide::{CubicBezier, Glide, Transition};
use aqua_core::Rect;
use std::time::{Duration, Instant};

const SLIDE: Transition = Transition {
    duration: Duration::from_millis(250),
    easing: CubicBezier::new(0.22, 1.0, 0.36, 1.0),
};
const CLOSE: Transition = Transition {
    duration: Duration::from_millis(150),
    ..SLIDE
};

#[derive(Clone, Copy, Debug, PartialEq)]
struct Target {
    body: Rect,
    tab: Option<(u64, Rect)>,
}

#[derive(Clone, Copy, Debug)]
pub struct Motion {
    target: Option<Target>,
    left: Glide,
    right: Glide,
    height: Glide,
}

impl Default for Motion {
    fn default() -> Self {
        Self {
            target: None,
            left: Glide::Resting(0.0),
            right: Glide::Resting(0.0),
            height: Glide::Resting(0.0),
        }
    }
}

impl Motion {
    pub fn target(&mut self, body: Rect, tab: Option<(u64, Rect)>, now: Instant, reduced: bool) {
        self.advance(now, false);
        let next = Target { body, tab };
        let resized = self.target.is_none_or(|previous| previous.body != body);
        if self.target == Some(next) && !reduced {
            return;
        }
        let (left, right, height) = match tab {
            Some((_, tab)) => (tab.x, tab.right(), (body.y - tab.y).max(0.0)),
            None => (
                self.left.value(now, SLIDE),
                self.right.value(now, SLIDE),
                0.0,
            ),
        };
        match resized || reduced {
            true => {
                self.left = Glide::Resting(left);
                self.right = Glide::Resting(right);
                self.height = Glide::Resting(height);
            }
            false => {
                if self.target.is_some_and(|previous| previous.tab.is_none()) && !self.animating() {
                    self.left = Glide::Resting(left);
                    self.right = Glide::Resting(right);
                }
                let transition = match tab {
                    Some(_) => SLIDE,
                    None => CLOSE,
                };
                self.left = self.left.retarget(left, now, transition, false);
                self.right = self.right.retarget(right, now, transition, false);
                self.height = self.height.retarget(height, now, transition, false);
            }
        }
        self.target = Some(next);
    }

    pub fn destination(&self) -> Option<Rect> {
        let body = self.target?.body;
        Some(Rect::from_edges(
            self.left.target(),
            body.y - self.height.target(),
            self.right.target(),
            body.y,
        ))
    }

    pub fn sample(&self, now: Instant) -> Option<Rect> {
        let body = self.target?.body;
        let height = self.height.value(now, SLIDE);
        (height > 0.001).then(|| {
            Rect::from_edges(
                self.left.value(now, SLIDE),
                body.y - height,
                self.right.value(now, SLIDE),
                body.y,
            )
        })
    }

    pub fn advance(&mut self, now: Instant, reduced: bool) -> bool {
        let changed = self.animating();
        for glide in [&mut self.left, &mut self.right, &mut self.height] {
            *glide = match reduced {
                true => Glide::Resting(glide.target()),
                false => glide.settle(now),
            };
        }
        changed
    }

    pub fn animating(&self) -> bool {
        self.left.running() || self.right.running() || self.height.running()
    }
}
