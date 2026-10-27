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
        // Reversing the wheel should respond immediately, without first
        // spending the distance queued in the previous direction.
        if self.remaining.signum() != delta.signum() {
            self.remaining = 0.0;
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
        // A 90 ms response settles gently in either direction. Elapsed time
        // keeps the speed independent of the display's refresh rate.
        let fraction = 1.0 - (-elapsed.as_secs_f32() / 0.09).exp();
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
    fn reversing_direction_drops_the_previous_ticks_backlog() {
        let mut glide = WheelGlide::default();
        glide.push(-250.0, -100.0, 500.0);
        glide.push(-250.0, 30.0, 500.0);
        let next = glide
            .step_elapsed(-250.0, 500.0, Duration::from_millis(16))
            .unwrap();
        assert!(next > -250.0, "the next frame follows the new direction");
        assert!(next < -220.0, "the new direction still eases in");
    }

    #[test]
    fn equal_elapsed_time_travels_the_same_distance_at_different_refresh_rates() {
        let travel = |frame_ms, frames| {
            let mut glide = WheelGlide::default();
            glide.push(-250.0, -100.0, 500.0);
            let mut current = -250.0;
            for _ in 0..frames {
                current = glide
                    .step_elapsed(current, 500.0, Duration::from_millis(frame_ms))
                    .unwrap();
            }
            current
        };
        assert!((travel(8, 12) - travel(16, 6)).abs() < 0.001);
    }

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
        assert!(
            (offset - -40.0).abs() < 0.001,
            "settles within a thousandth of a pixel"
        );
        assert!(frames > 5, "it eases over several frames, not one jump");
        glide.push(-10.0, 60.0, 500.0);
        assert!(glide.step_elapsed(-10.0, 500.0, Duration::from_secs(1)) <= Some(0.0));
    }
}
