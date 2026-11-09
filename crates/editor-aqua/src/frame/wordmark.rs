use crate::frame::scramble::{self, Cell, Rows};
use std::time::{Duration, Instant};

pub const REST: [&str; 3] = ["AR", "TIS", "AN"];
pub const SWAP: [&str; 3] = ["TO", "GG", "LE"];
pub const REST_SQUEEZED: [&str; 3] = ["AR", "TS", "AN"];
pub const LINE_HEIGHTS: [f32; 3] = [0.71, 0.8, 0.7];
pub const LEAVE_DELAY: Duration = Duration::from_millis(120);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Face {
    Rest,
    Swap,
}

#[derive(Clone, Debug, PartialEq)]
enum Motion {
    Settled(Rows),
    Scrambling {
        cells: Vec<Vec<Cell>>,
        settled: Rows,
        started: Instant,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Wordmark {
    face: Face,
    motion: Motion,
    leaving: Option<Instant>,
}

impl Default for Wordmark {
    fn default() -> Self {
        Self {
            face: Face::Rest,
            motion: Motion::Settled(scramble::rows(&REST)),
            leaving: None,
        }
    }
}

impl Wordmark {
    #[must_use]
    pub const fn face(&self) -> Face {
        self.face
    }

    #[must_use]
    pub fn shown(&self, now: Instant) -> Rows {
        match &self.motion {
            Motion::Settled(rows) => rows.clone(),
            Motion::Scrambling {
                cells,
                settled,
                started,
            } => {
                let elapsed = elapsed_ms(*started, now);
                if elapsed >= scramble::total_ms(cells) {
                    settled.clone()
                } else {
                    scramble::frame_at(cells, elapsed)
                }
            }
        }
    }

    pub fn enter(&mut self, now: Instant, reduced: bool) {
        self.leaving = None;
        if self.face == Face::Swap {
            return;
        }
        self.face = Face::Swap;
        self.play(&SWAP, &SWAP, now, reduced);
    }

    pub fn leave(&mut self, now: Instant) {
        self.leaving = Some(now + LEAVE_DELAY);
    }

    pub fn advance(&mut self, now: Instant, reduced: bool) {
        if let Some(at) = self.leaving.filter(|at| *at <= now) {
            self.leaving = None;
            if self.face == Face::Swap {
                self.face = Face::Rest;
                self.play(&REST_SQUEEZED, &REST, at, reduced);
            }
        }
        if let Motion::Scrambling {
            cells,
            settled,
            started,
        } = &self.motion
            && elapsed_ms(*started, now) >= scramble::total_ms(cells)
        {
            self.motion = Motion::Settled(settled.clone());
        }
    }

    #[must_use]
    pub const fn scrambling(&self) -> bool {
        matches!(self.motion, Motion::Scrambling { .. })
    }

    #[must_use]
    pub const fn deadline(&self) -> Option<Instant> {
        self.leaving
    }

    fn play(&mut self, target: &[&str], last: &[&str], now: Instant, reduced: bool) {
        let settled = scramble::rows(last);
        if reduced {
            self.motion = Motion::Settled(settled);
            return;
        }
        let from = scramble::squeeze(&self.shown(now));
        self.motion = Motion::Scrambling {
            cells: scramble::cells_between(&from, target),
            settled,
            started: now,
        };
    }
}

fn elapsed_ms(started: Instant, now: Instant) -> f64 {
    now.saturating_duration_since(started).as_secs_f64() * 1000.0
}
