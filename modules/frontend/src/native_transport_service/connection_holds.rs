//! Connection holds: the async counting lock that keeps one Forge connection
//! open while admitted mutations are still in flight.
//!
//! Any number of holds exist independently and release in any order. The
//! aggregate status is decided only by the live count: busy while any hold is
//! alive, idle at zero. Nobody waits on an individual hold. Release is `Drop`,
//! so a hold cannot be forgotten, is released on panic and cancellation, and
//! cannot be released twice. Sealing refuses new holds so a host switch or an
//! Editor quit can drain the connection before it is closed.

#![forbid(unsafe_code)]

use std::sync::Arc;

use tokio::sync::watch;

use super::{
    ComposerDraftCommand, ComposerStateCommand, ForgeDecisionCommand, NativeTransportCommand,
    PreferencesCommand,
};

/// What an in-flight hold is keeping open, for progress copy only.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum HoldKind {
    /// A first or follow-up message waiting for its Forge receipt.
    Message,
    /// A queued-message withdrawal.
    QueueChange,
    /// A run stop request.
    StopRequest,
    /// An approval or question answer.
    Answer,
    /// A new task in an existing project.
    NewTask,
    /// A project intake (validate, attach, create).
    ProjectIntake,
    /// A thread engine configuration save.
    EngineSettings,
    /// A model favorite change.
    ModelFavorite,
    /// A composer draft save or attachment upload.
    Draft,
    /// A navigation record or preference import.
    Preferences,
}

impl HoldKind {
    /// Every kind in presentation order.
    pub const ALL: [Self; 10] = [
        Self::Message,
        Self::QueueChange,
        Self::StopRequest,
        Self::Answer,
        Self::NewTask,
        Self::ProjectIntake,
        Self::EngineSettings,
        Self::ModelFavorite,
        Self::Draft,
        Self::Preferences,
    ];

    const fn index(self) -> usize {
        self as usize
    }

    /// The user-facing noun for `count` holds of this kind.
    #[must_use]
    pub const fn noun(self, count: usize) -> &'static str {
        let (one, many) = match self {
            Self::Message => ("message", "messages"),
            Self::QueueChange => ("queue change", "queue changes"),
            Self::StopRequest => ("stop request", "stop requests"),
            Self::Answer => ("answer", "answers"),
            Self::NewTask => ("new task", "new tasks"),
            Self::ProjectIntake => ("project", "projects"),
            Self::EngineSettings => ("model setting", "model settings"),
            Self::ModelFavorite => ("favorite", "favorites"),
            Self::Draft => ("draft", "drafts"),
            Self::Preferences => ("preference", "preferences"),
        };
        if count == 1 { one } else { many }
    }
}

/// One immutable observation of a connection's holds.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HoldState {
    /// Live holds. `0` is idle; anything else is busy.
    pub count: usize,
    /// Whether new holds are refused.
    pub sealed: bool,
    kinds: [usize; HoldKind::ALL.len()],
}

impl HoldState {
    /// Whether no hold is alive.
    #[must_use]
    pub const fn is_idle(&self) -> bool {
        self.count == 0
    }

    /// Live holds of one kind.
    #[must_use]
    pub const fn count_of(&self, kind: HoldKind) -> usize {
        self.kinds[kind.index()]
    }

    /// Progress copy such as `2 messages and 1 model setting`, or `None` when idle.
    #[must_use]
    pub fn summary(&self) -> Option<String> {
        let parts: Vec<String> = HoldKind::ALL
            .iter()
            .map(|kind| (*kind, self.count_of(*kind)))
            .filter(|(_, count)| *count > 0)
            .map(|(kind, count)| format!("{count} {}", kind.noun(count)))
            .collect();
        match parts.split_last() {
            None => None,
            Some((last, [])) => Some(last.clone()),
            Some((last, rest)) => Some(format!("{} and {last}", rest.join(", "))),
        }
    }
}

/// The async counting lock owned by one connection.
#[derive(Debug)]
pub struct ConnectionHolds {
    state: watch::Sender<HoldState>,
}

impl ConnectionHolds {
    /// Creates an idle, unsealed lock.
    #[must_use]
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: watch::Sender::new(HoldState::default()),
        })
    }

    /// Acquires one hold, or `None` once the connection is sealed.
    #[must_use]
    pub fn try_hold(self: &Arc<Self>, kind: HoldKind) -> Option<Hold> {
        let admitted = self.state.send_if_modified(|state| {
            if state.sealed {
                return false;
            }
            state.count += 1;
            state.kinds[kind.index()] += 1;
            true
        });
        admitted.then(|| Hold {
            holds: Arc::clone(self),
            kind,
        })
    }

    /// The current state, synchronously, for rendering.
    #[must_use]
    pub fn status(&self) -> HoldState {
        *self.state.borrow()
    }

    /// A receiver that observes every state change.
    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<HoldState> {
        self.state.subscribe()
    }

    /// Resolves once no hold is alive; immediately when already idle.
    pub async fn idle(&self) {
        let mut changes = self.subscribe();
        // The sender lives as long as `self`, so the wait cannot observe closure.
        let _ = changes.wait_for(HoldState::is_idle).await;
    }

    /// Refuses new holds; existing holds complete normally.
    pub fn seal(&self) {
        self.state
            .send_if_modified(|state| !std::mem::replace(&mut state.sealed, true));
    }

    /// Accepts new holds again, for a cancelled switch.
    pub fn unseal(&self) {
        self.state
            .send_if_modified(|state| std::mem::replace(&mut state.sealed, false));
    }
}

/// One live hold. Dropping it releases the hold.
#[must_use = "a hold releases as soon as it is dropped"]
#[derive(Debug)]
pub struct Hold {
    holds: Arc<ConnectionHolds>,
    kind: HoldKind,
}

impl Hold {
    /// What this hold keeps open.
    #[must_use]
    pub const fn kind(&self) -> HoldKind {
        self.kind
    }

    /// Takes one more hold of this kind, even on a sealed connection.
    ///
    /// A live hold keeps the count above zero, so `idle()` cannot have
    /// resolved: extending it lets work already admitted under this hold (a
    /// draft's next coalesced save) finish, and never reopens a drained
    /// connection.
    pub fn extend(&self) -> Self {
        let index = self.kind.index();
        self.holds.state.send_modify(|state| {
            state.count += 1;
            state.kinds[index] += 1;
        });
        Self {
            holds: Arc::clone(&self.holds),
            kind: self.kind,
        }
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        let index = self.kind.index();
        self.holds.state.send_modify(|state| {
            state.count -= 1;
            state.kinds[index] -= 1;
        });
    }
}

/// One admitted command travelling to the service loop with the hold that
/// keeps the connection open until its handler has returned.
#[derive(Debug)]
pub struct QueuedCommand {
    pub(super) command: NativeTransportCommand,
    pub(super) hold: Option<Hold>,
}

impl From<NativeTransportCommand> for QueuedCommand {
    fn from(command: NativeTransportCommand) -> Self {
        Self {
            command,
            hold: None,
        }
    }
}

impl QueuedCommand {
    /// The admitted command.
    #[must_use]
    pub const fn command(&self) -> &NativeTransportCommand {
        &self.command
    }

    /// The hold admitted with the command, if it mutates Forge state.
    #[must_use]
    pub const fn hold(&self) -> Option<&Hold> {
        self.hold.as_ref()
    }
}

impl NativeTransportCommand {
    /// The hold a command takes when admitted. Mutations hold; reads,
    /// subscriptions, acknowledgements, and shutdown never do.
    #[must_use]
    pub const fn hold_kind(&self) -> Option<HoldKind> {
        match self {
            Self::QueueFirstMessage(_) | Self::SubmitComposerDraft(_) => Some(HoldKind::Message),
            Self::ComposerState(
                ComposerStateCommand::WithdrawQueuedMessage { .. }
                | ComposerStateCommand::RetryFailedMessage { .. },
            ) => Some(HoldKind::QueueChange),
            Self::StopRun(_) => Some(HoldKind::StopRequest),
            Self::RespondApproval(_) | Self::RespondQuestion(_) => Some(HoldKind::Answer),
            Self::CreateTask(_) | Self::RecoverFailedMessage { .. } => Some(HoldKind::NewTask),
            Self::BeginProjectIntake | Self::BeginProjectIntakeAt(_) | Self::RetryProjectIntake => {
                Some(HoldKind::ProjectIntake)
            }
            Self::SetThreadEngineConfig(_) => Some(HoldKind::EngineSettings),
            Self::SetModelFavorite(_) => Some(HoldKind::ModelFavorite),
            Self::ComposerDraft(
                ComposerDraftCommand::Save { .. } | ComposerDraftCommand::Upload { .. },
            ) => Some(HoldKind::Draft),
            Self::Preferences(
                PreferencesCommand::RecordNavigation(_) | PreferencesCommand::ImportLegacy(_),
            ) => Some(HoldKind::Preferences),
            Self::ComposerState(
                ComposerStateCommand::ReadFooterUsage { .. }
                | ComposerStateCommand::ReadRunUsage { .. },
            )
            | Self::ComposerDraft(
                ComposerDraftCommand::Read(_) | ComposerDraftCommand::ReadAttachment { .. },
            )
            | Self::ForgeDecision(
                ForgeDecisionCommand::ReadHostCatalog
                | ForgeDecisionCommand::ResolveModelSelection(_)
                | ForgeDecisionCommand::ResolveEngineConfiguration(_),
            )
            | Self::Preferences(PreferencesCommand::Read)
            | Self::ReadActiveRun { .. }
            | Self::SelectProject(_)
            | Self::ReadSidebarThreads { .. }
            | Self::RequestSnapshot(_)
            | Self::ReadMessageImage(_)
            | Self::LoadThreadEngineSettings { .. }
            | Self::ReadComposerCatalog { .. }
            | Self::ReadModelFavorites { .. }
            | Self::ListRegisteredProfiles
            | Self::ReadAccountUsage { .. }
            | Self::ResolveRichLink { .. }
            | Self::QueryProjectRepository { .. }
            | Self::Subscribe { .. }
            | Self::Unsubscribe { .. }
            | Self::AcknowledgePatch { .. }
            | Self::Shutdown => None,
        }
    }
}

#[cfg(test)]
#[path = "connection_holds_tests.rs"]
mod tests;
