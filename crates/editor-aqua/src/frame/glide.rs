use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CubicBezier {
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
}

impl CubicBezier {
    #[must_use]
    pub const fn new(x1: f64, y1: f64, x2: f64, y2: f64) -> Self {
        Self { x1, y1, x2, y2 }
    }

    #[must_use]
    pub fn sample(self, progress: f64) -> f64 {
        match progress {
            t if t <= 0.0 => 0.0,
            t if t >= 1.0 => 1.0,
            t => curve(self.y1, self.y2, self.solve_x(t)),
        }
    }

    fn solve_x(self, x: f64) -> f64 {
        let newton = (0..8).try_fold(x, |t, _| {
            let error = curve(self.x1, self.x2, t) - x;
            let slope = slope(self.x1, self.x2, t);
            match (error.abs() < 1e-7, slope.abs() < 1e-6) {
                (true, _) => Err(Some(t)),
                (false, true) => Err(None),
                (false, false) => Ok(t - error / slope),
            }
        });
        match newton {
            Err(Some(t)) => t,
            Ok(_) | Err(None) => self.bisect(x),
        }
    }

    fn bisect(self, x: f64) -> f64 {
        let (low, high) = (0..64).fold((0.0_f64, 1.0_f64), |(low, high), _| {
            let middle = f64::midpoint(low, high);
            if curve(self.x1, self.x2, middle) < x {
                (middle, high)
            } else {
                (low, middle)
            }
        });
        f64::midpoint(low, high)
    }
}

fn curve(p1: f64, p2: f64, t: f64) -> f64 {
    let inverse = 1.0 - t;
    (3.0 * inverse * inverse * t).mul_add(p1, (3.0 * inverse * t * t).mul_add(p2, t * t * t))
}

fn slope(p1: f64, p2: f64, t: f64) -> f64 {
    let inverse = 1.0 - t;
    (3.0 * inverse * inverse).mul_add(p1, (6.0 * inverse * t).mul_add(p2 - p1, 3.0 * t * t * (1.0 - p2)))
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transition {
    pub duration: Duration,
    pub easing: CubicBezier,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Run {
    from: f32,
    to: f32,
    started: Instant,
    duration: Duration,
    shortening: f64,
}

impl Run {
    fn progress(&self, now: Instant) -> f64 {
        match self.duration.as_secs_f64() {
            total if total <= 0.0 => 1.0,
            total => (now.saturating_duration_since(self.started).as_secs_f64() / total).min(1.0),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Glide {
    Resting(f32),
    Running(Run),
}

impl Glide {
    #[must_use]
    pub fn value(&self, now: Instant, transition: Transition) -> f32 {
        match self {
            Self::Resting(value) => *value,
            Self::Running(run) => match run.progress(now) {
                progress if progress >= 1.0 => run.to,
                progress => {
                    let eased = transition.easing.sample(progress) as f32;
                    (run.to - run.from).mul_add(eased, run.from)
                }
            },
        }
    }

    #[must_use]
    pub const fn target(&self) -> f32 {
        match self {
            Self::Resting(value) => *value,
            Self::Running(run) => run.to,
        }
    }

    #[must_use]
    pub fn retarget(self, to: f32, now: Instant, transition: Transition, reduced: bool) -> Self {
        if reduced {
            return Self::Resting(to);
        }
        let from = self.value(now, transition);
        let shortening = match self {
            Self::Running(run) if (run.from - to).abs() < f32::EPSILON => {
                let output = transition.easing.sample(run.progress(now));
                output.mul_add(run.shortening, 1.0 - run.shortening).clamp(0.0, 1.0)
            }
            Self::Running(_) | Self::Resting(_) => 1.0,
        };
        Self::Running(Run {
            from,
            to,
            started: now,
            duration: transition.duration.mul_f64(shortening),
            shortening,
        })
    }

    #[must_use]
    pub fn settle(self, now: Instant) -> Self {
        match self {
            Self::Running(run) if run.progress(now) >= 1.0 => Self::Resting(run.to),
            glide => glide,
        }
    }

    #[must_use]
    pub const fn running(&self) -> bool {
        matches!(self, Self::Running(_))
    }
}
