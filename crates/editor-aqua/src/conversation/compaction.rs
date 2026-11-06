use crate::ui::motion::MotionCurve;
use std::time::{Duration, Instant};

const COMPACTION_OPEN: Duration = Duration::from_millis(250);
const COMPACTION_DRAW_DELAY: Duration = Duration::from_millis(150);
const COMPACTION_DRAW: Duration = Duration::from_millis(400);
const COMPACTION_LABEL_DELAY: Duration = Duration::from_millis(230);
const COMPACTION_LABEL_RISE: Duration = Duration::from_millis(250);
const COMPACTION_SWAP_HALF: Duration = Duration::from_millis(150);
const COMPACTION_PULSE: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompactionPhase {
    Running,
    Done,
    Stopped,
}

impl CompactionPhase {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Running => "Compacting",
            Self::Done => "Compacted",
            Self::Stopped => "Compaction stopped",
        }
    }
}

#[derive(Debug)]
pub struct CompactionMotion {
    phase: CompactionPhase,
    entered: Option<Instant>,
    settled: Option<(Instant, CompactionPhase)>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CompactionFrame {
    pub open: f32,
    pub draw: f32,
    pub label: f32,
    pub swap: Option<(CompactionPhase, f32)>,
    pub pulse: Option<f32>,
}

impl CompactionFrame {
    const REST: Self = Self {
        open: 1.0,
        draw: 1.0,
        label: 1.0,
        swap: None,
        pulse: None,
    };
}

impl CompactionMotion {
    pub fn first_seen(phase: CompactionPhase, enter: bool, now: Instant) -> Self {
        Self {
            phase,
            entered: (enter && phase == CompactionPhase::Running).then_some(now),
            settled: None,
        }
    }

    pub fn advance(
        &mut self,
        phase: CompactionPhase,
        full_motion: bool,
        now: Instant,
    ) -> (CompactionFrame, bool) {
        if phase != self.phase {
            self.settled = (full_motion
                && self.phase == CompactionPhase::Running
                && phase == CompactionPhase::Done)
                .then_some((now, self.phase));
            self.phase = phase;
        }
        if !full_motion {
            self.entered = None;
            self.settled = None;
        }
        let mut frame = CompactionFrame::REST;
        if let Some(entered) = self.entered {
            let elapsed = now.saturating_duration_since(entered);
            frame.open = eased(
                MotionCurve::SmoothOut,
                linear(elapsed, Duration::ZERO, COMPACTION_OPEN),
            );
            frame.draw = eased(
                MotionCurve::SmoothOut,
                linear(elapsed, COMPACTION_DRAW_DELAY, COMPACTION_DRAW),
            );
            frame.label = eased(
                MotionCurve::SmoothOut,
                linear(elapsed, COMPACTION_LABEL_DELAY, COMPACTION_LABEL_RISE),
            );
            if elapsed
                >= (COMPACTION_DRAW_DELAY + COMPACTION_DRAW)
                    .max(COMPACTION_LABEL_DELAY + COMPACTION_LABEL_RISE)
            {
                self.entered = None;
            }
        }
        if let Some((settled, from)) = self.settled {
            let elapsed = now.saturating_duration_since(settled);
            let swap = linear(elapsed, Duration::ZERO, COMPACTION_SWAP_HALF * 2);
            if swap < 1.0 {
                frame.swap = Some((from, swap));
            }
            let pulse = linear(elapsed, COMPACTION_SWAP_HALF * 2, COMPACTION_PULSE);
            if pulse > 0.0 && pulse < 1.0 {
                frame.pulse = Some(pulse);
            }
            if elapsed >= COMPACTION_SWAP_HALF * 2 + COMPACTION_PULSE {
                self.settled = None;
            }
        }
        (
            frame,
            frame != CompactionFrame::REST || self.entered.is_some() || self.settled.is_some(),
        )
    }
}

fn linear(elapsed: Duration, delay: Duration, duration: Duration) -> f32 {
    let Some(run) = elapsed.checked_sub(delay) else {
        return 0.0;
    };
    (run.as_secs_f32() / duration.as_secs_f32()).clamp(0.0, 1.0)
}

#[expect(clippy::cast_possible_truncation, reason = "progress stays in 0..=1")]
pub fn eased(curve: MotionCurve, value: f32) -> f32 {
    curve.sample(f64::from(value)) as f32
}
