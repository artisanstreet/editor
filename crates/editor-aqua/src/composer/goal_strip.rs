use crate::ui::motion::MotionCurve;
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GoalStrip {
    shown: Option<ComposerGoal>,
    grown_at: Option<Instant>,
    leaving_at: Option<Instant>,
    swap: Option<(Instant, ComposerGoal)>,
}

pub struct GoalStripFrame {
    pub reveal: f32,
    pub goal: ComposerGoal,
    pub label_opacity: f32,
    pub label_offset: f32,
    pub moving: bool,
}

impl GoalStrip {
    pub fn set(&mut self, goal: Option<ComposerGoal>, now: Instant, reduced: bool) -> bool {
        match (goal, self.shown.clone()) {
            (None, None) => false,
            (None, Some(_)) => {
                if self.leaving_at.is_some() {
                    return false;
                }
                if reduced {
                    *self = Self::default();
                } else {
                    self.leaving_at = Some(now);
                    self.swap = None;
                }
                true
            }
            (Some(goal), None) => {
                self.shown = Some(goal);
                self.grown_at = (!reduced).then_some(now);
                self.leaving_at = None;
                self.swap = None;
                true
            }
            (Some(goal), Some(shown)) => {
                if self.leaving_at.take().is_some() {
                    self.grown_at = (!reduced).then_some(now);
                    self.shown = Some(goal);
                    return true;
                }
                if goal == shown {
                    return false;
                }
                if !reduced {
                    self.swap = Some((now, shown));
                }
                self.shown = Some(goal);
                true
            }
        }
    }

    pub fn frame(&mut self, now: Instant, reduced: bool) -> Option<GoalStripFrame> {
        if reduced {
            if self.leaving_at.is_some() {
                *self = Self::default();
            } else {
                self.grown_at = None;
                self.swap = None;
            }
        }
        let goal = self.shown.clone()?;
        let mut reveal = 1.0;
        let mut label = 1.0;
        let mut moving = false;
        if let Some(leaving) = self.leaving_at {
            let elapsed = now.saturating_duration_since(leaving);
            label = 1.0 - progress(elapsed, Duration::ZERO, Duration::from_millis(150));
            reveal = 1.0
                - eased(
                    MotionCurve::SmoothOut,
                    progress(
                        elapsed,
                        Duration::from_millis(150),
                        Duration::from_millis(350),
                    ),
                );
            if elapsed >= Duration::from_millis(500) {
                *self = Self::default();
                return None;
            }
            moving = true;
        } else if let Some(grown) = self.grown_at {
            let elapsed = now.saturating_duration_since(grown);
            reveal = eased(
                MotionCurve::SmoothOut,
                progress(elapsed, Duration::ZERO, Duration::from_millis(400)),
            );
            label = eased(
                MotionCurve::SmoothOut,
                progress(
                    elapsed,
                    Duration::from_millis(250),
                    Duration::from_millis(250),
                ),
            );
            if elapsed >= Duration::from_millis(500) {
                self.grown_at = None;
            } else {
                moving = true;
            }
        }
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
            reveal,
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

#[derive(Clone, Debug, Default, PartialEq, Eq)]
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

    pub fn set_parked_time(
        &mut self,
        until: Option<artisan_api::Timestamp>,
        now: Instant,
        reduced: bool,
    ) -> bool {
        self.set_parked(until, crate::ui::local_time::moment, now, reduced)
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
