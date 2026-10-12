//! The Attachment machine, one per image: Encoding → Uploading → Ready /
//! Failed. The composer owns the bytes and performs the effects.

use artisan_api::BlobHash;
use statig::blocking::{self, Outcome};

#[derive(Debug)]
pub enum Input {
    /// The WebP is ready to upload.
    Encoded,
    Uploaded(BlobHash),
    /// Already on Forge (restored from a saved draft).
    Restored(BlobHash),
    Failed(String),
}

#[derive(Debug, PartialEq, Eq)]
pub enum Effect {
    Upload,
}

#[derive(Default)]
pub struct Machine;

#[blocking::state_machine(initial = "State::encoding()", state(derive(Debug)))]
impl Machine {
    #[state]
    fn encoding(context: &mut Vec<Effect>, event: &Input) -> Outcome<State> {
        match event {
            Input::Encoded => {
                context.push(Effect::Upload);
                Outcome::Transition(State::uploading())
            }
            Input::Restored(hash) => Outcome::Transition(State::ready(hash.clone())),
            Input::Failed(reason) => Outcome::Transition(State::failed(reason.clone())),
            Input::Uploaded(_) => Outcome::Handled,
        }
    }

    #[state]
    fn uploading(event: &Input) -> Outcome<State> {
        match event {
            Input::Uploaded(hash) => Outcome::Transition(State::ready(hash.clone())),
            Input::Failed(reason) => Outcome::Transition(State::failed(reason.clone())),
            _ => Outcome::Handled,
        }
    }

    #[state]
    fn ready(hash: &mut BlobHash, event: &Input) -> Outcome<State> {
        let _ = (hash, event);
        Outcome::Handled
    }

    #[state]
    fn failed(reason: &mut String, event: &Input) -> Outcome<State> {
        let _ = (reason, event);
        Outcome::Handled
    }
}

impl State {
    #[must_use]
    pub fn hash(&self) -> Option<&BlobHash> {
        match self {
            Self::Ready { hash } => Some(hash),
            _ => None,
        }
    }

    #[must_use]
    pub fn failure(&self) -> Option<&str> {
        match self {
            Self::Failed { reason } => Some(reason),
            _ => None,
        }
    }
}
