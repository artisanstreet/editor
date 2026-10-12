//! The Send machine: Draft → Sending → Sent / Failed { retry }.
//!
//! It holds only the id of the message in flight; the text and images stay
//! in the editor and attachments until Forge accepted them. A retry after a
//! failure reuses the id, so Forge stores the message at most once.

use artisan_api::ItemId;
use statig::blocking::{self, Outcome};

#[derive(Debug)]
pub enum Input {
    /// Send (or retry) the message with this id.
    Send(ItemId),
    /// Forge stored it.
    Accepted,
    /// Creating the thread or sending failed; says what and why.
    Rejected(String),
    /// The user changed the draft after a send or failure.
    Edited,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    /// Create the thread if needed, then post the message with this id.
    Post(ItemId),
    /// Forge has it: clear the draft.
    Clear,
}

#[derive(Default)]
pub struct Machine;

#[blocking::state_machine(initial = "State::draft()", state(derive(Debug)))]
impl Machine {
    #[state]
    fn draft(context: &mut Vec<Effect>, event: &Input) -> Outcome<State> {
        match event {
            Input::Send(id) => {
                context.push(Effect::Post(id.clone()));
                Outcome::Transition(State::sending(id.clone()))
            }
            _ => Outcome::Handled,
        }
    }

    #[state]
    fn sending(id: &mut ItemId, context: &mut Vec<Effect>, event: &Input) -> Outcome<State> {
        match event {
            Input::Accepted => {
                context.push(Effect::Clear);
                Outcome::Transition(State::sent())
            }
            Input::Rejected(error) => Outcome::Transition(State::failed(id.clone(), error.clone())),
            _ => Outcome::Handled,
        }
    }

    #[state]
    fn sent(context: &mut Vec<Effect>, event: &Input) -> Outcome<State> {
        match event {
            Input::Send(id) => {
                context.push(Effect::Post(id.clone()));
                Outcome::Transition(State::sending(id.clone()))
            }
            Input::Edited => Outcome::Transition(State::draft()),
            _ => Outcome::Handled,
        }
    }

    /// `id` is what a retry sends again.
    #[state]
    fn failed(
        id: &mut ItemId,
        error: &mut String,
        context: &mut Vec<Effect>,
        event: &Input,
    ) -> Outcome<State> {
        let _ = error;
        match event {
            Input::Send(_) => {
                context.push(Effect::Post(id.clone()));
                Outcome::Transition(State::sending(id.clone()))
            }
            Input::Edited => Outcome::Transition(State::draft()),
            _ => Outcome::Handled,
        }
    }
}

impl State {
    #[must_use]
    pub fn is_sending(&self) -> bool {
        matches!(self, Self::Sending { .. })
    }

    /// Whether the message with `id` is the one in flight.
    #[must_use]
    pub fn is_sending_id(&self, id: &ItemId) -> bool {
        matches!(self, Self::Sending { id: sending } if sending == id)
    }

    /// The id a retry reuses.
    #[must_use]
    pub fn retry_id(&self) -> Option<&ItemId> {
        match self {
            Self::Failed { id, .. } => Some(id),
            _ => None,
        }
    }

    #[must_use]
    pub fn error(&self) -> Option<&str> {
        match self {
            Self::Failed { error, .. } => Some(error),
            _ => None,
        }
    }
}
