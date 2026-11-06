use std::time::{Duration, Instant};

const LINE_UNIT: f32 = 16.0;
const NOTCH_LINES: f32 = 3.0;
const LINE_HEIGHT: f32 = 16.0 * 1.618_034;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WheelInput {
    Notch(f32),
    Precise(f32),
}

impl WheelInput {
    #[must_use]
    pub fn classify(delta_y: f32) -> Option<Self> {
        if delta_y == 0.0 || !delta_y.is_finite() {
            return None;
        }
        let lines = delta_y / LINE_UNIT;
        Some(if lines.abs() >= 1.0 && lines.fract() == 0.0 {
            Self::Notch(lines * NOTCH_LINES * LINE_HEIGHT)
        } else {
            Self::Precise(delta_y)
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WheelGlide {
    remaining: f32,
    last_frame: Option<Instant>,
}

impl WheelGlide {
    pub fn push(&mut self, current: f32, delta: f32, max: f32) {
        if !self.active() {
            self.last_frame = Some(Instant::now());
        }
        if self.remaining.signum() != delta.signum() {
            self.remaining = 0.0;
        }
        let target = (current + self.remaining + delta).clamp(-max.max(0.0), 0.0);
        self.remaining = target - current;
    }

    pub const fn cancel(&mut self) {
        self.remaining = 0.0;
        self.last_frame = None;
    }

    #[must_use]
    pub fn step(&mut self, current: f32, max: f32) -> Option<f32> {
        let now = Instant::now();
        let elapsed = self
            .last_frame
            .replace(now)
            .map_or(Duration::ZERO, |last| now.saturating_duration_since(last));
        self.step_elapsed(current, max, elapsed)
    }

    pub fn push_down(&mut self, offset: f32, delta: f32, max: f32) {
        self.push(-offset, -delta, max);
    }

    #[must_use]
    pub fn step_down(&mut self, offset: f32, max: f32) -> Option<f32> {
        self.step(-offset, max).map(|next| -next)
    }

    #[must_use]
    pub fn step_down_elapsed(&mut self, offset: f32, max: f32, elapsed: Duration) -> Option<f32> {
        self.step_elapsed(-offset, max, elapsed).map(|next| -next)
    }

    pub fn step_elapsed(&mut self, current: f32, max: f32, elapsed: Duration) -> Option<f32> {
        let current = current.clamp(-max.max(0.0), 0.0);
        let target = (current + self.remaining).clamp(-max.max(0.0), 0.0);
        let distance = target - current;
        if distance.abs() <= 0.25 {
            self.cancel();
            return (distance != 0.0).then_some(target);
        }
        let fraction = 1.0 - (-elapsed.as_secs_f32() / 0.09).exp();
        let moved = distance * fraction;
        self.remaining = distance - moved;
        Some(current + moved)
    }

    #[must_use]
    pub fn active(&self) -> bool {
        self.remaining != 0.0
    }
}
