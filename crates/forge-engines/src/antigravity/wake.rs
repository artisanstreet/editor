use std::time::{Duration, Instant};

pub(super) const REPLY_SETTLE: Duration = Duration::from_secs(3);
pub(super) const OPEN_REPLY_SILENCE: Duration = Duration::from_mins(2);
pub(super) const COMMAND_SILENCE: Duration = Duration::from_mins(30);
pub(super) const PROMPT_SILENCE: Duration = Duration::from_mins(15);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Wake {
    #[default]
    Quiet,
    Prompted {
        heard: Instant,
    },
    Open {
        heard: Instant,
    },
    Closed {
        heard: Instant,
    },
    Pending {
        heard: Instant,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Seen {
    PromptSent,
    PromptEnded { lingers: bool },
    Output,
    StepEnded,
    Progress,
    Probed { pending: bool },
    Lapsed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Hold {
    Free,
    Commands,
    Schedule,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Due {
    Probe,
    Lapse,
    Abandon,
}

impl Wake {
    #[must_use]
    pub(super) const fn next(self, seen: Seen, now: Instant) -> Self {
        match (self, seen) {
            (_, Seen::PromptSent) => Self::Prompted { heard: now },
            (Self::Prompted { .. }, Seen::PromptEnded { lingers: false })
            | (Self::Closed { .. }, Seen::Probed { pending: false })
            | (Self::Open { .. } | Self::Closed { .. } | Self::Pending { .. }, Seen::Lapsed) => {
                Self::Quiet
            }
            (Self::Prompted { .. }, Seen::PromptEnded { lingers: true })
            | (Self::Open { .. } | Self::Closed { .. } | Self::Pending { .. }, Seen::StepEnded) => {
                Self::Closed { heard: now }
            }
            (
                Self::Quiet | Self::Open { .. } | Self::Closed { .. } | Self::Pending { .. },
                Seen::Output,
            ) => Self::Open { heard: now },
            (Self::Closed { .. }, Seen::Probed { pending: true }) => Self::Pending { heard: now },
            (state, Seen::Output | Seen::StepEnded | Seen::Progress) => state.heard_at(now),
            (state, Seen::PromptEnded { .. } | Seen::Probed { .. } | Seen::Lapsed) => state,
        }
    }

    const fn heard_at(self, now: Instant) -> Self {
        match self {
            Self::Quiet => Self::Quiet,
            Self::Prompted { .. } => Self::Prompted { heard: now },
            Self::Open { .. } => Self::Open { heard: now },
            Self::Closed { .. } => Self::Closed { heard: now },
            Self::Pending { .. } => Self::Pending { heard: now },
        }
    }

    #[must_use]
    pub(super) fn due(self, hold: Hold, busy: bool) -> Option<(Instant, Due)> {
        match (self, hold, busy) {
            (Self::Quiet, ..) | (Self::Prompted { .. }, _, true) | (_, Hold::Schedule, _) => None,
            (Self::Prompted { heard }, _, false) => Some((heard + PROMPT_SILENCE, Due::Abandon)),
            (
                Self::Open { heard } | Self::Closed { heard } | Self::Pending { heard },
                Hold::Commands,
                _,
            ) => Some((heard + COMMAND_SILENCE, Due::Lapse)),
            (Self::Open { heard } | Self::Pending { heard }, Hold::Free, _) => {
                Some((heard + OPEN_REPLY_SILENCE, Due::Lapse))
            }
            (Self::Closed { heard }, Hold::Free, _) => Some((heard + REPLY_SETTLE, Due::Probe)),
        }
    }

    #[must_use]
    pub(super) const fn reply_closed(self) -> bool {
        matches!(self, Self::Closed { .. } | Self::Pending { .. })
    }

    #[must_use]
    pub(super) const fn after_prompt(self) -> bool {
        matches!(
            self,
            Self::Open { .. } | Self::Closed { .. } | Self::Pending { .. }
        )
    }
}
