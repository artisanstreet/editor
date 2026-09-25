//! A user's model selection as catalog identities, and the Forge's typed
//! refusals of a selection or a send.
//!
//! The Editor never builds or validates an engine configuration: it names
//! the catalog model and the option identities the picker shows, and the
//! Forge resolves them against its own catalog into the durable
//! [`EngineRunConfig`] (or refuses with a presentation-ready reason).

use std::fmt;

use thiserror::Error;

use crate::identifiers::{EngineProfileId, ThreadId};
use crate::{EngineRunConfig, ModelFavoriteId};

/// Maximum UTF-8 byte length of one catalog option identity.
pub const CATALOG_OPTION_ID_MAX_BYTES: usize = 256;

/// Maximum UTF-8 byte length of a refusal message.
pub const SUBMISSION_REFUSAL_MESSAGE_MAX_BYTES: usize = 1_024;

/// One catalog option identity (a reasoning, speed, context-window, or
/// permission option id).
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct CatalogOptionId(String);

impl CatalogOptionId {
    /// Validates one option identity.
    ///
    /// # Errors
    ///
    /// Returns [`CatalogSelectionError`] for an empty value, a control
    /// character, or a value over [`CATALOG_OPTION_ID_MAX_BYTES`].
    pub fn parse(value: impl Into<String>) -> Result<Self, CatalogSelectionError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > CATALOG_OPTION_ID_MAX_BYTES
            || value.chars().any(char::is_control)
        {
            return Err(CatalogSelectionError::OptionId);
        }
        Ok(Self(value))
    }

    /// Returns the option identity.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Invalid selection or refusal value at a boundary.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
pub enum CatalogSelectionError {
    /// An option identity was empty, too long, or held control characters.
    #[error("catalog option id is empty, too long, or has control characters")]
    OptionId,
    /// A refusal message was blank, too long, or held control characters.
    #[error("refusal message is blank, too long, or has control characters")]
    RefusalMessage,
}

/// The model a user selected, as catalog identities.
///
/// Each option is the id of the option the picker shows, `None` when the
/// picker shows none; the Forge looks every id up in its own catalog.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct CatalogSelection {
    /// Catalog model identity.
    pub model_id: ModelFavoriteId,
    /// Engine profile the selection names, when the catalog scope has one.
    pub profile_id: Option<EngineProfileId>,
    /// Reasoning-effort option.
    pub reasoning_effort: Option<CatalogOptionId>,
    /// Speed option.
    pub speed: Option<CatalogOptionId>,
    /// Context-window option.
    pub context_window: Option<CatalogOptionId>,
    /// Permission option.
    pub permission: Option<CatalogOptionId>,
}

/// Asks the Forge to resolve a selection for a thread into the engine
/// configuration it would run, without saving anything.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ResolveModelSelection {
    /// Thread whose saved configuration and catalog the selection resolves
    /// against.
    pub thread_id: ThreadId,
    /// The user's selection.
    pub selection: CatalogSelection,
}

/// The Forge's answer to [`ResolveModelSelection`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModelSelectionResolution {
    /// Thread the selection was resolved for.
    pub thread_id: ThreadId,
    /// The selection as asked, so a late answer can be matched.
    pub selection: CatalogSelection,
    /// The resolved configuration or the refusal.
    pub outcome: Result<EngineRunConfig, SubmissionRefusal>,
}

/// Why the Forge refused a selection or a send.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SubmissionRefusalKind {
    /// The selection does not name a model and options the catalog can run.
    InvalidSelection,
    /// The thread has no saved configuration and the send named none.
    NoSelection,
    /// The selected engine's account cannot run right now.
    EngineNotReady,
    /// The thread's run is still starting.
    RunStarting,
    /// An attached image cannot be sent to the thread's engine.
    AttachmentRejected,
}

impl fmt::Display for SubmissionRefusalKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidSelection => "invalid_selection",
            Self::NoSelection => "no_selection",
            Self::EngineNotReady => "engine_not_ready",
            Self::RunStarting => "run_starting",
            Self::AttachmentRejected => "attachment_rejected",
        })
    }
}

/// A typed refusal with the message a client shows as it is.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SubmissionRefusal {
    kind: SubmissionRefusalKind,
    message: String,
}

impl SubmissionRefusal {
    /// Creates one refusal.
    ///
    /// # Errors
    ///
    /// Returns [`CatalogSelectionError::RefusalMessage`] for a blank
    /// message, control characters, or a message over
    /// [`SUBMISSION_REFUSAL_MESSAGE_MAX_BYTES`].
    pub fn new(
        kind: SubmissionRefusalKind,
        message: impl Into<String>,
    ) -> Result<Self, CatalogSelectionError> {
        let message = message.into();
        if message.trim().is_empty()
            || message.len() > SUBMISSION_REFUSAL_MESSAGE_MAX_BYTES
            || message.chars().any(char::is_control)
        {
            return Err(CatalogSelectionError::RefusalMessage);
        }
        Ok(Self { kind, message })
    }

    /// Returns the refusal kind.
    #[must_use]
    pub const fn kind(&self) -> SubmissionRefusalKind {
        self.kind
    }

    /// Returns the presentation-ready message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}
