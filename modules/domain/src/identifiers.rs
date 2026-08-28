//! Validated identifiers for the first native workflow.
//!
//! Forge owns identity minting: directories, projects, threads, and messages
//! all receive their identities from Forge, never from clients. Clients mint
//! only their own stable request identities so a retried mutation can be
//! recognized and answered with a duplicate receipt instead of a second
//! effect (legacy mints command ids in
//! `modules/frontend/src/lib/root/draft-thread.ts` and detects byte-exact
//! replays in `modules/backend/src/persistence/journal-store.ts`).
//!
//! Every identifier shares one validation rule and one documented UTF-8 byte
//! bound ([`IDENTIFIER_MAX_BYTES`]): non-empty, no Unicode whitespace or
//! control characters anywhere, and bounded. The legacy `Identifier` pattern
//! (`/^\S+$/`) is preserved and tightened with an explicit ceiling.

use std::fmt;
use std::str::FromStr;

use thiserror::Error;

use crate::bounds::IDENTIFIER_MAX_BYTES;

/// Validation failure for a wire-facing identifier.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum IdentifierError {
    /// The supplied value contained no characters at all.
    #[error("identifier must not be empty")]
    Empty,
    /// The supplied value contained a forbidden character.
    #[error("identifier must not contain whitespace or control characters; found {character:?}")]
    ForbiddenCharacter {
        /// The offending Unicode scalar value.
        character: char,
    },
    /// The supplied value exceeded [`IDENTIFIER_MAX_BYTES`].
    #[error("identifier is {length} UTF-8 bytes; the maximum is {maximum}")]
    TooLong {
        /// Offending length in UTF-8 bytes.
        length: usize,
        /// The shared identifier ceiling in UTF-8 bytes.
        maximum: usize,
    },
}

/// Checks an external value against the shared wire-facing identifier rule.
///
/// # Errors
///
/// Returns the first violation found: emptiness, a whitespace or control
/// character, or a length above [`IDENTIFIER_MAX_BYTES`].
fn validate_identifier(value: &str) -> Result<(), IdentifierError> {
    if value.is_empty() {
        return Err(IdentifierError::Empty);
    }

    if let Some(character) = value
        .chars()
        .find(|ch| ch.is_whitespace() || ch.is_control())
    {
        return Err(IdentifierError::ForbiddenCharacter { character });
    }

    let length = value.len();
    if length > IDENTIFIER_MAX_BYTES {
        return Err(IdentifierError::TooLong {
            length,
            maximum: IDENTIFIER_MAX_BYTES,
        });
    }

    Ok(())
}

macro_rules! wire_identifier {
    (
        $(#[$type_docs:meta])*
        $name:ident
    ) => {
        $(#[$type_docs])*
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            /// Creates an identifier after validating the external value.
            ///
            /// # Errors
            ///
            /// Returns [`IdentifierError`] when the value is empty, contains
            /// Unicode whitespace or control characters, or exceeds
            /// [`IDENTIFIER_MAX_BYTES`] UTF-8 bytes.
            pub fn parse(value: impl Into<String>) -> Result<Self, IdentifierError> {
                let value = value.into();
                validate_identifier(&value)?;
                Ok(Self(value))
            }

            /// Returns the validated identifier text.
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = IdentifierError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::parse(value)
            }
        }
    };
}

wire_identifier! {
    /// Stable request identity minted by the client for one mutation.
    ///
    /// A retry of the same logical mutation replays the identical request id,
    /// which lets Forge answer `ReceiptDisposition::Duplicate` instead of
    /// applying the mutation twice. Legacy evidence:
    /// `modules/frontend/src/lib/thread-interaction/commands.ts` (minted by
    /// the route "rather than left to the transport") and
    /// `modules/backend/src/persistence/journal-store.ts` (duplicate
    /// detection keyed on the command id).
    RequestId
}

wire_identifier! {
    /// Opaque identity Forge minted for one visible directory.
    ///
    /// Directories are addressed only by this identity; host path data never
    /// crosses the boundary
    /// (`modules/protocol/src/project-directory.ts`). Legacy directory
    /// identities were process-local registry keys, so a client holding one
    /// across a Forge restart must expect a typed unknown-directory outcome
    /// rather than silently reusing stale state.
    DirectoryId
}

wire_identifier! {
    /// Identity Forge minted for one attached project.
    ///
    /// Minted when a client attaches a directory; detaching and re-attaching
    /// the same folder must resolve to the same project id (legacy keeps this
    /// guarantee through its never-deleted project identity table,
    /// `modules/backend/src/persistence/schema/journal.ts`).
    ProjectId
}

wire_identifier! {
    /// Identity Forge minted for one thread at creation time.
    ///
    /// Never supplied by clients: thread creation carries only a request id,
    /// the owning project id, and a title.
    ThreadId
}

wire_identifier! {
    /// Identity Forge minted for one durably queued message.
    ///
    /// Never supplied by clients: queueing carries only a request id, the
    /// target thread id, and the bounded body.
    MessageId
}

wire_identifier! {
    /// Forge-minted identity of one canonical conversation turn.
    TurnId
}

wire_identifier! {
    /// Forge-minted identity of one renderer-visible conversation item.
    ItemId
}

wire_identifier! {
    /// Forge-minted identity of one replayable conversation patch.
    PatchId
}

wire_identifier! {
    /// Forge-minted opaque routing identity of one assistant run.
    ///
    /// Nonsecret evidence of which run produced a durable assistant item;
    /// never a run lifecycle, lease, credential, engine id, or public state
    /// machine, and never an alias of [`MessageId`] or a protocol frame id.
    /// Never supplied by clients.
    RunId
}
