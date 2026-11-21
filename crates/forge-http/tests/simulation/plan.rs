#[derive(Debug, Clone)]
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) const fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut mixed = self.0;
        mixed = (mixed ^ (mixed >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        mixed = (mixed ^ (mixed >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        mixed ^ (mixed >> 31)
    }

    pub(crate) fn below(&mut self, bound: u64) -> u64 {
        self.next() % bound
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ending {
    Replies,
    EngineDies,
    ForgeDies(Stop),
    ForgeDiesBeforeSession(Stop),
}

impl Ending {
    pub(crate) const fn leaves_session(self, resumed: bool) -> bool {
        match self {
            Self::Replies | Self::EngineDies | Self::ForgeDies(_) => true,
            Self::ForgeDiesBeforeSession(_) => resumed,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stop {
    Graceful,
    Killed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Delivery {
    Clean,
    ResponseLost,
    Aborted { after_ms: u64 },
    DiskFails,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Toggle {
    ContinueAfterRestart(bool),
    Archived(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Policy {
    continue_after_restart: bool,
    archived: bool,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            continue_after_restart: true,
            archived: false,
        }
    }
}

impl Policy {
    pub(crate) const fn toggle(&mut self, toggle: Toggle) {
        match toggle {
            Toggle::ContinueAfterRestart(on) => self.continue_after_restart = on,
            Toggle::Archived(on) => self.archived = on,
        }
    }

    pub(crate) const fn carries_on(self) -> bool {
        self.continue_after_restart && !self.archived
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    Turn {
        number: usize,
        delivery: Delivery,
        ending: Ending,
    },
    Draft {
        number: usize,
        disk_fails: bool,
    },
    Restart(Stop),
    StepClock {
        by_ms: i64,
    },
    DropEvents {
        after: usize,
    },
    Toggle(Toggle),
}

const MIN_ACTIONS: u64 = 6;
const MORE_ACTIONS: u64 = 8;

pub(crate) fn plan(seed: u64) -> Vec<Action> {
    let mut rng = Rng::new(seed);
    let count = MIN_ACTIONS + rng.below(MORE_ACTIONS);
    let mut turns = 0;
    let mut drafts = 0;
    let mut actions: Vec<Action> = (0..count)
        .map(|_| match rng.below(12) {
            0..=4 => {
                turns += 1;
                Action::Turn {
                    number: turns,
                    delivery: delivery(&mut rng),
                    ending: ending(&mut rng),
                }
            }
            5 | 6 => {
                drafts += 1;
                Action::Draft {
                    number: drafts,
                    disk_fails: rng.chance(30),
                }
            }
            7 => Action::Restart(stop(&mut rng)),
            8 => Action::StepClock {
                by_ms: i64::try_from(rng.below(20_001)).expect("a small step") - 10_000,
            },
            9 => Action::Toggle(Toggle::ContinueAfterRestart(rng.chance(50))),
            10 => Action::Toggle(Toggle::Archived(rng.chance(50))),
            _ => Action::DropEvents {
                after: usize::try_from(rng.below(6)).expect("a small count"),
            },
        })
        .collect();
    if turns == 0 {
        actions.push(Action::Turn {
            number: 1,
            delivery: delivery(&mut rng),
            ending: Ending::Replies,
        });
    }
    actions
}

fn delivery(rng: &mut Rng) -> Delivery {
    match rng.below(8) {
        0 => Delivery::ResponseLost,
        1 => Delivery::Aborted {
            after_ms: rng.below(40),
        },
        2 => Delivery::DiskFails,
        _ => Delivery::Clean,
    }
}

fn ending(rng: &mut Rng) -> Ending {
    match rng.below(7) {
        0 => Ending::EngineDies,
        1 => Ending::ForgeDies(stop(rng)),
        2 => Ending::ForgeDiesBeforeSession(stop(rng)),
        _ => Ending::Replies,
    }
}

fn stop(rng: &mut Rng) -> Stop {
    if rng.chance(50) {
        Stop::Killed
    } else {
        Stop::Graceful
    }
}
