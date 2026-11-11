#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Door {
    Open,
    HeldForReply,
    HeldForBackground,
    Settling,
    Closed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Session {
    pub in_turn: bool,
    pub awaiting_input: bool,
    pub background: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ending {
    Replied,
    Cancelled,
}

impl Door {
    #[must_use]
    pub const fn takes_input(self) -> bool {
        !matches!(self, Self::Closed)
    }

    #[must_use]
    pub const fn after_result(self, session: Session, ending: Ending) -> Self {
        match (self, ending, session.background, session.awaiting_input) {
            (Self::Closed, ..)
            | (_, Ending::Cancelled, ..)
            | (_, Ending::Replied, false, false) => Self::Closed,
            (_, Ending::Replied, true, _) => Self::HeldForBackground,
            (_, Ending::Replied, false, true) => Self::HeldForReply,
        }
    }

    #[must_use]
    pub const fn after_frame(self, session: Session) -> Self {
        match self {
            Self::Open | Self::Closed => self,
            Self::HeldForReply | Self::HeldForBackground | Self::Settling if session.background => {
                Self::HeldForBackground
            }
            Self::HeldForReply | Self::HeldForBackground | Self::Settling if session.in_turn => {
                Self::Open
            }
            Self::HeldForReply if session.awaiting_input => Self::HeldForReply,
            Self::HeldForReply => Self::Closed,
            Self::HeldForBackground | Self::Settling => Self::Settling,
        }
    }

    #[must_use]
    pub const fn after_grace(self, session: Session) -> Self {
        match self.after_frame(session) {
            Self::Settling if session.awaiting_input => Self::HeldForReply,
            Self::Settling => Self::Closed,
            door @ (Self::Open | Self::HeldForReply | Self::HeldForBackground | Self::Closed) => {
                door
            }
        }
    }
}
