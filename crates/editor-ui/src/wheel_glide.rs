//! Wheel ticks eased into a glide.
//!
//! As the previous Editor's transcript and sidebar did
//! (`PickerScrollState`), ticks accumulate into one distance
//! that settles on display frames. Offsets use GPUI's range `[-max, 0]`.

use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WheelGlide {
    /// The distance still to travel. Kept relative, not as a target offset,
    /// so content that moves under a glide doesn't redirect it.
    remaining: f32,
    last_frame: Option<Instant>,
}

impl WheelGlide {
    /// Adds one wheel delta, clamped so the glide ends inside the range.
    pub fn push(&mut self, current: f32, delta: f32, max: f32) {
        if !self.active() {
            self.last_frame = Some(Instant::now());
        }
        let target = (current + self.remaining + delta).clamp(-max.max(0.0), 0.0);
        self.remaining = target - current;
    }

    /// Drops the glide: a trackpad delta or a jump moved the offset itself.
    pub const fn cancel(&mut self) {
        self.remaining = 0.0;
        self.last_frame = None;
    }

    /// The offset for this display frame, if it moves. Elapsed time sets
    /// the step, so refresh rate and dropped frames don't change the speed.
    #[must_use]
    pub fn step(&mut self, current: f32, max: f32) -> Option<f32> {
        let now = Instant::now();
        let elapsed = self
            .last_frame
            .replace(now)
            .map_or(Duration::ZERO, |last| now.saturating_duration_since(last));
        self.step_elapsed(current, max, elapsed)
    }

    fn step_elapsed(&mut self, current: f32, max: f32, elapsed: Duration) -> Option<f32> {
        let current = current.clamp(-max.max(0.0), 0.0);
        let target = (current + self.remaining).clamp(-max.max(0.0), 0.0);
        let distance = target - current;
        if distance.abs() <= 0.25 {
            self.cancel();
            return (distance != 0.0).then_some(target);
        }
        // Master's 60 Hz response: 28% of the way each frame.
        let fraction = 1.0 - 0.72_f32.powf(elapsed.as_secs_f32() * 60.0);
        let moved = distance * fraction;
        self.remaining = distance - moved;
        Some(current + moved)
    }

    /// Whether a frame is still needed.
    #[must_use]
    pub fn active(&self) -> bool {
        self.remaining != 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wheel_tick_glides_to_its_distance_and_stops_at_the_edge() {
        let mut glide = WheelGlide::default();
        glide.push(-100.0, 60.0, 500.0);
        let mut offset = -100.0;
        let mut frames = 0;
        while let Some(next) = glide.step_elapsed(offset, 500.0, Duration::from_millis(16)) {
            assert!(next >= offset, "a glide never reverses");
            offset = next;
            frames += 1;
        }
        assert!((offset - -40.0).abs() < f32::EPSILON);
        assert!(frames > 5, "it eases over several frames, not one jump");
        glide.push(-10.0, 60.0, 500.0);
        assert!(glide.step_elapsed(-10.0, 500.0, Duration::from_secs(1)) <= Some(0.0));
    }
}
