//! Renderer-facing assistant message values.
//!
//! These values share the user-message item shape wherever their meaning is
//! the same and add exactly what distinguishes assistant output: the
//! Forge-minted run that produced the item and the renderer-disclosed text
//! phase. A run id is opaque routing evidence, never execution state; a
//! phase classifies only the text a renderer was given, never hidden
//! reasoning, and it stays independent of the item lifecycle (`Final` does
//! not imply [`ConversationLifecycle::Completed`]).
//!
//! Assistant bodies deliberately accept empty and whitespace-only text: a
//! stored assistant row may exist before any visible token arrived, so the
//! opening body can legitimately be empty. The bound stays the shared
//! 65,536 UTF-8-byte body ceiling, measured in native bytes.

use std::fmt;

use thiserror::Error;

use crate::bounds::MESSAGE_BODY_MAX_BYTES;
use crate::conversation::{ConversationLifecycle, ItemOrdinal, Revision};
use crate::identifiers::{ItemId, RunId, TurnId};
use crate::time::UnixMillis;

/// Validation failure for [`AssistantBody`].
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum AssistantBodyError {
    /// The body exceeded its documented UTF-8 byte ceiling.
    #[error("assistant body is {length} UTF-8 bytes; the maximum is {maximum}")]
    TooLong {
        /// Offending length in UTF-8 bytes.
        length: usize,
        /// The shared body ceiling ([`MESSAGE_BODY_MAX_BYTES`]).
        maximum: usize,
    },
}

/// Complete bounded text of one renderer-visible assistant item.
///
/// Unlike the nonblank user [`MessageBody`](crate::text::MessageBody), this
/// value permits empty and whitespace-only text and preserves every accepted
/// byte exactly: no trimming, normalization, or truncation ever applies.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AssistantBody(String);

impl AssistantBody {
    /// Maximum UTF-8 byte length accepted for this value.
    pub const MAX_BYTES: usize = MESSAGE_BODY_MAX_BYTES;

    /// Creates the value after validating the external text.
    ///
    /// # Errors
    ///
    /// Returns [`AssistantBodyError::TooLong`] carrying only the offending
    /// length when the text exceeds `MAX_BYTES` UTF-8 bytes. Empty and
    /// whitespace-only text stay valid.
    pub fn parse(value: impl Into<String>) -> Result<Self, AssistantBodyError> {
        let value = value.into();
        let length = value.len();
        if length > Self::MAX_BYTES {
            return Err(AssistantBodyError::TooLong {
                length,
                maximum: Self::MAX_BYTES,
            });
        }
        Ok(Self(value))
    }

    /// Returns the validated text exactly as supplied.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AssistantBody {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Renderer-disclosed display phase of one assistant message's text.
///
/// This classifies only the text a renderer was given to show; hidden
/// reasoning is out of scope. The phase is independent of the item
/// lifecycle: `Final` marks settled reply text without claiming anything
/// about whether the owning run or item completed.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum AssistantMessagePhase {
    /// No phase was disclosed for this text.
    Unspecified,
    /// Progress commentary rather than the settled reply.
    Commentary,
    /// The settled reply text.
    Final,
}

/// One durably stored assistant-message item.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct AssistantMessageItem {
    /// Forge-minted item identity.
    pub item_id: ItemId,
    /// Turn that owns the item.
    pub turn_id: TurnId,
    /// Forge-minted identity of the run that produced this output.
    ///
    /// Opaque, nonsecret routing evidence of origin only: never a lease,
    /// credential, engine id, or public run-state machine.
    pub run_id: RunId,
    /// Stable position in the containing conversation.
    pub ordinal: ItemOrdinal,
    /// Current entity revision; newly stored items start at zero.
    pub revision: Revision,
    /// Renderer-visible lifecycle.
    pub lifecycle: ConversationLifecycle,
    /// Complete, bounded text stored durably by Forge.
    pub body: AssistantBody,
    /// Renderer-disclosed text phase.
    pub phase: AssistantMessagePhase,
    /// Creation time as signed Unix epoch milliseconds.
    pub created_at: UnixMillis,
    /// Last update time as signed Unix epoch milliseconds.
    pub updated_at: UnixMillis,
}
