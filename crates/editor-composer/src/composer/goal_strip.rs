use editor_ui::motion::MotionCurve;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ComposerGoalState {
    Active,
    Paused,
    Blocked,
    UsageLimited,
    BudgetLimited,
    SafetyHolding,
    SafetyRerouted,
    SafetyRefused,
    Note,
    HeadsUp,
    Parked,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposerGoal {
    pub objective: String,
    pub state: ComposerGoalState,
    pub detail: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Level {
    reveal: f32,
    label: f32,
}

impl Level {
    const HIDDEN: Self = Self {
        reveal: 0.0,
        label: 0.0,
    };
    const SHOWN: Self = Self {
        reveal: 1.0,
        label: 1.0,
    };
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Shown,
    Growing { started: Instant, from: Level },
    Leaving { started: Instant, from: Level },
}

impl Phase {
    fn level(self, now: Instant) -> Level {
        match self {
            Self::Shown => Level::SHOWN,
            Self::Growing { started, from } => {
                let elapsed = now.saturating_duration_since(started);
                let reveal = eased(
                    MotionCurve::SmoothOut,
                    progress(elapsed, Duration::ZERO, Duration::from_millis(400)),
                );
                let label = eased(
                    MotionCurve::SmoothOut,
                    progress(
                        elapsed,
                        Duration::from_millis(250),
                        Duration::from_millis(250),
                    ),
                );
                Level {
                    reveal: (1.0 - from.reveal).mul_add(reveal, from.reveal),
                    label: (1.0 - from.label).mul_add(label, from.label),
                }
            }
            Self::Leaving { started, from } => {
                let elapsed = now.saturating_duration_since(started);
                let label = progress(elapsed, Duration::ZERO, Duration::from_millis(150));
                let reveal = eased(
                    MotionCurve::SmoothOut,
                    progress(
                        elapsed,
                        Duration::from_millis(150),
                        Duration::from_millis(350),
                    ),
                );
                Level {
                    reveal: from.reveal * (1.0 - reveal),
                    label: from.label * (1.0 - label),
                }
            }
        }
    }

    fn finished(self, now: Instant) -> bool {
        match self {
            Self::Shown => true,
            Self::Growing { started, .. } | Self::Leaving { started, .. } => {
                now.saturating_duration_since(started) >= Duration::from_millis(500)
            }
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GoalStrip {
    shown: Option<(ComposerGoal, Phase)>,
    swap: Option<(Instant, ComposerGoal)>,
}

#[derive(Debug)]
pub struct GoalStripFrame {
    pub reveal: f32,
    pub goal: ComposerGoal,
    pub label_opacity: f32,
    pub label_offset: f32,
    pub moving: bool,
}

impl GoalStrip {
    fn set(&mut self, goal: Option<ComposerGoal>, now: Instant, reduced: bool) -> bool {
        match (goal, self.shown.take()) {
            (None, None) => false,
            (None, Some((shown, phase @ Phase::Leaving { .. }))) => {
                self.shown = Some((shown, phase));
                false
            }
            (None, Some((shown, phase))) => {
                if reduced {
                    *self = Self::default();
                } else {
                    self.shown = Some((
                        shown,
                        Phase::Leaving {
                            started: now,
                            from: phase.level(now),
                        },
                    ));
                    self.swap = None;
                }
                true
            }
            (Some(goal), None) => {
                self.shown = Some((goal, Self::grow(Level::HIDDEN, now, reduced)));
                self.swap = None;
                true
            }
            (Some(goal), Some((_, phase @ Phase::Leaving { .. }))) => {
                self.shown = Some((goal, Self::grow(phase.level(now), now, reduced)));
                true
            }
            (Some(goal), Some((shown, phase))) => {
                if goal == shown {
                    self.shown = Some((shown, phase));
                    return false;
                }
                if !reduced {
                    self.swap = Some((now, shown));
                }
                self.shown = Some((goal, phase));
                true
            }
        }
    }

    fn grow(from: Level, now: Instant, reduced: bool) -> Phase {
        if reduced {
            Phase::Shown
        } else {
            Phase::Growing { started: now, from }
        }
    }

    pub fn frame(&mut self, now: Instant, reduced: bool) -> Option<GoalStripFrame> {
        if reduced {
            self.swap = None;
            self.shown = match self.shown.take() {
                Some((_, Phase::Leaving { .. })) | None => None,
                Some((goal, _)) => Some((goal, Phase::Shown)),
            };
        }
        let (goal, phase) = self.shown.clone()?;
        let level = phase.level(now);
        let mut moving = !phase.finished(now);
        match (phase, moving) {
            (Phase::Leaving { .. }, false) => {
                *self = Self::default();
                return None;
            }
            (Phase::Growing { .. }, false) => self.shown = Some((goal.clone(), Phase::Shown)),
            _ => {}
        }
        let label = level.label;
        let (goal, label_opacity, label_offset) = match self.swap.clone() {
            Some((started, from)) => {
                let swap = progress(
                    now.saturating_duration_since(started),
                    Duration::ZERO,
                    Duration::from_millis(300),
                );
                if swap >= 1.0 {
                    self.swap = None;
                    (goal, label, 0.0)
                } else if swap < 0.5 {
                    moving = true;
                    let out = eased(MotionCurve::EaseInOut, swap * 2.0);
                    (from, 1.0 - out, -4.0 * out)
                } else {
                    moving = true;
                    let enter = eased(MotionCurve::EaseInOut, swap * 2.0 - 1.0);
                    (goal, enter, 4.0 * (1.0 - enter))
                }
            }
            None => (goal, label, 4.0 * (1.0 - label)),
        };
        Some(GoalStripFrame {
            reveal: level.reveal,
            goal,
            label_opacity,
            label_offset,
            moving,
        })
    }
}

fn progress(elapsed: Duration, delay: Duration, duration: Duration) -> f32 {
    let Some(run) = elapsed.checked_sub(delay) else {
        return 0.0;
    };
    (run.as_secs_f32() / duration.as_secs_f32()).clamp(0.0, 1.0)
}
fn eased(curve: MotionCurve, value: f32) -> f32 {
    curve.sample_f32(f64::from(value))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComposerNote {
    pub id: String,
    pub heads_up: bool,
    pub text: String,
}

#[expect(
    clippy::partial_pub_fields,
    reason = "composer tests drive the strip motion directly; the rest of the strip state stays private"
)]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StripState {
    pub motion: GoalStrip,
    goal: Option<ComposerGoal>,
    note: Option<ComposerNote>,
    closed_note: Option<String>,
    parked: Option<(artisan_api::Timestamp, String)>,
    settled: bool,
}

impl StripState {
    pub fn set_goal(&mut self, goal: Option<ComposerGoal>, now: Instant, reduced: bool) -> bool {
        self.goal = goal;
        self.update(now, reduced)
    }

    pub fn set_note(&mut self, note: Option<ComposerNote>, now: Instant, reduced: bool) -> bool {
        self.note = note;
        self.update(now, reduced)
    }

    pub fn set_parked(
        &mut self,
        until: Option<artisan_api::Timestamp>,
        format: impl FnOnce(artisan_api::Timestamp) -> String,
        now: Instant,
        reduced: bool,
    ) -> bool {
        if self.parked.as_ref().map(|(at, _)| *at) == until {
            return false;
        }
        self.parked = until.map(|at| (at, format(at)));
        self.update(now, reduced)
    }

    pub fn settle(&mut self) {
        self.settled = true;
    }

    pub fn close_note(&mut self, now: Instant, reduced: bool) -> bool {
        self.closed_note = self.note.as_ref().map(|note| note.id.clone());
        self.update(now, reduced)
    }

    fn update(&mut self, now: Instant, reduced: bool) -> bool {
        let parked = self.parked.as_ref().map(|(_, formatted)| ComposerGoal {
            objective: format!("This thread will pick up at {formatted}"),
            state: ComposerGoalState::Parked,
            detail: None,
        });
        let note = self
            .note
            .as_ref()
            .filter(|note| self.closed_note.as_ref() != Some(&note.id))
            .map(|note| ComposerGoal {
                objective: note.text.clone(),
                state: if note.heads_up {
                    ComposerGoalState::HeadsUp
                } else {
                    ComposerGoalState::Note
                },
                detail: None,
            });
        self.motion.set(
            parked.or(note).or_else(|| self.goal.clone()),
            now,
            reduced || !self.settled,
        )
    }
}

#[cfg(test)]
#[path = "../../tests/composer/goal_strip.rs"]
mod tests;
