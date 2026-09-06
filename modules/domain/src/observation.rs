//! Shared, durable observation vocabulary for native engine runs.
//!
//! This module ports the provider-neutral event shapes of TypeScript
//! `EngineObservation` (`modules/engines/src/engine.ts`) into validated Rust
//! domain types. It covers persistence only: every observation carries a
//! durable sequence for ordering, every string is bounded in UTF-8 bytes, and
//! every provider-disclosed label (phase, state, action, scope) parses through
//! an explicit enum so unknown provider values reject with a typed
//! [`ObservationError`] instead of a stringly fallback.
//!
//! Deliberate boundaries:
//!
//! - There is no wire or transport here: no raw frames, no provenance blobs,
//!   no credentials. Sanitized values only.
//! - There is no delivery here: a completed reasoning summary settles its
//!   phase even when no delta ever arrived, but streaming assembly stays with
//!   the owner.
//! - Absent means unreported, never zero: optional counts such as
//!   `lines_added` stay [`None`] when the engine did not report them and must
//!   never be imputed as zero by a renderer.
//! - Usage context is a gauge, never additive: [`UsageObservation`] preserves
//!   the provider `basis` verbatim, but `context_tokens` always replaces the
//!   previous report regardless of that basis. The codec performs no
//!   arithmetic at all.
//! - Child rows never live in the root: [`TranscriptContent`] projects only
//!   the eight renderer-safe kinds into [`Observation::SubagentTranscript`];
//!   every other root kind rejects projection with
//!   [`ObservationError::NotProjectable`].
//!
//! Bounds mirror the [`EngineRuntimeControls`](crate::EngineRuntimeControls)
//! style: each ceiling is a documented `pub const` measured in UTF-8 bytes,
//! reusing the shared conversation ceilings wherever the meaning is the same.

use thiserror::Error;

use crate::bounds::{
    CONVERSATION_TEXT_FRAGMENT_MAX_BYTES, ENGINE_RUNTIME_MAX_MILLIS, IDENTIFIER_MAX_BYTES,
    MESSAGE_BODY_MAX_BYTES, THREAD_TITLE_MAX_BYTES,
};
use crate::identifiers::IdentifierError;
use crate::run_usage::RunUsageBasis;

// ---------------------------------------------------------------------------
// Bounds
// ---------------------------------------------------------------------------

/// Maximum UTF-8 byte length of any observation identity.
///
/// Every identity carried here (observation, item, turn, tool, activity,
/// search, approval, question, compaction, plan entry, native thread) follows
/// the shared wire identifier rule: non-empty, no whitespace or control
/// characters, bounded.
pub const OBSERVATION_ID_MAX_BYTES: usize = IDENTIFIER_MAX_BYTES;

/// Maximum UTF-8 byte length of one streamed text fragment.
///
/// Mirrors the conversation fragment ceiling: agent message deltas and
/// reasoning summary deltas chunk to this size.
pub const OBSERVATION_DELTA_MAX_BYTES: usize = CONVERSATION_TEXT_FRAGMENT_MAX_BYTES;

/// Maximum UTF-8 byte length of one completed message or reasoning text.
///
/// Mirrors the stored assistant body ceiling.
pub const OBSERVATION_MESSAGE_MAX_BYTES: usize = MESSAGE_BODY_MAX_BYTES;

/// Maximum UTF-8 byte length of a detail, summary, question text, retry
/// message, diagnostic message, or subagent activity string.
pub const OBSERVATION_TEXT_MAX_BYTES: usize = CONVERSATION_TEXT_FRAGMENT_MAX_BYTES;

/// Maximum UTF-8 byte length of a terminal activity output chunk.
pub const OBSERVATION_OUTPUT_MAX_BYTES: usize = 8_192;

/// Maximum UTF-8 byte length of a short reason, error detail, or approval
/// reason string.
pub const OBSERVATION_REASON_MAX_BYTES: usize = 1_024;

/// Maximum UTF-8 byte length of a command string (approval command or
/// terminal activity command).
pub const OBSERVATION_COMMAND_MAX_BYTES: usize = CONVERSATION_TEXT_FRAGMENT_MAX_BYTES;

/// Maximum UTF-8 byte length of a filesystem path description (file
/// observation path, agent path, approval working directory).
pub const OBSERVATION_PATH_MAX_BYTES: usize = CONVERSATION_TEXT_FRAGMENT_MAX_BYTES;

/// Maximum UTF-8 byte length of a search query.
pub const OBSERVATION_QUERY_MAX_BYTES: usize = 1_024;

/// Maximum UTF-8 byte length of a short label (tool name, shell name,
/// question header, question option label, native action name).
pub const OBSERVATION_LABEL_MAX_BYTES: usize = 256;

/// Maximum UTF-8 byte length of an approval description or question option
/// description.
pub const OBSERVATION_DESCRIPTION_MAX_BYTES: usize = 1_024;

/// Maximum UTF-8 byte length of a plan entry text.
pub const OBSERVATION_PLAN_TEXT_MAX_BYTES: usize = 1_024;

/// Maximum number of entries in one plan observation.
pub const OBSERVATION_PLAN_MAX_ENTRIES: usize = 64;

/// Maximum number of options offered by one question observation.
pub const OBSERVATION_QUESTION_MAX_OPTIONS: usize = 16;

/// Maximum number of answers resolving one question observation.
pub const OBSERVATION_ANSWERS_MAX: usize = 16;

/// Maximum UTF-8 byte length of one question answer.
pub const OBSERVATION_ANSWER_MAX_BYTES: usize = 1_024;

/// Maximum UTF-8 byte length of a stable `AE-*` artisan error code.
pub const OBSERVATION_ARTISAN_CODE_MAX_BYTES: usize = 64;

/// Maximum UTF-8 byte length of a provider error code or quota-bucket id.
pub const OBSERVATION_PROVIDER_CODE_MAX_BYTES: usize = 128;

/// Maximum UTF-8 byte length of a provider quota-bucket label or affected
/// model id.
pub const OBSERVATION_LIMIT_LABEL_MAX_BYTES: usize = 128;

/// Maximum UTF-8 byte length of an ISO `resets_at` timestamp carried as
/// provider evidence.
pub const OBSERVATION_TIMESTAMP_MAX_BYTES: usize = 64;

/// Maximum UTF-8 byte length of a harness-generated session title.
pub const OBSERVATION_TITLE_MAX_BYTES: usize = THREAD_TITLE_MAX_BYTES;

/// Largest durable observation sequence representable by SQLite.
pub const OBSERVATION_SEQUENCE_MAX: u64 = i64::MAX as u64;

/// Largest token or line count representable by SQLite.
pub const OBSERVATION_COUNT_MAX: u64 = i64::MAX as u64;

/// Largest compaction duration in milliseconds.
///
/// Mirrors the engine runtime millisecond ceiling.
pub const OBSERVATION_DURATION_MAX_MILLIS: u64 = ENGINE_RUNTIME_MAX_MILLIS;

/// Largest reasoning summary index representable by SQLite.
pub const OBSERVATION_SUMMARY_INDEX_MAX: u64 = i64::MAX as u64;

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

/// Validation failure for one externally supplied observation value.
///
/// Only a stable field label and a finite reason category are retained. A
/// rejected value is never stored or formatted, and unknown provider labels
/// reject as [`ObservationError::UnknownValue`] rather than passing through
/// as strings.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum ObservationError {
    /// An observation identity violated the shared wire identifier rule.
    #[error("observation identifier is invalid")]
    Identifier(#[from] IdentifierError),
    /// A required value was empty.
    #[error("observation field `{field}` must not be empty")]
    Empty {
        /// Stable label of the rejected field.
        field: &'static str,
    },
    /// A value exceeded its documented UTF-8 byte ceiling.
    #[error("observation field `{field}` is {length} UTF-8 bytes; the maximum is {maximum}")]
    TooLong {
        /// Stable label of the rejected field.
        field: &'static str,
        /// Offending length in UTF-8 bytes.
        length: usize,
        /// The documented ceiling in UTF-8 bytes.
        maximum: usize,
    },
    /// A collection exceeded its documented entry ceiling.
    #[error("observation field `{field}` has {count} entries; the maximum is {maximum}")]
    TooMany {
        /// Stable label of the rejected field.
        field: &'static str,
        /// Offending entry count.
        count: usize,
        /// The documented entry ceiling.
        maximum: usize,
    },
    /// A provider-disclosed label used a value this version does not model.
    #[error("observation field `{field}` carries an unknown provider value")]
    UnknownValue {
        /// Stable label of the rejected field.
        field: &'static str,
    },
    /// A numeric value fell outside its finite range.
    #[error("observation field `{field}` is outside its finite range")]
    OutOfRange {
        /// Stable label of the rejected field.
        field: &'static str,
    },
    /// A value required by the observation state was absent.
    #[error("observation field `{field}` is required for this state")]
    MissingField {
        /// Stable label of the missing field.
        field: &'static str,
    },
    /// A value forbidden by the observation state was present.
    #[error("observation field `{field}` must be absent in this state")]
    UnexpectedField {
        /// Stable label of the forbidden field.
        field: &'static str,
    },
    /// A root observation kind cannot be projected into a subagent transcript.
    #[error("observation kind `{kind}` cannot be projected into a subagent transcript")]
    NotProjectable {
        /// Stable tag of the rejected root kind.
        kind: &'static str,
    },
}

// ---------------------------------------------------------------------------
// Shared primitives
// ---------------------------------------------------------------------------

/// Validated identity of one observation or of one entity it references.
///
/// All identities here (observation, item, turn, tool, activity, search,
/// approval, question, compaction, plan entry, native thread) are opaque
/// strings under the shared wire identifier rule.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ObservationId(String);

impl ObservationId {
    /// Creates an identity after validating the external value.
    ///
    /// # Errors
    ///
    /// Returns [`IdentifierError`] when the value is empty, contains
    /// whitespace or control characters, or exceeds
    /// [`OBSERVATION_ID_MAX_BYTES`] UTF-8 bytes.
    pub fn parse(value: impl Into<String>) -> Result<Self, IdentifierError> {
        let value = value.into();
        if value.is_empty() {
            return Err(IdentifierError::Empty);
        }
        if let Some(character) = value
            .chars()
            .find(|character| character.is_whitespace() || character.is_control())
        {
            return Err(IdentifierError::ForbiddenCharacter { character });
        }
        let length = value.len();
        if length > OBSERVATION_ID_MAX_BYTES {
            return Err(IdentifierError::TooLong {
                length,
                maximum: OBSERVATION_ID_MAX_BYTES,
            });
        }
        Ok(Self(value))
    }

    /// Returns the validated identity text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Durable, monotonically committed sequence of one observation.
///
/// The value must fit the SQLite signed integer column. Ordering across a run
/// is enforced by the persistence layer, which requires strictly increasing
/// sequences; this type only enforces the finite range.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ObservationSequence(u64);

impl ObservationSequence {
    /// Creates a sequence after validating its finite range.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError::OutOfRange`] when `value` exceeds
    /// [`OBSERVATION_SEQUENCE_MAX`].
    pub const fn new(value: u64) -> Result<Self, ObservationError> {
        if value > OBSERVATION_SEQUENCE_MAX {
            return Err(ObservationError::OutOfRange { field: "sequence" });
        }
        Ok(Self(value))
    }

    /// Returns the sequence value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// Renderer-disclosed display phase of one agent-authored message.
///
/// Preserved verbatim from the provider disclosure without inferring it from
/// message order. Independent of any item lifecycle.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MessagePhase {
    /// Progress commentary rather than the settled reply.
    Commentary,
    /// The settled reply text.
    Final,
    /// No phase was disclosed for this text.
    Unspecified,
}

impl MessagePhase {
    /// Returns the stable provider spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Commentary => "commentary",
            Self::Final => "final",
            Self::Unspecified => "unspecified",
        }
    }

    /// Parses a provider-disclosed phase spelling.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError::UnknownValue`] for any other spelling.
    /// Unknown phases never default to another phase.
    pub fn parse(value: &str) -> Result<Self, ObservationError> {
        match value {
            "commentary" => Ok(Self::Commentary),
            "final" => Ok(Self::Final),
            "unspecified" => Ok(Self::Unspecified),
            _ => Err(ObservationError::UnknownValue { field: "phase" }),
        }
    }
}

fn check_text(value: &str, field: &'static str, maximum: usize) -> Result<(), ObservationError> {
    let length = value.len();
    if length > maximum {
        return Err(ObservationError::TooLong {
            field,
            length,
            maximum,
        });
    }
    Ok(())
}

fn check_nonempty_text(
    value: &str,
    field: &'static str,
    maximum: usize,
) -> Result<(), ObservationError> {
    if value.is_empty() {
        return Err(ObservationError::Empty { field });
    }
    check_text(value, field, maximum)
}

fn check_optional_text(
    value: Option<&str>,
    field: &'static str,
    maximum: usize,
) -> Result<(), ObservationError> {
    if let Some(text) = value {
        if text.is_empty() {
            return Err(ObservationError::Empty { field });
        }
        check_text(text, field, maximum)?;
    }
    Ok(())
}

fn check_count(value: Option<u64>, field: &'static str) -> Result<(), ObservationError> {
    if value.is_some_and(|count| count > OBSERVATION_COUNT_MAX) {
        return Err(ObservationError::OutOfRange { field });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Agent message delta / completed
// ---------------------------------------------------------------------------

/// One streamed fragment of an agent-authored message.
///
/// Fragments are non-empty and bounded; an empty delta carries no information
/// and is rejected rather than persisted.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct AgentMessageDeltaObservation {
    id: ObservationId,
    sequence: ObservationSequence,
    item_id: ObservationId,
    phase: MessagePhase,
    delta: String,
    turn_id: ObservationId,
}

impl AgentMessageDeltaObservation {
    /// Creates a delta after validating identities, phase, and bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when an identity is invalid, the delta is
    /// empty or exceeds [`OBSERVATION_DELTA_MAX_BYTES`] bytes.
    pub fn new(
        id: ObservationId,
        sequence: ObservationSequence,
        item_id: ObservationId,
        phase: MessagePhase,
        delta: String,
        turn_id: ObservationId,
    ) -> Result<Self, ObservationError> {
        check_nonempty_text(&delta, "delta", OBSERVATION_DELTA_MAX_BYTES)?;
        Ok(Self {
            id,
            sequence,
            item_id,
            phase,
            delta,
            turn_id,
        })
    }

    /// Returns the observation identity.
    #[must_use]
    pub const fn id(&self) -> &ObservationId {
        &self.id
    }

    /// Returns the durable sequence.
    #[must_use]
    pub const fn sequence(&self) -> ObservationSequence {
        self.sequence
    }

    /// Returns the native assistant message item this delta extends.
    #[must_use]
    pub const fn item_id(&self) -> &ObservationId {
        &self.item_id
    }

    /// Returns the provider-disclosed display phase, preserved verbatim.
    #[must_use]
    pub const fn phase(&self) -> MessagePhase {
        self.phase
    }

    /// Returns the exact delta text.
    #[must_use]
    pub fn delta(&self) -> &str {
        &self.delta
    }

    /// Returns the provider turn identity.
    #[must_use]
    pub const fn turn_id(&self) -> &ObservationId {
        &self.turn_id
    }
}

/// One completed agent-authored message.
///
/// Empty text is valid: a provider may settle a message item that never
/// streamed visible content.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct AgentMessageCompletedObservation {
    id: ObservationId,
    sequence: ObservationSequence,
    item_id: ObservationId,
    phase: MessagePhase,
    message: String,
    turn_id: ObservationId,
}

impl AgentMessageCompletedObservation {
    /// Creates a completed message after validating identities and bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when an identity is invalid or the message
    /// exceeds [`OBSERVATION_MESSAGE_MAX_BYTES`] bytes.
    pub fn new(
        id: ObservationId,
        sequence: ObservationSequence,
        item_id: ObservationId,
        phase: MessagePhase,
        message: String,
        turn_id: ObservationId,
    ) -> Result<Self, ObservationError> {
        check_text(&message, "message", OBSERVATION_MESSAGE_MAX_BYTES)?;
        Ok(Self {
            id,
            sequence,
            item_id,
            phase,
            message,
            turn_id,
        })
    }

    /// Returns the observation identity.
    #[must_use]
    pub const fn id(&self) -> &ObservationId {
        &self.id
    }

    /// Returns the durable sequence.
    #[must_use]
    pub const fn sequence(&self) -> ObservationSequence {
        self.sequence
    }

    /// Returns the native assistant message item completed by this observation.
    #[must_use]
    pub const fn item_id(&self) -> &ObservationId {
        &self.item_id
    }

    /// Returns the provider-disclosed display phase, preserved verbatim.
    #[must_use]
    pub const fn phase(&self) -> MessagePhase {
        self.phase
    }

    /// Returns the complete message text.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Returns the provider turn identity.
    #[must_use]
    pub const fn turn_id(&self) -> &ObservationId {
        &self.turn_id
    }
}

// ---------------------------------------------------------------------------
// Reasoning summary delta / completed
// ---------------------------------------------------------------------------

/// One streamed fragment of a provider-authored reasoning summary.
///
/// Carries only the public summary text, never private reasoning content.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ReasoningSummaryDeltaObservation {
    id: ObservationId,
    sequence: ObservationSequence,
    item_id: ObservationId,
    summary_index: u64,
    delta: String,
    thinking_tokens: Option<u64>,
    turn_id: ObservationId,
}

impl ReasoningSummaryDeltaObservation {
    /// Creates a reasoning delta after validating identities and bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when an identity is invalid, the summary
    /// index or thinking-token count leaves its finite range, or the delta is
    /// empty or exceeds [`OBSERVATION_DELTA_MAX_BYTES`] bytes.
    pub fn new(
        id: ObservationId,
        sequence: ObservationSequence,
        item_id: ObservationId,
        summary_index: u64,
        delta: String,
        thinking_tokens: Option<u64>,
        turn_id: ObservationId,
    ) -> Result<Self, ObservationError> {
        if summary_index > OBSERVATION_SUMMARY_INDEX_MAX {
            return Err(ObservationError::OutOfRange {
                field: "summary_index",
            });
        }
        check_count(thinking_tokens, "thinking_tokens")?;
        check_nonempty_text(&delta, "delta", OBSERVATION_DELTA_MAX_BYTES)?;
        Ok(Self {
            id,
            sequence,
            item_id,
            summary_index,
            delta,
            thinking_tokens,
            turn_id,
        })
    }

    /// Returns the observation identity.
    #[must_use]
    pub const fn id(&self) -> &ObservationId {
        &self.id
    }

    /// Returns the durable sequence.
    #[must_use]
    pub const fn sequence(&self) -> ObservationSequence {
        self.sequence
    }

    /// Returns the reasoning item this delta extends.
    #[must_use]
    pub const fn item_id(&self) -> &ObservationId {
        &self.item_id
    }

    /// Returns the provider summary index.
    #[must_use]
    pub const fn summary_index(&self) -> u64 {
        self.summary_index
    }

    /// Returns the exact delta text.
    #[must_use]
    pub fn delta(&self) -> &str {
        &self.delta
    }

    /// Returns the engine's cumulative thinking-token estimate, when disclosed.
    #[must_use]
    pub const fn thinking_tokens(&self) -> Option<u64> {
        self.thinking_tokens
    }

    /// Returns the provider turn identity.
    #[must_use]
    pub const fn turn_id(&self) -> &ObservationId {
        &self.turn_id
    }
}

/// One settled reasoning phase for a turn.
///
/// A provider may complete a phase that streamed no delta at all (for example
/// suppressed thinking display), so consumers must settle reasoning state on
/// this observation rather than on delta arrival. When `text` is present it is
/// the authoritative public summary and may replace streamed deltas.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ReasoningSummaryCompletedObservation {
    id: ObservationId,
    sequence: ObservationSequence,
    item_id: ObservationId,
    text: Option<String>,
    turn_id: ObservationId,
}

impl ReasoningSummaryCompletedObservation {
    /// Creates a settled reasoning phase after validating identities and
    /// bounds. A [`None`] text settles a phase without any delta.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when an identity is invalid, a present
    /// text is empty, or the text exceeds [`OBSERVATION_MESSAGE_MAX_BYTES`]
    /// bytes.
    pub fn new(
        id: ObservationId,
        sequence: ObservationSequence,
        item_id: ObservationId,
        text: Option<String>,
        turn_id: ObservationId,
    ) -> Result<Self, ObservationError> {
        check_optional_text(text.as_deref(), "text", OBSERVATION_MESSAGE_MAX_BYTES)?;
        Ok(Self {
            id,
            sequence,
            item_id,
            text,
            turn_id,
        })
    }

    /// Returns the observation identity.
    #[must_use]
    pub const fn id(&self) -> &ObservationId {
        &self.id
    }

    /// Returns the durable sequence.
    #[must_use]
    pub const fn sequence(&self) -> ObservationSequence {
        self.sequence
    }

    /// Returns the reasoning item settled by this observation.
    #[must_use]
    pub const fn item_id(&self) -> &ObservationId {
        &self.item_id
    }

    /// Returns the authoritative public summary, when the provider supplied one.
    #[must_use]
    pub fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }

    /// Returns the provider turn identity.
    #[must_use]
    pub const fn turn_id(&self) -> &ObservationId {
        &self.turn_id
    }
}

// ---------------------------------------------------------------------------
// Tool / file / search / terminal activity
// ---------------------------------------------------------------------------

/// Lifecycle action of one tool invocation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ToolAction {
    /// The tool started.
    Started,
    /// The tool reported progress.
    Progress,
    /// The tool completed.
    Completed,
    /// The tool failed.
    Failed,
}

impl ToolAction {
    /// Returns the stable provider spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Progress => "progress",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    /// Parses a provider-disclosed tool action.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError::UnknownValue`] for any other spelling.
    pub fn parse(value: &str) -> Result<Self, ObservationError> {
        match value {
            "started" => Ok(Self::Started),
            "progress" => Ok(Self::Progress),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            _ => Err(ObservationError::UnknownValue { field: "action" }),
        }
    }
}

/// One tool lifecycle event without provider-specific tool types.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ToolObservation {
    id: ObservationId,
    sequence: ObservationSequence,
    tool_id: ObservationId,
    tool_name: String,
    action: ToolAction,
    detail: Option<String>,
}

impl ToolObservation {
    /// Creates a tool observation after validating identities and bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when an identity is invalid, the tool name
    /// is empty or exceeds [`OBSERVATION_LABEL_MAX_BYTES`] bytes, or a present
    /// detail is empty or exceeds [`OBSERVATION_TEXT_MAX_BYTES`] bytes.
    pub fn new(
        id: ObservationId,
        sequence: ObservationSequence,
        tool_id: ObservationId,
        tool_name: String,
        action: ToolAction,
        detail: Option<String>,
    ) -> Result<Self, ObservationError> {
        check_nonempty_text(&tool_name, "tool_name", OBSERVATION_LABEL_MAX_BYTES)?;
        check_optional_text(detail.as_deref(), "detail", OBSERVATION_TEXT_MAX_BYTES)?;
        Ok(Self {
            id,
            sequence,
            tool_id,
            tool_name,
            action,
            detail,
        })
    }

    /// Returns the observation identity.
    #[must_use]
    pub const fn id(&self) -> &ObservationId {
        &self.id
    }

    /// Returns the durable sequence.
    #[must_use]
    pub const fn sequence(&self) -> ObservationSequence {
        self.sequence
    }

    /// Returns the provider tool invocation identity.
    #[must_use]
    pub const fn tool_id(&self) -> &ObservationId {
        &self.tool_id
    }

    /// Returns the provider tool name.
    #[must_use]
    pub fn tool_name(&self) -> &str {
        &self.tool_name
    }

    /// Returns the lifecycle action.
    #[must_use]
    pub const fn action(&self) -> ToolAction {
        self.action
    }

    /// Returns the optional provider detail.
    #[must_use]
    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }
}

/// File action of one file observation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FileAction {
    /// The file was created.
    Created,
    /// The file was modified.
    Modified,
    /// The file was deleted.
    Deleted,
    /// The file was read.
    Read,
}

impl FileAction {
    /// Returns the stable provider spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Modified => "modified",
            Self::Deleted => "deleted",
            Self::Read => "read",
        }
    }

    /// Parses a provider-disclosed file action.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError::UnknownValue`] for any other spelling.
    pub fn parse(value: &str) -> Result<Self, ObservationError> {
        match value {
            "created" => Ok(Self::Created),
            "modified" => Ok(Self::Modified),
            "deleted" => Ok(Self::Deleted),
            "read" => Ok(Self::Read),
            _ => Err(ObservationError::UnknownValue { field: "action" }),
        }
    }
}

/// One file mutation or inspection performed during a run.
///
/// Line counts are present only when the engine reported enough to count
/// them. Absent means uncounted, never zero.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct FileObservation {
    id: ObservationId,
    sequence: ObservationSequence,
    path: String,
    action: FileAction,
    lines_added: Option<u64>,
    lines_deleted: Option<u64>,
}

impl FileObservation {
    /// Creates a file observation after validating identities and bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when an identity is invalid, the path is
    /// empty or exceeds [`OBSERVATION_PATH_MAX_BYTES`] bytes, or a line count
    /// leaves its finite range.
    pub fn new(
        id: ObservationId,
        sequence: ObservationSequence,
        path: String,
        action: FileAction,
        lines_added: Option<u64>,
        lines_deleted: Option<u64>,
    ) -> Result<Self, ObservationError> {
        check_nonempty_text(&path, "path", OBSERVATION_PATH_MAX_BYTES)?;
        check_count(lines_added, "lines_added")?;
        check_count(lines_deleted, "lines_deleted")?;
        Ok(Self {
            id,
            sequence,
            path,
            action,
            lines_added,
            lines_deleted,
        })
    }

    /// Returns the observation identity.
    #[must_use]
    pub const fn id(&self) -> &ObservationId {
        &self.id
    }

    /// Returns the durable sequence.
    #[must_use]
    pub const fn sequence(&self) -> ObservationSequence {
        self.sequence
    }

    /// Returns the affected path description.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Returns the file action.
    #[must_use]
    pub const fn action(&self) -> FileAction {
        self.action
    }

    /// Returns counted added lines, or [`None`] when uncounted (never zero).
    #[must_use]
    pub const fn lines_added(&self) -> Option<u64> {
        self.lines_added
    }

    /// Returns counted deleted lines, or [`None`] when uncounted (never zero).
    #[must_use]
    pub const fn lines_deleted(&self) -> Option<u64> {
        self.lines_deleted
    }
}

/// Scope of one search observation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SearchScope {
    /// The search looked at workspace files.
    Workspace,
    /// The search looked at the web.
    Web,
}

impl SearchScope {
    /// Returns the stable provider spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::Web => "web",
        }
    }

    /// Parses a provider-disclosed search scope.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError::UnknownValue`] for any other spelling.
    pub fn parse(value: &str) -> Result<Self, ObservationError> {
        match value {
            "workspace" => Ok(Self::Workspace),
            "web" => Ok(Self::Web),
            _ => Err(ObservationError::UnknownValue { field: "scope" }),
        }
    }
}

/// Lifecycle state of one search observation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SearchState {
    /// The search started.
    Started,
    /// The search completed.
    Completed,
}

impl SearchState {
    /// Returns the stable provider spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Completed => "completed",
        }
    }

    /// Parses a provider-disclosed search state.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError::UnknownValue`] for any other spelling.
    pub fn parse(value: &str) -> Result<Self, ObservationError> {
        match value {
            "started" => Ok(Self::Started),
            "completed" => Ok(Self::Completed),
            _ => Err(ObservationError::UnknownValue { field: "state" }),
        }
    }
}

/// One search operation performed during a run.
///
/// An absent scope stays absent: renderers apply the historical web default,
/// but persistence never imputes it.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SearchObservation {
    id: ObservationId,
    sequence: ObservationSequence,
    query: String,
    scope: Option<SearchScope>,
    search_id: Option<ObservationId>,
    state: SearchState,
    result_count: Option<u64>,
}

impl SearchObservation {
    /// Creates a search observation after validating identities and bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when an identity is invalid, the query is
    /// empty or exceeds [`OBSERVATION_QUERY_MAX_BYTES`] bytes, or the result
    /// count leaves its finite range.
    pub fn new(
        id: ObservationId,
        sequence: ObservationSequence,
        query: String,
        scope: Option<SearchScope>,
        search_id: Option<ObservationId>,
        state: SearchState,
        result_count: Option<u64>,
    ) -> Result<Self, ObservationError> {
        check_nonempty_text(&query, "query", OBSERVATION_QUERY_MAX_BYTES)?;
        check_count(result_count, "result_count")?;
        Ok(Self {
            id,
            sequence,
            query,
            scope,
            search_id,
            state,
            result_count,
        })
    }

    /// Returns the observation identity.
    #[must_use]
    pub const fn id(&self) -> &ObservationId {
        &self.id
    }

    /// Returns the durable sequence.
    #[must_use]
    pub const fn sequence(&self) -> ObservationSequence {
        self.sequence
    }

    /// Returns the search query.
    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Returns where the search looked, when the provider disclosed it.
    #[must_use]
    pub const fn scope(&self) -> Option<SearchScope> {
        self.scope
    }

    /// Returns the provider search identity, when the provider supplied one.
    #[must_use]
    pub const fn search_id(&self) -> Option<&ObservationId> {
        self.search_id.as_ref()
    }

    /// Returns the search lifecycle state.
    #[must_use]
    pub const fn state(&self) -> SearchState {
        self.state
    }

    /// Returns the reported result count, when the provider supplied one.
    #[must_use]
    pub const fn result_count(&self) -> Option<u64> {
        self.result_count
    }
}

/// Output channel of one terminal activity observation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TerminalChannel {
    /// Standard output.
    Stdout,
    /// Standard error.
    Stderr,
}

impl TerminalChannel {
    /// Returns the stable provider spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
        }
    }

    /// Parses a provider-disclosed terminal channel.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError::UnknownValue`] for any other spelling.
    pub fn parse(value: &str) -> Result<Self, ObservationError> {
        match value {
            "stdout" => Ok(Self::Stdout),
            "stderr" => Ok(Self::Stderr),
            _ => Err(ObservationError::UnknownValue { field: "channel" }),
        }
    }
}

/// Lifecycle state of one terminal activity observation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TerminalActivityState {
    /// The command started.
    Started,
    /// The command emitted output.
    Output,
    /// The command completed.
    Completed,
    /// The command failed.
    Failed,
}

impl TerminalActivityState {
    /// Returns the stable provider spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Output => "output",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    /// Parses a provider-disclosed terminal activity state.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError::UnknownValue`] for any other spelling.
    pub fn parse(value: &str) -> Result<Self, ObservationError> {
        match value {
            "started" => Ok(Self::Started),
            "output" => Ok(Self::Output),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            _ => Err(ObservationError::UnknownValue { field: "state" }),
        }
    }
}

/// Validated values used to construct one terminal activity observation.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TerminalActivityInput {
    /// Provider activity identity.
    pub activity_id: ObservationId,
    /// Output channel, when the provider disclosed one.
    pub channel: Option<TerminalChannel>,
    /// Command text that was run, when the provider disclosed it.
    pub command: Option<String>,
    /// Interpreter executable name (`bash`, `pwsh`, `nu`), when disclosed.
    pub shell: Option<String>,
    /// Output chunk, when the provider emitted one.
    pub output: Option<String>,
    /// Process exit code, when the activity reported one.
    pub exit_code: Option<i32>,
    /// Lifecycle state.
    pub state: TerminalActivityState,
}

/// One shell or process activity row, independent from the run outcome.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TerminalActivityObservation {
    id: ObservationId,
    sequence: ObservationSequence,
    activity_id: ObservationId,
    channel: Option<TerminalChannel>,
    command: Option<String>,
    shell: Option<String>,
    output: Option<String>,
    exit_code: Option<i32>,
    state: TerminalActivityState,
}

impl TerminalActivityObservation {
    /// Creates a terminal activity observation after validating identities
    /// and bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when an identity is invalid, a present
    /// command, shell, or output value is empty or exceeds its ceiling.
    pub fn new(
        id: ObservationId,
        sequence: ObservationSequence,
        input: TerminalActivityInput,
    ) -> Result<Self, ObservationError> {
        check_optional_text(
            input.command.as_deref(),
            "command",
            OBSERVATION_COMMAND_MAX_BYTES,
        )?;
        check_optional_text(input.shell.as_deref(), "shell", OBSERVATION_LABEL_MAX_BYTES)?;
        if let Some(output) = input.output.as_deref() {
            check_text(output, "output", OBSERVATION_OUTPUT_MAX_BYTES)?;
        }
        Ok(Self {
            id,
            sequence,
            activity_id: input.activity_id,
            channel: input.channel,
            command: input.command,
            shell: input.shell,
            output: input.output,
            exit_code: input.exit_code,
            state: input.state,
        })
    }

    /// Returns the observation identity.
    #[must_use]
    pub const fn id(&self) -> &ObservationId {
        &self.id
    }

    /// Returns the durable sequence.
    #[must_use]
    pub const fn sequence(&self) -> ObservationSequence {
        self.sequence
    }

    /// Returns the provider activity identity.
    #[must_use]
    pub const fn activity_id(&self) -> &ObservationId {
        &self.activity_id
    }

    /// Returns the output channel, when disclosed.
    #[must_use]
    pub const fn channel(&self) -> Option<TerminalChannel> {
        self.channel
    }

    /// Returns the command text that was run, when disclosed.
    #[must_use]
    pub fn command(&self) -> Option<&str> {
        self.command.as_deref()
    }

    /// Returns the interpreter executable name, when disclosed.
    #[must_use]
    pub fn shell(&self) -> Option<&str> {
        self.shell.as_deref()
    }

    /// Returns the output chunk, when emitted.
    #[must_use]
    pub fn output(&self) -> Option<&str> {
        self.output.as_deref()
    }

    /// Returns the process exit code, when reported.
    #[must_use]
    pub const fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }

    /// Returns the activity lifecycle state.
    #[must_use]
    pub const fn state(&self) -> TerminalActivityState {
        self.state
    }
}

// ---------------------------------------------------------------------------
// Approval / question (per the S0 audit shapes)
// ---------------------------------------------------------------------------

/// Lifecycle state of one approval observation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ApprovalState {
    /// The approval was requested and awaits a decision.
    Requested,
    /// The approval was decided.
    Resolved,
}

impl ApprovalState {
    /// Returns the stable provider spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Resolved => "resolved",
        }
    }

    /// Parses a provider-disclosed approval state.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError::UnknownValue`] for any other spelling.
    pub fn parse(value: &str) -> Result<Self, ObservationError> {
        match value {
            "requested" => Ok(Self::Requested),
            "resolved" => Ok(Self::Resolved),
            _ => Err(ObservationError::UnknownValue { field: "state" }),
        }
    }
}

/// Provider-neutral action bound to one approval request.
///
/// Mirrors TypeScript `EngineApprovalRequest`: a command approval carries the
/// command text under review, while file-change and generic action approvals
/// carry only an optional reason.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ApprovalRequest {
    kind: ApprovalKind,
    command: Option<String>,
    cwd: Option<String>,
    reason: Option<String>,
}

/// Kind of action bound to one approval request.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ApprovalKind {
    /// A shell command awaiting approval.
    Command,
    /// A file change awaiting approval.
    FileChange,
    /// A generic provider action awaiting approval.
    Action,
}

impl ApprovalKind {
    /// Returns the stable provider spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Command => "command",
            Self::FileChange => "file_change",
            Self::Action => "action",
        }
    }

    /// Parses a provider-disclosed approval kind.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError::UnknownValue`] for any other spelling.
    pub fn parse(value: &str) -> Result<Self, ObservationError> {
        match value {
            "command" => Ok(Self::Command),
            "file_change" => Ok(Self::FileChange),
            "action" => Ok(Self::Action),
            _ => Err(ObservationError::UnknownValue { field: "kind" }),
        }
    }
}

impl ApprovalRequest {
    /// Creates a command approval request after validating bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when the command is empty or exceeds
    /// [`OBSERVATION_COMMAND_MAX_BYTES`] bytes, or when a present working
    /// directory or reason value is empty or exceeds its ceiling.
    pub fn command(
        command: String,
        cwd: Option<String>,
        reason: Option<String>,
    ) -> Result<Self, ObservationError> {
        check_nonempty_text(&command, "command", OBSERVATION_COMMAND_MAX_BYTES)?;
        check_optional_text(cwd.as_deref(), "cwd", OBSERVATION_PATH_MAX_BYTES)?;
        check_optional_text(reason.as_deref(), "reason", OBSERVATION_REASON_MAX_BYTES)?;
        Ok(Self {
            kind: ApprovalKind::Command,
            command: Some(command),
            cwd,
            reason,
        })
    }

    /// Creates a file-change approval request after validating bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when a present reason is empty or exceeds
    /// [`OBSERVATION_REASON_MAX_BYTES`] bytes.
    pub fn file_change(reason: Option<String>) -> Result<Self, ObservationError> {
        check_optional_text(reason.as_deref(), "reason", OBSERVATION_REASON_MAX_BYTES)?;
        Ok(Self {
            kind: ApprovalKind::FileChange,
            command: None,
            cwd: None,
            reason,
        })
    }

    /// Creates a generic action approval request after validating bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when a present reason is empty or exceeds
    /// [`OBSERVATION_REASON_MAX_BYTES`] bytes.
    pub fn action(reason: Option<String>) -> Result<Self, ObservationError> {
        check_optional_text(reason.as_deref(), "reason", OBSERVATION_REASON_MAX_BYTES)?;
        Ok(Self {
            kind: ApprovalKind::Action,
            command: None,
            cwd: None,
            reason,
        })
    }

    /// Returns the approval kind.
    #[must_use]
    pub const fn kind(&self) -> ApprovalKind {
        self.kind
    }

    /// Returns the command text under review for command approvals.
    #[must_use]
    pub fn command_text(&self) -> Option<&str> {
        self.command.as_deref()
    }

    /// Returns the working directory for command approvals, when disclosed.
    #[must_use]
    pub fn cwd(&self) -> Option<&str> {
        self.cwd.as_deref()
    }

    /// Returns the provider reason, when disclosed.
    #[must_use]
    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }
}

/// One approval request or its resolution.
///
/// A requested observation never carries a decision; a resolved observation
/// always does. The constructor pair enforces that relationship.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ApprovalObservation {
    id: ObservationId,
    sequence: ObservationSequence,
    approval_id: ObservationId,
    state: ApprovalState,
    description: String,
    request: ApprovalRequest,
    approved: Option<bool>,
}

impl ApprovalObservation {
    /// Creates a requested approval with no decision attached.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when an identity is invalid or the
    /// description is empty or exceeds
    /// [`OBSERVATION_DESCRIPTION_MAX_BYTES`] bytes.
    pub fn requested(
        id: ObservationId,
        sequence: ObservationSequence,
        approval_id: ObservationId,
        description: String,
        request: ApprovalRequest,
    ) -> Result<Self, ObservationError> {
        check_nonempty_text(
            &description,
            "description",
            OBSERVATION_DESCRIPTION_MAX_BYTES,
        )?;
        Ok(Self {
            id,
            sequence,
            approval_id,
            state: ApprovalState::Requested,
            description,
            request,
            approved: None,
        })
    }

    /// Creates a resolved approval carrying its decision.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when an identity is invalid or the
    /// description is empty or exceeds
    /// [`OBSERVATION_DESCRIPTION_MAX_BYTES`] bytes.
    pub fn resolved(
        id: ObservationId,
        sequence: ObservationSequence,
        approval_id: ObservationId,
        description: String,
        request: ApprovalRequest,
        approved: bool,
    ) -> Result<Self, ObservationError> {
        check_nonempty_text(
            &description,
            "description",
            OBSERVATION_DESCRIPTION_MAX_BYTES,
        )?;
        Ok(Self {
            id,
            sequence,
            approval_id,
            state: ApprovalState::Resolved,
            description,
            request,
            approved: Some(approved),
        })
    }

    /// Returns the observation identity.
    #[must_use]
    pub const fn id(&self) -> &ObservationId {
        &self.id
    }

    /// Returns the durable sequence.
    #[must_use]
    pub const fn sequence(&self) -> ObservationSequence {
        self.sequence
    }

    /// Returns the provider approval identity.
    #[must_use]
    pub const fn approval_id(&self) -> &ObservationId {
        &self.approval_id
    }

    /// Returns the approval lifecycle state.
    #[must_use]
    pub const fn state(&self) -> ApprovalState {
        self.state
    }

    /// Returns the human-readable approval description.
    #[must_use]
    pub fn description(&self) -> &str {
        &self.description
    }

    /// Returns the provider-neutral action under review.
    #[must_use]
    pub const fn request(&self) -> &ApprovalRequest {
        &self.request
    }

    /// Returns the decision for resolved approvals, [`None`] while requested.
    #[must_use]
    pub const fn approved(&self) -> Option<bool> {
        self.approved
    }
}

/// Lifecycle state of one question observation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum QuestionState {
    /// The question was asked and awaits answers.
    Requested,
    /// The question was answered.
    Resolved,
}

impl QuestionState {
    /// Returns the stable provider spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Requested => "requested",
            Self::Resolved => "resolved",
        }
    }

    /// Parses a provider-disclosed question state.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError::UnknownValue`] for any other spelling.
    pub fn parse(value: &str) -> Result<Self, ObservationError> {
        match value {
            "requested" => Ok(Self::Requested),
            "resolved" => Ok(Self::Resolved),
            _ => Err(ObservationError::UnknownValue { field: "state" }),
        }
    }
}

/// One provider-offered answer to a question.
///
/// Mirrors TypeScript `EngineQuestionOption`: the label names the choice and
/// the optional description explains what choosing it means.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct QuestionOption {
    label: String,
    description: Option<String>,
}

impl QuestionOption {
    /// Creates a question option after validating bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when the label is empty or exceeds
    /// [`OBSERVATION_LABEL_MAX_BYTES`] bytes, or when a present description
    /// is empty or exceeds [`OBSERVATION_DESCRIPTION_MAX_BYTES`] bytes.
    pub fn new(label: String, description: Option<String>) -> Result<Self, ObservationError> {
        check_nonempty_text(&label, "label", OBSERVATION_LABEL_MAX_BYTES)?;
        check_optional_text(
            description.as_deref(),
            "description",
            OBSERVATION_DESCRIPTION_MAX_BYTES,
        )?;
        Ok(Self { label, description })
    }

    /// Returns the option label.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Returns what choosing this option means, when the provider explained it.
    #[must_use]
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }
}

/// Validated values shared by a question request and its resolution.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct QuestionInput {
    /// Provider question identity.
    pub question_id: ObservationId,
    /// The question itself.
    pub text: String,
    /// Short category label shown beside the question, when disclosed.
    pub header: Option<String>,
    /// Whether more than one option may be chosen at once.
    pub multi_select: bool,
    /// Offered answers; [`None`] marks a free-form question answered with
    /// typed text rather than a selection.
    pub options: Option<Vec<QuestionOption>>,
}

/// One question request or its resolution.
///
/// A requested observation never carries answers; a resolved observation
/// always carries the answer list (possibly empty for an explicitly skipped
/// question).
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct QuestionObservation {
    id: ObservationId,
    sequence: ObservationSequence,
    question_id: ObservationId,
    state: QuestionState,
    text: String,
    header: Option<String>,
    multi_select: bool,
    options: Option<Vec<QuestionOption>>,
    answers: Option<Vec<String>>,
}

impl QuestionObservation {
    fn validate_input(input: &QuestionInput) -> Result<(), ObservationError> {
        check_nonempty_text(&input.text, "text", OBSERVATION_TEXT_MAX_BYTES)?;
        check_optional_text(
            input.header.as_deref(),
            "header",
            OBSERVATION_LABEL_MAX_BYTES,
        )?;
        if let Some(options) = input.options.as_deref() {
            if options.is_empty() {
                return Err(ObservationError::Empty { field: "options" });
            }
            if options.len() > OBSERVATION_QUESTION_MAX_OPTIONS {
                return Err(ObservationError::TooMany {
                    field: "options",
                    count: options.len(),
                    maximum: OBSERVATION_QUESTION_MAX_OPTIONS,
                });
            }
        }
        Ok(())
    }

    fn validate_answers(answers: &[String]) -> Result<(), ObservationError> {
        if answers.len() > OBSERVATION_ANSWERS_MAX {
            return Err(ObservationError::TooMany {
                field: "answers",
                count: answers.len(),
                maximum: OBSERVATION_ANSWERS_MAX,
            });
        }
        for answer in answers {
            check_nonempty_text(answer, "answers", OBSERVATION_ANSWER_MAX_BYTES)?;
        }
        Ok(())
    }

    /// Creates a requested question with no answers attached.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when an identity is invalid, the text is
    /// empty or exceeds [`OBSERVATION_TEXT_MAX_BYTES`] bytes, the header or an
    /// option violates its ceiling, or the option list is empty or exceeds
    /// [`OBSERVATION_QUESTION_MAX_OPTIONS`] entries.
    pub fn requested(
        id: ObservationId,
        sequence: ObservationSequence,
        input: QuestionInput,
    ) -> Result<Self, ObservationError> {
        Self::validate_input(&input)?;
        Ok(Self {
            id,
            sequence,
            question_id: input.question_id,
            state: QuestionState::Requested,
            text: input.text,
            header: input.header,
            multi_select: input.multi_select,
            options: input.options,
            answers: None,
        })
    }

    /// Creates a resolved question carrying its answers.
    ///
    /// # Errors
    ///
    /// Returns the same [`ObservationError`] cases as
    /// [`QuestionObservation::requested`], plus answer bound violations when
    /// an answer is empty or exceeds [`OBSERVATION_ANSWER_MAX_BYTES`] bytes or
    /// the list exceeds [`OBSERVATION_ANSWERS_MAX`] entries.
    pub fn resolved(
        id: ObservationId,
        sequence: ObservationSequence,
        input: QuestionInput,
        answers: Vec<String>,
    ) -> Result<Self, ObservationError> {
        Self::validate_input(&input)?;
        Self::validate_answers(&answers)?;
        Ok(Self {
            id,
            sequence,
            question_id: input.question_id,
            state: QuestionState::Resolved,
            text: input.text,
            header: input.header,
            multi_select: input.multi_select,
            options: input.options,
            answers: Some(answers),
        })
    }

    /// Returns the observation identity.
    #[must_use]
    pub const fn id(&self) -> &ObservationId {
        &self.id
    }

    /// Returns the durable sequence.
    #[must_use]
    pub const fn sequence(&self) -> ObservationSequence {
        self.sequence
    }

    /// Returns the provider question identity.
    #[must_use]
    pub const fn question_id(&self) -> &ObservationId {
        &self.question_id
    }

    /// Returns the question lifecycle state.
    #[must_use]
    pub const fn state(&self) -> QuestionState {
        self.state
    }

    /// Returns the question itself.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns the short category label, when disclosed.
    #[must_use]
    pub fn header(&self) -> Option<&str> {
        self.header.as_deref()
    }

    /// Returns whether more than one option may be chosen at once.
    #[must_use]
    pub const fn multi_select(&self) -> bool {
        self.multi_select
    }

    /// Returns the offered answers, or [`None`] for a free-form question.
    #[must_use]
    pub const fn options(&self) -> Option<&Vec<QuestionOption>> {
        self.options.as_ref()
    }

    /// Returns the answers for resolved questions, [`None`] while requested.
    #[must_use]
    pub const fn answers(&self) -> Option<&Vec<String>> {
        self.answers.as_ref()
    }
}

// ---------------------------------------------------------------------------
// Plan / compaction / retry / run-state / turn-state
// ---------------------------------------------------------------------------

/// Status of one plan entry.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PlanEntryStatus {
    /// The step has not started.
    Pending,
    /// The step is in progress.
    InProgress,
    /// The step completed.
    Completed,
}

impl PlanEntryStatus {
    /// Returns the stable provider spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
        }
    }

    /// Parses a provider-disclosed plan entry status.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError::UnknownValue`] for any other spelling.
    pub fn parse(value: &str) -> Result<Self, ObservationError> {
        match value {
            "pending" => Ok(Self::Pending),
            "in_progress" => Ok(Self::InProgress),
            "completed" => Ok(Self::Completed),
            _ => Err(ObservationError::UnknownValue { field: "status" }),
        }
    }
}

/// One provider-neutral plan entry.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct PlanEntry {
    id: ObservationId,
    status: PlanEntryStatus,
    text: String,
}

impl PlanEntry {
    /// Creates a plan entry after validating bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when the identity is invalid or the text
    /// is empty or exceeds [`OBSERVATION_PLAN_TEXT_MAX_BYTES`] bytes.
    pub fn new(
        id: ObservationId,
        status: PlanEntryStatus,
        text: String,
    ) -> Result<Self, ObservationError> {
        check_nonempty_text(&text, "text", OBSERVATION_PLAN_TEXT_MAX_BYTES)?;
        Ok(Self { id, status, text })
    }

    /// Returns the entry identity.
    #[must_use]
    pub const fn id(&self) -> &ObservationId {
        &self.id
    }

    /// Returns the entry status.
    #[must_use]
    pub const fn status(&self) -> PlanEntryStatus {
        self.status
    }

    /// Returns the entry text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }
}

/// One provider-neutral plan update.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct PlanObservation {
    id: ObservationId,
    sequence: ObservationSequence,
    entries: Vec<PlanEntry>,
    turn_id: Option<ObservationId>,
}

impl PlanObservation {
    /// Creates a plan observation after validating entry bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when an identity is invalid or the entry
    /// list is empty or exceeds [`OBSERVATION_PLAN_MAX_ENTRIES`] entries.
    pub fn new(
        id: ObservationId,
        sequence: ObservationSequence,
        entries: Vec<PlanEntry>,
        turn_id: Option<ObservationId>,
    ) -> Result<Self, ObservationError> {
        if entries.is_empty() {
            return Err(ObservationError::Empty { field: "entries" });
        }
        if entries.len() > OBSERVATION_PLAN_MAX_ENTRIES {
            return Err(ObservationError::TooMany {
                field: "entries",
                count: entries.len(),
                maximum: OBSERVATION_PLAN_MAX_ENTRIES,
            });
        }
        Ok(Self {
            id,
            sequence,
            entries,
            turn_id,
        })
    }

    /// Returns the observation identity.
    #[must_use]
    pub const fn id(&self) -> &ObservationId {
        &self.id
    }

    /// Returns the durable sequence.
    #[must_use]
    pub const fn sequence(&self) -> ObservationSequence {
        self.sequence
    }

    /// Returns the plan entries.
    #[must_use]
    pub const fn entries(&self) -> &Vec<PlanEntry> {
        &self.entries
    }

    /// Returns the provider turn identity, when the provider attributed one.
    #[must_use]
    pub const fn turn_id(&self) -> Option<&ObservationId> {
        self.turn_id.as_ref()
    }
}

/// Lifecycle state of one compaction observation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CompactionState {
    /// Context compaction started.
    Started,
    /// Context compaction completed.
    Completed,
}

impl CompactionState {
    /// Returns the stable provider spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Completed => "completed",
        }
    }

    /// Parses a provider-disclosed compaction state.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError::UnknownValue`] for any other spelling.
    pub fn parse(value: &str) -> Result<Self, ObservationError> {
        match value {
            "started" => Ok(Self::Started),
            "completed" => Ok(Self::Completed),
            _ => Err(ObservationError::UnknownValue { field: "state" }),
        }
    }
}

/// One provider context compaction report.
///
/// The duration is the sole evidence of a stall the transcript cannot
/// otherwise account for on engines that only announce the completed
/// boundary.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct CompactionObservation {
    id: ObservationId,
    sequence: ObservationSequence,
    state: CompactionState,
    compaction_id: Option<ObservationId>,
    duration_ms: Option<u64>,
    summary: Option<String>,
}

impl CompactionObservation {
    /// Creates a compaction observation after validating identities and bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when an identity is invalid, the duration
    /// exceeds [`OBSERVATION_DURATION_MAX_MILLIS`] milliseconds, or a present
    /// summary is empty or exceeds [`OBSERVATION_TEXT_MAX_BYTES`] bytes.
    pub fn new(
        id: ObservationId,
        sequence: ObservationSequence,
        state: CompactionState,
        compaction_id: Option<ObservationId>,
        duration_ms: Option<u64>,
        summary: Option<String>,
    ) -> Result<Self, ObservationError> {
        if duration_ms.is_some_and(|duration| duration > OBSERVATION_DURATION_MAX_MILLIS) {
            return Err(ObservationError::OutOfRange {
                field: "duration_ms",
            });
        }
        check_optional_text(summary.as_deref(), "summary", OBSERVATION_TEXT_MAX_BYTES)?;
        Ok(Self {
            id,
            sequence,
            state,
            compaction_id,
            duration_ms,
            summary,
        })
    }

    /// Returns the observation identity.
    #[must_use]
    pub const fn id(&self) -> &ObservationId {
        &self.id
    }

    /// Returns the durable sequence.
    #[must_use]
    pub const fn sequence(&self) -> ObservationSequence {
        self.sequence
    }

    /// Returns the compaction lifecycle state.
    #[must_use]
    pub const fn state(&self) -> CompactionState {
        self.state
    }

    /// Returns the provider compaction identity for engines that expose one.
    #[must_use]
    pub const fn compaction_id(&self) -> Option<&ObservationId> {
        self.compaction_id.as_ref()
    }

    /// Returns how long the compaction ran, when the engine measured it.
    #[must_use]
    pub const fn duration_ms(&self) -> Option<u64> {
        self.duration_ms
    }

    /// Returns the provider compaction summary, when supplied.
    #[must_use]
    pub fn summary(&self) -> Option<&str> {
        self.summary.as_deref()
    }
}

/// Provider attempt state of one retry observation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RetryAttemptState {
    /// The provider will make another attempt.
    Retrying,
    /// The attempt ended terminally.
    Terminal,
}

impl RetryAttemptState {
    /// Returns the stable provider spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Retrying => "retrying",
            Self::Terminal => "terminal",
        }
    }

    /// Parses a provider-disclosed retry attempt state.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError::UnknownValue`] for any other spelling.
    pub fn parse(value: &str) -> Result<Self, ObservationError> {
        match value {
            "retrying" => Ok(Self::Retrying),
            "terminal" => Ok(Self::Terminal),
            _ => Err(ObservationError::UnknownValue {
                field: "attempt_state",
            }),
        }
    }
}

/// One provider error report with its continuation intent.
///
/// Models only the lifecycle the provider disclosed; no attempt number is
/// synthesized when native data does not include one.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct RetryObservation {
    id: ObservationId,
    sequence: ObservationSequence,
    turn_id: ObservationId,
    attempt_state: RetryAttemptState,
    will_retry: bool,
    message: String,
}

impl RetryObservation {
    /// Creates a retry observation after validating identities and bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when an identity is invalid or the message
    /// is empty or exceeds [`OBSERVATION_TEXT_MAX_BYTES`] bytes.
    pub fn new(
        id: ObservationId,
        sequence: ObservationSequence,
        turn_id: ObservationId,
        attempt_state: RetryAttemptState,
        will_retry: bool,
        message: String,
    ) -> Result<Self, ObservationError> {
        check_nonempty_text(&message, "message", OBSERVATION_TEXT_MAX_BYTES)?;
        Ok(Self {
            id,
            sequence,
            turn_id,
            attempt_state,
            will_retry,
            message,
        })
    }

    /// Returns the observation identity.
    #[must_use]
    pub const fn id(&self) -> &ObservationId {
        &self.id
    }

    /// Returns the durable sequence.
    #[must_use]
    pub const fn sequence(&self) -> ObservationSequence {
        self.sequence
    }

    /// Returns the provider turn identity.
    #[must_use]
    pub const fn turn_id(&self) -> &ObservationId {
        &self.turn_id
    }

    /// Returns the provider attempt state.
    #[must_use]
    pub const fn attempt_state(&self) -> RetryAttemptState {
        self.attempt_state
    }

    /// Returns whether the provider will continue the current attempt.
    #[must_use]
    pub const fn will_retry(&self) -> bool {
        self.will_retry
    }

    /// Returns the provider message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// Non-terminal lifecycle state of one run.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RunState {
    /// The run is opening.
    Opening,
    /// The run is running.
    Running,
    /// The run is waiting (for example on an approval or question).
    Waiting,
}

impl RunState {
    /// Returns the stable provider spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Opening => "opening",
            Self::Running => "running",
            Self::Waiting => "waiting",
        }
    }

    /// Parses a provider-disclosed run state.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError::UnknownValue`] for any other spelling.
    pub fn parse(value: &str) -> Result<Self, ObservationError> {
        match value {
            "opening" => Ok(Self::Opening),
            "running" => Ok(Self::Running),
            "waiting" => Ok(Self::Waiting),
            _ => Err(ObservationError::UnknownValue { field: "state" }),
        }
    }
}

/// One non-terminal lifecycle change for the run.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct RunStateObservation {
    id: ObservationId,
    sequence: ObservationSequence,
    state: RunState,
}

impl RunStateObservation {
    /// Creates a run state observation.
    ///
    /// Infallible: identities arrive pre-validated.
    #[must_use]
    pub fn new(id: ObservationId, sequence: ObservationSequence, state: RunState) -> Self {
        Self {
            id,
            sequence,
            state,
        }
    }

    /// Returns the observation identity.
    #[must_use]
    pub const fn id(&self) -> &ObservationId {
        &self.id
    }

    /// Returns the durable sequence.
    #[must_use]
    pub const fn sequence(&self) -> ObservationSequence {
        self.sequence
    }

    /// Returns the run lifecycle state.
    #[must_use]
    pub const fn state(&self) -> RunState {
        self.state
    }
}

/// Lifecycle state of one provider turn.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TurnState {
    /// The turn started.
    Started,
    /// The turn is waiting.
    Waiting,
    /// The turn completed.
    Completed,
    /// The turn was cancelled.
    Cancelled,
    /// The turn failed.
    Failed,
}

impl TurnState {
    /// Returns the stable provider spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Waiting => "waiting",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }

    /// Parses a provider-disclosed turn state.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError::UnknownValue`] for any other spelling.
    pub fn parse(value: &str) -> Result<Self, ObservationError> {
        match value {
            "started" => Ok(Self::Started),
            "waiting" => Ok(Self::Waiting),
            "completed" => Ok(Self::Completed),
            "cancelled" => Ok(Self::Cancelled),
            "failed" => Ok(Self::Failed),
            _ => Err(ObservationError::UnknownValue { field: "state" }),
        }
    }
}

/// One lifecycle progress report for a single provider turn.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TurnStateObservation {
    id: ObservationId,
    sequence: ObservationSequence,
    turn_id: ObservationId,
    state: TurnState,
}

impl TurnStateObservation {
    /// Creates a turn state observation.
    ///
    /// Infallible: identities arrive pre-validated.
    #[must_use]
    pub fn new(
        id: ObservationId,
        sequence: ObservationSequence,
        turn_id: ObservationId,
        state: TurnState,
    ) -> Self {
        Self {
            id,
            sequence,
            turn_id,
            state,
        }
    }

    /// Returns the observation identity.
    #[must_use]
    pub const fn id(&self) -> &ObservationId {
        &self.id
    }

    /// Returns the durable sequence.
    #[must_use]
    pub const fn sequence(&self) -> ObservationSequence {
        self.sequence
    }

    /// Returns the provider turn identity.
    #[must_use]
    pub const fn turn_id(&self) -> &ObservationId {
        &self.turn_id
    }

    /// Returns the turn lifecycle state.
    #[must_use]
    pub const fn state(&self) -> TurnState {
        self.state
    }
}

// ---------------------------------------------------------------------------
// Subagent lifecycle + transcript projection
// ---------------------------------------------------------------------------

/// Lifecycle state of one provider-native subagent.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SubagentState {
    /// The subagent was discovered.
    Discovered,
    /// The subagent is running.
    Running,
    /// The subagent is waiting.
    Waiting,
    /// The subagent completed.
    Completed,
    /// The subagent failed.
    Failed,
    /// The subagent was interrupted.
    Interrupted,
}

impl SubagentState {
    /// Returns the stable provider spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Discovered => "discovered",
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
        }
    }

    /// Parses a provider-disclosed subagent state.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError::UnknownValue`] for any other spelling.
    pub fn parse(value: &str) -> Result<Self, ObservationError> {
        match value {
            "discovered" => Ok(Self::Discovered),
            "running" => Ok(Self::Running),
            "waiting" => Ok(Self::Waiting),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "interrupted" => Ok(Self::Interrupted),
            _ => Err(ObservationError::UnknownValue { field: "state" }),
        }
    }
}

/// Validated values used to construct one subagent lifecycle observation.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SubagentInput {
    /// Provider identity of the child agent thread.
    pub agent_native_thread_id: ObservationId,
    /// Provider identity of the parent thread that owns the run.
    pub parent_native_thread_id: ObservationId,
    /// Lifecycle state.
    pub state: SubagentState,
    /// Provider activity description, when disclosed.
    pub activity: Option<String>,
    /// Provider agent path, when disclosed.
    pub agent_path: Option<String>,
    /// Provider turn identity, when the provider attributed one.
    pub turn_id: Option<ObservationId>,
}

/// One provider-native subagent activity report.
///
/// The subagent never becomes the owner run: its public content travels
/// separately through [`Observation::SubagentTranscript`].
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SubagentObservation {
    id: ObservationId,
    sequence: ObservationSequence,
    agent_native_thread_id: ObservationId,
    parent_native_thread_id: ObservationId,
    state: SubagentState,
    activity: Option<String>,
    agent_path: Option<String>,
    turn_id: Option<ObservationId>,
}

impl SubagentObservation {
    /// Creates a subagent observation after validating identities and bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when an identity is invalid or a present
    /// activity or agent path value is empty or exceeds its ceiling.
    pub fn new(
        id: ObservationId,
        sequence: ObservationSequence,
        input: SubagentInput,
    ) -> Result<Self, ObservationError> {
        check_optional_text(
            input.activity.as_deref(),
            "activity",
            OBSERVATION_TEXT_MAX_BYTES,
        )?;
        check_optional_text(
            input.agent_path.as_deref(),
            "agent_path",
            OBSERVATION_PATH_MAX_BYTES,
        )?;
        Ok(Self {
            id,
            sequence,
            agent_native_thread_id: input.agent_native_thread_id,
            parent_native_thread_id: input.parent_native_thread_id,
            state: input.state,
            activity: input.activity,
            agent_path: input.agent_path,
            turn_id: input.turn_id,
        })
    }

    /// Returns the observation identity.
    #[must_use]
    pub const fn id(&self) -> &ObservationId {
        &self.id
    }

    /// Returns the durable sequence.
    #[must_use]
    pub const fn sequence(&self) -> ObservationSequence {
        self.sequence
    }

    /// Returns the provider identity of the child agent thread.
    #[must_use]
    pub const fn agent_native_thread_id(&self) -> &ObservationId {
        &self.agent_native_thread_id
    }

    /// Returns the provider identity of the parent thread.
    #[must_use]
    pub const fn parent_native_thread_id(&self) -> &ObservationId {
        &self.parent_native_thread_id
    }

    /// Returns the subagent lifecycle state.
    #[must_use]
    pub const fn state(&self) -> SubagentState {
        self.state
    }

    /// Returns the provider activity description, when disclosed.
    #[must_use]
    pub fn activity(&self) -> Option<&str> {
        self.activity.as_deref()
    }

    /// Returns the provider agent path, when disclosed.
    #[must_use]
    pub fn agent_path(&self) -> Option<&str> {
        self.agent_path.as_deref()
    }

    /// Returns the provider turn identity, when attributed.
    #[must_use]
    pub const fn turn_id(&self) -> Option<&ObservationId> {
        self.turn_id.as_ref()
    }
}

/// Renderer-safe content of one native subagent row.
///
/// Only the eight projectable kinds exist here: agent message
/// delta/completed, reasoning summary delta/completed, terminal activity,
/// tool, file, and search. Approvals, questions, usage, diagnostics, plan,
/// compaction, retry, run/turn state, terminal outcomes, native actions, and
/// nested subagent rows never project into a child transcript.
#[derive(Clone, Debug, PartialEq)]
pub enum TranscriptContent {
    /// Child agent message fragment.
    AgentMessageDelta(TranscriptAgentMessageDelta),
    /// Child completed agent message.
    AgentMessageCompleted(TranscriptAgentMessageCompleted),
    /// Child reasoning summary fragment.
    ReasoningSummaryDelta(TranscriptReasoningSummaryDelta),
    /// Child settled reasoning phase.
    ReasoningSummaryCompleted(TranscriptReasoningSummaryCompleted),
    /// Child terminal activity row.
    TerminalActivity(TranscriptTerminalActivity),
    /// Child tool row.
    Tool(TranscriptTool),
    /// Child file row.
    File(TranscriptFile),
    /// Child search row.
    Search(TranscriptSearch),
}

impl TranscriptContent {
    /// Returns the stable content tag.
    #[must_use]
    pub const fn tag(&self) -> &'static str {
        match self {
            Self::AgentMessageDelta(_) => "agent_message_delta",
            Self::AgentMessageCompleted(_) => "agent_message_completed",
            Self::ReasoningSummaryDelta(_) => "reasoning_summary_delta",
            Self::ReasoningSummaryCompleted(_) => "reasoning_summary_completed",
            Self::TerminalActivity(_) => "terminal_activity",
            Self::Tool(_) => "tool",
            Self::File(_) => "file",
            Self::Search(_) => "search",
        }
    }

    /// Projects renderer-safe content out of one root observation.
    ///
    /// Root header (observation identity, durable sequence, turn attribution)
    /// is dropped: a child row receives its own durable identity at commit
    /// time and must never reuse the root row.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError::NotProjectable`] for every root kind that
    /// has no transcript projection.
    pub fn project(observation: &Observation) -> Result<Self, ObservationError> {
        match observation {
            Observation::AgentMessageDelta(value) => TranscriptAgentMessageDelta::new(
                value.item_id.clone(),
                value.phase,
                value.delta.clone(),
            )
            .map(Self::AgentMessageDelta),
            Observation::AgentMessageCompleted(value) => TranscriptAgentMessageCompleted::new(
                value.item_id.clone(),
                value.phase,
                value.message.clone(),
            )
            .map(Self::AgentMessageCompleted),
            Observation::ReasoningSummaryDelta(value) => TranscriptReasoningSummaryDelta::new(
                value.item_id.clone(),
                value.summary_index,
                value.delta.clone(),
            )
            .map(Self::ReasoningSummaryDelta),
            Observation::ReasoningSummaryCompleted(value) => {
                TranscriptReasoningSummaryCompleted::new(value.item_id.clone(), value.text.clone())
                    .map(Self::ReasoningSummaryCompleted)
            }
            Observation::TerminalActivity(value) => TranscriptTerminalActivity::new(
                value.activity_id.clone(),
                value.channel,
                value.command.clone(),
                value.exit_code,
                value.output.clone(),
                value.state,
            )
            .map(Self::TerminalActivity),
            Observation::Tool(value) => TranscriptTool::new(
                value.tool_id.clone(),
                value.tool_name.clone(),
                value.action,
                value.detail.clone(),
            )
            .map(Self::Tool),
            Observation::File(value) => TranscriptFile::new(
                value.path.clone(),
                value.action,
                value.lines_added,
                value.lines_deleted,
            )
            .map(Self::File),
            Observation::Search(value) => TranscriptSearch::new(
                value.query.clone(),
                value.result_count,
                value.scope,
                value.search_id.clone(),
                value.state,
            )
            .map(Self::Search),
            other => Err(ObservationError::NotProjectable { kind: other.tag() }),
        }
    }
}

/// Child agent message fragment.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TranscriptAgentMessageDelta {
    item_id: ObservationId,
    phase: MessagePhase,
    delta: String,
}

impl TranscriptAgentMessageDelta {
    /// Creates child message content after validating bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when the delta is empty or exceeds
    /// [`OBSERVATION_DELTA_MAX_BYTES`] bytes.
    pub fn new(
        item_id: ObservationId,
        phase: MessagePhase,
        delta: String,
    ) -> Result<Self, ObservationError> {
        check_nonempty_text(&delta, "delta", OBSERVATION_DELTA_MAX_BYTES)?;
        Ok(Self {
            item_id,
            phase,
            delta,
        })
    }

    /// Returns the child assistant message item identity.
    #[must_use]
    pub const fn item_id(&self) -> &ObservationId {
        &self.item_id
    }

    /// Returns the provider-disclosed display phase, preserved verbatim.
    #[must_use]
    pub const fn phase(&self) -> MessagePhase {
        self.phase
    }

    /// Returns the exact delta text.
    #[must_use]
    pub fn delta(&self) -> &str {
        &self.delta
    }
}

/// Child completed agent message.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TranscriptAgentMessageCompleted {
    item_id: ObservationId,
    phase: MessagePhase,
    message: String,
}

impl TranscriptAgentMessageCompleted {
    /// Creates child completed-message content after validating bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when the message exceeds
    /// [`OBSERVATION_MESSAGE_MAX_BYTES`] bytes.
    pub fn new(
        item_id: ObservationId,
        phase: MessagePhase,
        message: String,
    ) -> Result<Self, ObservationError> {
        check_text(&message, "message", OBSERVATION_MESSAGE_MAX_BYTES)?;
        Ok(Self {
            item_id,
            phase,
            message,
        })
    }

    /// Returns the child assistant message item identity.
    #[must_use]
    pub const fn item_id(&self) -> &ObservationId {
        &self.item_id
    }

    /// Returns the provider-disclosed display phase, preserved verbatim.
    #[must_use]
    pub const fn phase(&self) -> MessagePhase {
        self.phase
    }

    /// Returns the complete message text.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// Child reasoning summary fragment.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TranscriptReasoningSummaryDelta {
    item_id: ObservationId,
    summary_index: u64,
    delta: String,
}

impl TranscriptReasoningSummaryDelta {
    /// Creates child reasoning content after validating bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when the summary index leaves its finite
    /// range or the delta is empty or exceeds [`OBSERVATION_DELTA_MAX_BYTES`]
    /// bytes.
    pub fn new(
        item_id: ObservationId,
        summary_index: u64,
        delta: String,
    ) -> Result<Self, ObservationError> {
        if summary_index > OBSERVATION_SUMMARY_INDEX_MAX {
            return Err(ObservationError::OutOfRange {
                field: "summary_index",
            });
        }
        check_nonempty_text(&delta, "delta", OBSERVATION_DELTA_MAX_BYTES)?;
        Ok(Self {
            item_id,
            summary_index,
            delta,
        })
    }

    /// Returns the child reasoning item identity.
    #[must_use]
    pub const fn item_id(&self) -> &ObservationId {
        &self.item_id
    }

    /// Returns the provider summary index.
    #[must_use]
    pub const fn summary_index(&self) -> u64 {
        self.summary_index
    }

    /// Returns the exact delta text.
    #[must_use]
    pub fn delta(&self) -> &str {
        &self.delta
    }
}

/// Child settled reasoning phase.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TranscriptReasoningSummaryCompleted {
    item_id: ObservationId,
    text: Option<String>,
}

impl TranscriptReasoningSummaryCompleted {
    /// Creates child settled-reasoning content after validating bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when a present text is empty or exceeds
    /// [`OBSERVATION_MESSAGE_MAX_BYTES`] bytes.
    pub fn new(item_id: ObservationId, text: Option<String>) -> Result<Self, ObservationError> {
        check_optional_text(text.as_deref(), "text", OBSERVATION_MESSAGE_MAX_BYTES)?;
        Ok(Self { item_id, text })
    }

    /// Returns the child reasoning item identity.
    #[must_use]
    pub const fn item_id(&self) -> &ObservationId {
        &self.item_id
    }

    /// Returns the authoritative public summary, when supplied.
    #[must_use]
    pub fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }
}

/// Child terminal activity row.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TranscriptTerminalActivity {
    activity_id: ObservationId,
    channel: Option<TerminalChannel>,
    command: Option<String>,
    exit_code: Option<i32>,
    output: Option<String>,
    state: TerminalActivityState,
}

impl TranscriptTerminalActivity {
    /// Creates child terminal content after validating bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when a present command or output value is
    /// empty or exceeds its ceiling.
    pub fn new(
        activity_id: ObservationId,
        channel: Option<TerminalChannel>,
        command: Option<String>,
        exit_code: Option<i32>,
        output: Option<String>,
        state: TerminalActivityState,
    ) -> Result<Self, ObservationError> {
        check_optional_text(command.as_deref(), "command", OBSERVATION_COMMAND_MAX_BYTES)?;
        if let Some(output) = output.as_deref() {
            check_text(output, "output", OBSERVATION_OUTPUT_MAX_BYTES)?;
        }
        Ok(Self {
            activity_id,
            channel,
            command,
            exit_code,
            output,
            state,
        })
    }

    /// Returns the child activity identity.
    #[must_use]
    pub const fn activity_id(&self) -> &ObservationId {
        &self.activity_id
    }

    /// Returns the output channel, when disclosed.
    #[must_use]
    pub const fn channel(&self) -> Option<TerminalChannel> {
        self.channel
    }

    /// Returns the command text that was run, when disclosed.
    #[must_use]
    pub fn command(&self) -> Option<&str> {
        self.command.as_deref()
    }

    /// Returns the process exit code, when reported.
    #[must_use]
    pub const fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }

    /// Returns the output chunk, when emitted.
    #[must_use]
    pub fn output(&self) -> Option<&str> {
        self.output.as_deref()
    }

    /// Returns the activity lifecycle state.
    #[must_use]
    pub const fn state(&self) -> TerminalActivityState {
        self.state
    }
}

/// Child tool row.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TranscriptTool {
    tool_id: ObservationId,
    tool_name: String,
    action: ToolAction,
    detail: Option<String>,
}

impl TranscriptTool {
    /// Creates child tool content after validating bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when the tool name is empty or exceeds
    /// [`OBSERVATION_LABEL_MAX_BYTES`] bytes, or a present detail is empty or
    /// exceeds [`OBSERVATION_TEXT_MAX_BYTES`] bytes.
    pub fn new(
        tool_id: ObservationId,
        tool_name: String,
        action: ToolAction,
        detail: Option<String>,
    ) -> Result<Self, ObservationError> {
        check_nonempty_text(&tool_name, "tool_name", OBSERVATION_LABEL_MAX_BYTES)?;
        check_optional_text(detail.as_deref(), "detail", OBSERVATION_TEXT_MAX_BYTES)?;
        Ok(Self {
            tool_id,
            tool_name,
            action,
            detail,
        })
    }

    /// Returns the child tool invocation identity.
    #[must_use]
    pub const fn tool_id(&self) -> &ObservationId {
        &self.tool_id
    }

    /// Returns the provider tool name.
    #[must_use]
    pub fn tool_name(&self) -> &str {
        &self.tool_name
    }

    /// Returns the lifecycle action.
    #[must_use]
    pub const fn action(&self) -> ToolAction {
        self.action
    }

    /// Returns the optional provider detail.
    #[must_use]
    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }
}

/// Child file row.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TranscriptFile {
    path: String,
    action: FileAction,
    lines_added: Option<u64>,
    lines_deleted: Option<u64>,
}

impl TranscriptFile {
    /// Creates child file content after validating bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when the path is empty or exceeds
    /// [`OBSERVATION_PATH_MAX_BYTES`] bytes, or a line count leaves its finite
    /// range.
    pub fn new(
        path: String,
        action: FileAction,
        lines_added: Option<u64>,
        lines_deleted: Option<u64>,
    ) -> Result<Self, ObservationError> {
        check_nonempty_text(&path, "path", OBSERVATION_PATH_MAX_BYTES)?;
        check_count(lines_added, "lines_added")?;
        check_count(lines_deleted, "lines_deleted")?;
        Ok(Self {
            path,
            action,
            lines_added,
            lines_deleted,
        })
    }

    /// Returns the affected path description.
    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    /// Returns the file action.
    #[must_use]
    pub const fn action(&self) -> FileAction {
        self.action
    }

    /// Returns counted added lines, or [`None`] when uncounted (never zero).
    #[must_use]
    pub const fn lines_added(&self) -> Option<u64> {
        self.lines_added
    }

    /// Returns counted deleted lines, or [`None`] when uncounted (never zero).
    #[must_use]
    pub const fn lines_deleted(&self) -> Option<u64> {
        self.lines_deleted
    }
}

/// Child search row.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TranscriptSearch {
    query: String,
    result_count: Option<u64>,
    scope: Option<SearchScope>,
    search_id: Option<ObservationId>,
    state: SearchState,
}

impl TranscriptSearch {
    /// Creates child search content after validating bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when the query is empty or exceeds
    /// [`OBSERVATION_QUERY_MAX_BYTES`] bytes, or the result count leaves its
    /// finite range.
    pub fn new(
        query: String,
        result_count: Option<u64>,
        scope: Option<SearchScope>,
        search_id: Option<ObservationId>,
        state: SearchState,
    ) -> Result<Self, ObservationError> {
        check_nonempty_text(&query, "query", OBSERVATION_QUERY_MAX_BYTES)?;
        check_count(result_count, "result_count")?;
        Ok(Self {
            query,
            result_count,
            scope,
            search_id,
            state,
        })
    }

    /// Returns the search query.
    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Returns the reported result count, when supplied.
    #[must_use]
    pub const fn result_count(&self) -> Option<u64> {
        self.result_count
    }

    /// Returns where the search looked, when disclosed.
    #[must_use]
    pub const fn scope(&self) -> Option<SearchScope> {
        self.scope
    }

    /// Returns the provider search identity, when supplied.
    #[must_use]
    pub const fn search_id(&self) -> Option<&ObservationId> {
        self.search_id.as_ref()
    }

    /// Returns the search lifecycle state.
    #[must_use]
    pub const fn state(&self) -> SearchState {
        self.state
    }
}

/// One public content row emitted by a native subagent.
///
/// Keeps child transcript provenance separate from the root conversation: the
/// row carries both native thread identities and renderer-safe projected
/// content, and always receives its own durable observation identity.
#[derive(Clone, Debug, PartialEq)]
pub struct SubagentTranscriptObservation {
    id: ObservationId,
    sequence: ObservationSequence,
    agent_native_thread_id: ObservationId,
    parent_native_thread_id: ObservationId,
    content: TranscriptContent,
}

impl SubagentTranscriptObservation {
    /// Creates a subagent transcript row.
    ///
    /// Infallible: identities arrive pre-validated.
    #[must_use]
    pub fn new(
        id: ObservationId,
        sequence: ObservationSequence,
        agent_native_thread_id: ObservationId,
        parent_native_thread_id: ObservationId,
        content: TranscriptContent,
    ) -> Self {
        Self {
            id,
            sequence,
            agent_native_thread_id,
            parent_native_thread_id,
            content,
        }
    }

    /// Returns the observation identity.
    #[must_use]
    pub const fn id(&self) -> &ObservationId {
        &self.id
    }

    /// Returns the durable sequence.
    #[must_use]
    pub const fn sequence(&self) -> ObservationSequence {
        self.sequence
    }

    /// Returns the provider identity of the child agent thread.
    #[must_use]
    pub const fn agent_native_thread_id(&self) -> &ObservationId {
        &self.agent_native_thread_id
    }

    /// Returns the provider identity of the parent thread.
    #[must_use]
    pub const fn parent_native_thread_id(&self) -> &ObservationId {
        &self.parent_native_thread_id
    }

    /// Returns the projected renderer-safe content.
    #[must_use]
    pub const fn content(&self) -> &TranscriptContent {
        &self.content
    }
}

// ---------------------------------------------------------------------------
// Usage
// ---------------------------------------------------------------------------

/// Provider usage accounting basis.
///
/// Preserved verbatim from the provider report.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum UsageBasis {
    /// Counts add to prior observations.
    Delta,
    /// Counts replace a provider-reported total.
    Cumulative,
    /// The source did not identify its accounting basis.
    Unknown,
}

impl UsageBasis {
    /// Returns the stable provider spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Delta => "delta",
            Self::Cumulative => "cumulative",
            Self::Unknown => "unknown",
        }
    }

    /// Parses a provider-disclosed usage basis.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError::UnknownValue`] for any other spelling.
    /// Unknown bases never default to another basis.
    pub fn parse(value: &str) -> Result<Self, ObservationError> {
        match value {
            "delta" => Ok(Self::Delta),
            "cumulative" => Ok(Self::Cumulative),
            "unknown" => Ok(Self::Unknown),
            _ => Err(ObservationError::UnknownValue { field: "basis" }),
        }
    }
}

impl From<RunUsageBasis> for UsageBasis {
    fn from(basis: RunUsageBasis) -> Self {
        match basis {
            RunUsageBasis::Delta => Self::Delta,
            RunUsageBasis::Cumulative => Self::Cumulative,
            RunUsageBasis::Unknown => Self::Unknown,
        }
    }
}

impl From<UsageBasis> for RunUsageBasis {
    fn from(basis: UsageBasis) -> Self {
        match basis {
            UsageBasis::Delta => Self::Delta,
            UsageBasis::Cumulative => Self::Cumulative,
            UsageBasis::Unknown => Self::Unknown,
        }
    }
}

/// Validated values used to construct one usage observation.
#[derive(Clone, Debug, PartialEq)]
pub struct UsageInput {
    /// Whether counts add to prior observations or replace a total.
    pub basis: UsageBasis,
    /// Provider-reported input token count.
    pub input_tokens: Option<u64>,
    /// Provider-reported cached input token count.
    pub cached_input_tokens: Option<u64>,
    /// Provider-reported output token count.
    pub output_tokens: Option<u64>,
    /// Tokens occupying the context window when measured. A point-in-time
    /// gauge, never additive: a newer report replaces an older one regardless
    /// of `basis`.
    pub context_tokens: Option<u64>,
    /// Provider-reported usable context window in tokens, when disclosed.
    pub context_window_tokens: Option<u64>,
    /// Provider-reported cost in US dollars, when available.
    pub cost_usd: Option<f64>,
    /// Route provenance keeping gateway billing distinct, when disclosed.
    pub provider_route_id: Option<ObservationId>,
    /// Provider assistant message or turn identity, when disclosed.
    pub turn_id: Option<ObservationId>,
}

/// One provider usage measurement for the run or one turn.
///
/// `Some(0)` is deliberately distinct from [`None`]: zero is a measured
/// value, absent means the provider did not report the field. Context tokens
/// are a gauge and must never be summed across reports, no matter the basis.
#[derive(Clone, Debug, PartialEq)]
pub struct UsageObservation {
    id: ObservationId,
    sequence: ObservationSequence,
    basis: UsageBasis,
    input_tokens: Option<u64>,
    cached_input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    context_tokens: Option<u64>,
    context_window_tokens: Option<u64>,
    cost_usd: Option<f64>,
    provider_route_id: Option<ObservationId>,
    turn_id: Option<ObservationId>,
}

impl UsageObservation {
    /// Creates a usage observation after validating counts and bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when a token count leaves its finite
    /// range, the context window is zero, or the cost is negative or not
    /// finite.
    pub fn new(
        id: ObservationId,
        sequence: ObservationSequence,
        input: UsageInput,
    ) -> Result<Self, ObservationError> {
        check_count(input.input_tokens, "input_tokens")?;
        check_count(input.cached_input_tokens, "cached_input_tokens")?;
        check_count(input.output_tokens, "output_tokens")?;
        check_count(input.context_tokens, "context_tokens")?;
        if let Some(window) = input.context_window_tokens {
            if window == 0 || window > OBSERVATION_COUNT_MAX {
                return Err(ObservationError::OutOfRange {
                    field: "context_window_tokens",
                });
            }
        }
        if input
            .cost_usd
            .is_some_and(|cost| !cost.is_finite() || cost < 0.0)
        {
            return Err(ObservationError::OutOfRange { field: "cost_usd" });
        }
        Ok(Self {
            id,
            sequence,
            basis: input.basis,
            input_tokens: input.input_tokens,
            cached_input_tokens: input.cached_input_tokens,
            output_tokens: input.output_tokens,
            context_tokens: input.context_tokens,
            context_window_tokens: input.context_window_tokens,
            cost_usd: input.cost_usd,
            provider_route_id: input.provider_route_id,
            turn_id: input.turn_id,
        })
    }

    /// Returns the observation identity.
    #[must_use]
    pub const fn id(&self) -> &ObservationId {
        &self.id
    }

    /// Returns the durable sequence.
    #[must_use]
    pub const fn sequence(&self) -> ObservationSequence {
        self.sequence
    }

    /// Returns whether counts add to prior observations or replace a total.
    #[must_use]
    pub const fn basis(&self) -> UsageBasis {
        self.basis
    }

    /// Returns the provider-reported input token count.
    #[must_use]
    pub const fn input_tokens(&self) -> Option<u64> {
        self.input_tokens
    }

    /// Returns the provider-reported cached input token count.
    #[must_use]
    pub const fn cached_input_tokens(&self) -> Option<u64> {
        self.cached_input_tokens
    }

    /// Returns the provider-reported output token count.
    #[must_use]
    pub const fn output_tokens(&self) -> Option<u64> {
        self.output_tokens
    }

    /// Returns the context gauge value. Never additive across reports.
    #[must_use]
    pub const fn context_tokens(&self) -> Option<u64> {
        self.context_tokens
    }

    /// Returns the provider-reported usable context window, when disclosed.
    #[must_use]
    pub const fn context_window_tokens(&self) -> Option<u64> {
        self.context_window_tokens
    }

    /// Returns the provider-reported cost in US dollars, when available.
    #[must_use]
    pub const fn cost_usd(&self) -> Option<f64> {
        self.cost_usd
    }

    /// Returns the route provenance, when disclosed.
    #[must_use]
    pub const fn provider_route_id(&self) -> Option<&ObservationId> {
        self.provider_route_id.as_ref()
    }

    /// Returns the provider turn identity, when disclosed.
    #[must_use]
    pub const fn turn_id(&self) -> Option<&ObservationId> {
        self.turn_id.as_ref()
    }
}

// ---------------------------------------------------------------------------
// Native action + diagnostics + error reference
// ---------------------------------------------------------------------------

/// Stable `AE-*` artisan error code.
///
/// The adapter translates the provider's typed signal into this vocabulary at
/// the boundary; the provider's own code rides along as evidence in
/// [`EngineErrorRef`].
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ArtisanCode(String);

impl ArtisanCode {
    /// Creates an artisan code after validating the `AE-*` shape.
    ///
    /// The value must start with `AE-`, carry a non-empty suffix of ASCII
    /// uppercase letters, digits, or hyphens, and fit
    /// [`OBSERVATION_ARTISAN_CODE_MAX_BYTES`] bytes.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError::UnknownValue`] when the value is not a
    /// modeled `AE-*` code.
    pub fn parse(value: impl Into<String>) -> Result<Self, ObservationError> {
        const FIELD: &str = "artisan_code";
        let value = value.into();
        let valid = value.len() <= OBSERVATION_ARTISAN_CODE_MAX_BYTES
            && value.starts_with("AE-")
            && value.len() > 3
            && value
                .bytes()
                .skip(3)
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'-');
        if valid {
            Ok(Self(value))
        } else {
            Err(ObservationError::UnknownValue { field: FIELD })
        }
    }

    /// Returns the validated code.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Scope of a depleted provider allowance, when disclosed.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum LimitScope {
    /// The depleted allowance is shared.
    Shared,
    /// The depleted allowance is model-specific.
    Model,
    /// The provider did not disclose the scope.
    Unknown,
}

impl LimitScope {
    /// Returns the stable provider spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Shared => "shared",
            Self::Model => "model",
            Self::Unknown => "unknown",
        }
    }

    /// Parses a provider-disclosed limit scope.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError::UnknownValue`] for any other spelling.
    pub fn parse(value: &str) -> Result<Self, ObservationError> {
        match value {
            "shared" => Ok(Self::Shared),
            "model" => Ok(Self::Model),
            "unknown" => Ok(Self::Unknown),
            _ => Err(ObservationError::UnknownValue {
                field: "limit_scope",
            }),
        }
    }
}

/// Validated values used to construct one engine error reference.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct EngineErrorRefInput {
    /// Stable `AE-*` artisan code.
    pub artisan_code: ArtisanCode,
    /// Provider's own error code, when disclosed.
    pub provider_code: Option<String>,
    /// Renderer-safe explanation, when supplied.
    pub detail: Option<String>,
    /// Provider model whose allowance was depleted, when disclosed.
    pub affected_model_id: Option<String>,
    /// Provider quota-bucket identifier, when disclosed.
    pub limit_id: Option<String>,
    /// Provider quota-bucket label, when disclosed.
    pub limit_label: Option<String>,
    /// Whether the depleted allowance is shared, model-specific, or unknown.
    pub limit_scope: Option<LimitScope>,
    /// When a limit-class failure clears, as an ISO timestamp, when disclosed.
    pub resets_at: Option<String>,
}

/// One provider failure transferred into Artisan's custody.
///
/// Everything downstream reasons in the `AE-*` vocabulary while the
/// provider's own code and message ride along as evidence.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct EngineErrorRef {
    artisan_code: ArtisanCode,
    provider_code: Option<String>,
    detail: Option<String>,
    affected_model_id: Option<String>,
    limit_id: Option<String>,
    limit_label: Option<String>,
    limit_scope: Option<LimitScope>,
    resets_at: Option<String>,
}

impl EngineErrorRef {
    /// Creates an error reference after validating bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when a present evidence value is empty or
    /// exceeds its ceiling.
    pub fn new(input: EngineErrorRefInput) -> Result<Self, ObservationError> {
        check_optional_text(
            input.provider_code.as_deref(),
            "provider_code",
            OBSERVATION_PROVIDER_CODE_MAX_BYTES,
        )?;
        check_optional_text(
            input.detail.as_deref(),
            "detail",
            OBSERVATION_REASON_MAX_BYTES,
        )?;
        check_optional_text(
            input.affected_model_id.as_deref(),
            "affected_model_id",
            OBSERVATION_PROVIDER_CODE_MAX_BYTES,
        )?;
        check_optional_text(
            input.limit_id.as_deref(),
            "limit_id",
            OBSERVATION_PROVIDER_CODE_MAX_BYTES,
        )?;
        check_optional_text(
            input.limit_label.as_deref(),
            "limit_label",
            OBSERVATION_LIMIT_LABEL_MAX_BYTES,
        )?;
        if let Some(resets_at) = input.resets_at.as_deref() {
            if resets_at.is_empty() {
                return Err(ObservationError::Empty { field: "resets_at" });
            }
            if resets_at.len() > OBSERVATION_TIMESTAMP_MAX_BYTES {
                return Err(ObservationError::TooLong {
                    field: "resets_at",
                    length: resets_at.len(),
                    maximum: OBSERVATION_TIMESTAMP_MAX_BYTES,
                });
            }
            if resets_at
                .chars()
                .any(|character| character.is_whitespace() || character.is_control())
            {
                return Err(ObservationError::Identifier(
                    IdentifierError::ForbiddenCharacter {
                        character: resets_at
                            .chars()
                            .find(|character| character.is_whitespace() || character.is_control())
                            .unwrap_or('\0'),
                    },
                ));
            }
        }
        Ok(Self {
            artisan_code: input.artisan_code,
            provider_code: input.provider_code,
            detail: input.detail,
            affected_model_id: input.affected_model_id,
            limit_id: input.limit_id,
            limit_label: input.limit_label,
            limit_scope: input.limit_scope,
            resets_at: input.resets_at,
        })
    }

    /// Returns the stable `AE-*` artisan code.
    #[must_use]
    pub const fn artisan_code(&self) -> &ArtisanCode {
        &self.artisan_code
    }

    /// Returns the provider's own error code, when disclosed.
    #[must_use]
    pub fn provider_code(&self) -> Option<&str> {
        self.provider_code.as_deref()
    }

    /// Returns the renderer-safe explanation, when supplied.
    #[must_use]
    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }

    /// Returns the affected provider model, when disclosed.
    #[must_use]
    pub fn affected_model_id(&self) -> Option<&str> {
        self.affected_model_id.as_deref()
    }

    /// Returns the provider quota-bucket identifier, when disclosed.
    #[must_use]
    pub fn limit_id(&self) -> Option<&str> {
        self.limit_id.as_deref()
    }

    /// Returns the provider quota-bucket label, when disclosed.
    #[must_use]
    pub fn limit_label(&self) -> Option<&str> {
        self.limit_label.as_deref()
    }

    /// Returns the allowance scope, when disclosed.
    #[must_use]
    pub const fn limit_scope(&self) -> Option<LimitScope> {
        self.limit_scope
    }

    /// Returns when a limit-class failure clears, when disclosed.
    #[must_use]
    pub fn resets_at(&self) -> Option<&str> {
        self.resets_at.as_deref()
    }
}

/// One provider-native action with no canonical tool equivalent.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeActionObservation {
    id: ObservationId,
    sequence: ObservationSequence,
    action: String,
    detail: Option<String>,
    diagnostic: bool,
    error_ref: Option<EngineErrorRef>,
}

impl NativeActionObservation {
    /// Creates a native action observation after validating bounds.
    ///
    /// A `diagnostic` action marks a frame the adapter could not interpret
    /// rather than something the provider did; it is ordinary drift, not a
    /// fault.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when an identity is invalid, the action is
    /// empty or exceeds [`OBSERVATION_LABEL_MAX_BYTES`] bytes, or a present
    /// detail is empty or exceeds [`OBSERVATION_TEXT_MAX_BYTES`] bytes.
    pub fn new(
        id: ObservationId,
        sequence: ObservationSequence,
        action: String,
        detail: Option<String>,
        diagnostic: bool,
        error_ref: Option<EngineErrorRef>,
    ) -> Result<Self, ObservationError> {
        check_nonempty_text(&action, "action", OBSERVATION_LABEL_MAX_BYTES)?;
        check_optional_text(detail.as_deref(), "detail", OBSERVATION_TEXT_MAX_BYTES)?;
        Ok(Self {
            id,
            sequence,
            action,
            detail,
            diagnostic,
            error_ref,
        })
    }

    /// Returns the observation identity.
    #[must_use]
    pub const fn id(&self) -> &ObservationId {
        &self.id
    }

    /// Returns the durable sequence.
    #[must_use]
    pub const fn sequence(&self) -> ObservationSequence {
        self.sequence
    }

    /// Returns the provider-native action name.
    #[must_use]
    pub fn action(&self) -> &str {
        &self.action
    }

    /// Returns the optional provider detail.
    #[must_use]
    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }

    /// Returns whether this row marks uninterpretable drift rather than
    /// provider activity.
    #[must_use]
    pub const fn diagnostic(&self) -> bool {
        self.diagnostic
    }

    /// Returns the classified failure, when the action reported one.
    #[must_use]
    pub const fn error_ref(&self) -> Option<&EngineErrorRef> {
        self.error_ref.as_ref()
    }
}

/// Severity of one process or protocol diagnostic.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum DiagnosticLevel {
    /// Informational diagnostic.
    Info,
    /// Warning diagnostic.
    Warning,
    /// Error diagnostic.
    Error,
}

impl DiagnosticLevel {
    /// Returns the stable provider spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }

    /// Parses a provider-disclosed diagnostic level.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError::UnknownValue`] for any other spelling.
    pub fn parse(value: &str) -> Result<Self, ObservationError> {
        match value {
            "info" => Ok(Self::Info),
            "warning" => Ok(Self::Warning),
            "error" => Ok(Self::Error),
            _ => Err(ObservationError::UnknownValue { field: "level" }),
        }
    }
}

/// One process-level diagnostic from the engine host.
#[derive(Clone, Debug, PartialEq)]
pub struct ProcessDiagnosticObservation {
    id: ObservationId,
    sequence: ObservationSequence,
    level: DiagnosticLevel,
    message: String,
    error_ref: Option<EngineErrorRef>,
}

impl ProcessDiagnosticObservation {
    /// Creates a process diagnostic after validating identities and bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when an identity is invalid or the message
    /// is empty or exceeds [`OBSERVATION_TEXT_MAX_BYTES`] bytes.
    pub fn new(
        id: ObservationId,
        sequence: ObservationSequence,
        level: DiagnosticLevel,
        message: String,
        error_ref: Option<EngineErrorRef>,
    ) -> Result<Self, ObservationError> {
        check_nonempty_text(&message, "message", OBSERVATION_TEXT_MAX_BYTES)?;
        Ok(Self {
            id,
            sequence,
            level,
            message,
            error_ref,
        })
    }

    /// Returns the observation identity.
    #[must_use]
    pub const fn id(&self) -> &ObservationId {
        &self.id
    }

    /// Returns the durable sequence.
    #[must_use]
    pub const fn sequence(&self) -> ObservationSequence {
        self.sequence
    }

    /// Returns the diagnostic severity.
    #[must_use]
    pub const fn level(&self) -> DiagnosticLevel {
        self.level
    }

    /// Returns the diagnostic message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Returns the classified failure, when the diagnostic reported one.
    #[must_use]
    pub const fn error_ref(&self) -> Option<&EngineErrorRef> {
        self.error_ref.as_ref()
    }
}

/// One decoded transport or protocol diagnostic.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ProtocolDiagnosticObservation {
    id: ObservationId,
    sequence: ObservationSequence,
    level: DiagnosticLevel,
    message: String,
}

impl ProtocolDiagnosticObservation {
    /// Creates a protocol diagnostic after validating identities and bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when an identity is invalid or the message
    /// is empty or exceeds [`OBSERVATION_TEXT_MAX_BYTES`] bytes.
    pub fn new(
        id: ObservationId,
        sequence: ObservationSequence,
        level: DiagnosticLevel,
        message: String,
    ) -> Result<Self, ObservationError> {
        check_nonempty_text(&message, "message", OBSERVATION_TEXT_MAX_BYTES)?;
        Ok(Self {
            id,
            sequence,
            level,
            message,
        })
    }

    /// Returns the observation identity.
    #[must_use]
    pub const fn id(&self) -> &ObservationId {
        &self.id
    }

    /// Returns the durable sequence.
    #[must_use]
    pub const fn sequence(&self) -> ObservationSequence {
        self.sequence
    }

    /// Returns the diagnostic severity.
    #[must_use]
    pub const fn level(&self) -> DiagnosticLevel {
        self.level
    }

    /// Returns the diagnostic message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

// ---------------------------------------------------------------------------
// Run terminal
// ---------------------------------------------------------------------------

/// The only outcomes that can complete an engine run.
///
/// `Interrupted` separates "something ended this run from outside" (host
/// restart, shutdown, externally killed process) from `Failed`, which claims
/// the work itself went wrong. Only the former can be picked back up.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RunTerminalState {
    /// The run completed.
    Completed,
    /// The run was cancelled.
    Cancelled,
    /// The work itself went wrong.
    Failed,
    /// Something ended the run from outside.
    Interrupted,
    /// The run closed and released its scoped resources.
    Closed,
}

impl RunTerminalState {
    /// Returns the stable provider spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
            Self::Closed => "closed",
        }
    }

    /// Parses a provider-disclosed terminal state.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError::UnknownValue`] for any other spelling.
    /// Interrupted runs never collapse into cancellation.
    pub fn parse(value: &str) -> Result<Self, ObservationError> {
        match value {
            "completed" => Ok(Self::Completed),
            "cancelled" => Ok(Self::Cancelled),
            "failed" => Ok(Self::Failed),
            "interrupted" => Ok(Self::Interrupted),
            "closed" => Ok(Self::Closed),
            _ => Err(ObservationError::UnknownValue { field: "state" }),
        }
    }
}

/// The sole terminal outcome emitted by a run.
#[derive(Clone, Debug, PartialEq)]
pub struct RunTerminalObservation {
    id: ObservationId,
    sequence: ObservationSequence,
    state: RunTerminalState,
    error_ref: Option<EngineErrorRef>,
    summary_title: Option<String>,
}

impl RunTerminalObservation {
    /// Creates a terminal observation after validating identities and bounds.
    ///
    /// # Errors
    ///
    /// Returns [`ObservationError`] when an identity is invalid or a present
    /// session title is empty or exceeds [`OBSERVATION_TITLE_MAX_BYTES`]
    /// bytes.
    pub fn new(
        id: ObservationId,
        sequence: ObservationSequence,
        state: RunTerminalState,
        error_ref: Option<EngineErrorRef>,
        summary_title: Option<String>,
    ) -> Result<Self, ObservationError> {
        check_optional_text(
            summary_title.as_deref(),
            "summary_title",
            OBSERVATION_TITLE_MAX_BYTES,
        )?;
        Ok(Self {
            id,
            sequence,
            state,
            error_ref,
            summary_title,
        })
    }

    /// Returns the observation identity.
    #[must_use]
    pub const fn id(&self) -> &ObservationId {
        &self.id
    }

    /// Returns the durable sequence.
    #[must_use]
    pub const fn sequence(&self) -> ObservationSequence {
        self.sequence
    }

    /// Returns the terminal outcome.
    #[must_use]
    pub const fn state(&self) -> RunTerminalState {
        self.state
    }

    /// Returns the artisan-owned failure evidence for failed outcomes.
    #[must_use]
    pub const fn error_ref(&self) -> Option<&EngineErrorRef> {
        self.error_ref.as_ref()
    }

    /// Returns the harness-generated session title, when the engine produced
    /// one by the time the run settled.
    #[must_use]
    pub fn summary_title(&self) -> Option<&str> {
        self.summary_title.as_deref()
    }
}

// ---------------------------------------------------------------------------
// Observation union
// ---------------------------------------------------------------------------

/// The ordered, provider-neutral event stream emitted by a run.
///
/// Tags match the TypeScript `EngineObservation` union one for one. Every
/// variant carries its own durable [`ObservationSequence`]; live-stream
/// ordering is monotonic per run while durable sequencing is enforced by the
/// persistence layer.
#[derive(Clone, Debug, PartialEq)]
pub enum Observation {
    /// A streamed agent message fragment.
    AgentMessageDelta(AgentMessageDeltaObservation),
    /// A completed agent message.
    AgentMessageCompleted(AgentMessageCompletedObservation),
    /// An approval request or its resolution.
    Approval(ApprovalObservation),
    /// A provider context compaction report.
    Compaction(CompactionObservation),
    /// A file mutation or inspection.
    File(FileObservation),
    /// A provider-native action with no canonical tool equivalent.
    NativeAction(NativeActionObservation),
    /// A provider-neutral plan update.
    Plan(PlanObservation),
    /// A process-level diagnostic from the engine host.
    ProcessDiagnostic(ProcessDiagnosticObservation),
    /// A decoded transport or protocol diagnostic.
    ProtocolDiagnostic(ProtocolDiagnosticObservation),
    /// A question request or its resolution.
    Question(QuestionObservation),
    /// A settled reasoning phase for one turn.
    ReasoningSummaryCompleted(ReasoningSummaryCompletedObservation),
    /// A streamed reasoning summary fragment.
    ReasoningSummaryDelta(ReasoningSummaryDeltaObservation),
    /// A provider error with its continuation intent.
    Retry(RetryObservation),
    /// A non-terminal lifecycle change for the run.
    RunState(RunStateObservation),
    /// The sole terminal outcome of the run.
    RunTerminal(RunTerminalObservation),
    /// A search operation.
    Search(SearchObservation),
    /// Provider-native subagent activity.
    Subagent(SubagentObservation),
    /// Public content emitted by one native subagent.
    SubagentTranscript(SubagentTranscriptObservation),
    /// Shell or process activity independent from the run outcome.
    TerminalActivity(TerminalActivityObservation),
    /// A tool lifecycle event.
    Tool(ToolObservation),
    /// Lifecycle progress for a single provider turn.
    TurnState(TurnStateObservation),
    /// Provider usage measured for the run or one turn.
    Usage(UsageObservation),
}

impl Observation {
    /// Returns the stable provider tag of this observation.
    #[must_use]
    pub const fn tag(&self) -> &'static str {
        match self {
            Self::AgentMessageDelta(_) => "agent_message_delta",
            Self::AgentMessageCompleted(_) => "agent_message_completed",
            Self::Approval(_) => "approval",
            Self::Compaction(_) => "compaction",
            Self::File(_) => "file",
            Self::NativeAction(_) => "native_action",
            Self::Plan(_) => "plan",
            Self::ProcessDiagnostic(_) => "process_diagnostic",
            Self::ProtocolDiagnostic(_) => "protocol_diagnostic",
            Self::Question(_) => "question",
            Self::ReasoningSummaryCompleted(_) => "reasoning_summary_completed",
            Self::ReasoningSummaryDelta(_) => "reasoning_summary_delta",
            Self::Retry(_) => "retry",
            Self::RunState(_) => "run_state",
            Self::RunTerminal(_) => "run_terminal",
            Self::Search(_) => "search",
            Self::Subagent(_) => "subagent",
            Self::SubagentTranscript(_) => "subagent_transcript",
            Self::TerminalActivity(_) => "terminal_activity",
            Self::Tool(_) => "tool",
            Self::TurnState(_) => "turn_state",
            Self::Usage(_) => "usage",
        }
    }

    /// Returns the observation identity.
    #[must_use]
    pub fn observation_id(&self) -> &ObservationId {
        match self {
            Self::AgentMessageDelta(value) => value.id(),
            Self::AgentMessageCompleted(value) => value.id(),
            Self::Approval(value) => value.id(),
            Self::Compaction(value) => value.id(),
            Self::File(value) => value.id(),
            Self::NativeAction(value) => value.id(),
            Self::Plan(value) => value.id(),
            Self::ProcessDiagnostic(value) => value.id(),
            Self::ProtocolDiagnostic(value) => value.id(),
            Self::Question(value) => value.id(),
            Self::ReasoningSummaryCompleted(value) => value.id(),
            Self::ReasoningSummaryDelta(value) => value.id(),
            Self::Retry(value) => value.id(),
            Self::RunState(value) => value.id(),
            Self::RunTerminal(value) => value.id(),
            Self::Search(value) => value.id(),
            Self::Subagent(value) => value.id(),
            Self::SubagentTranscript(value) => value.id(),
            Self::TerminalActivity(value) => value.id(),
            Self::Tool(value) => value.id(),
            Self::TurnState(value) => value.id(),
            Self::Usage(value) => value.id(),
        }
    }

    /// Returns the durable sequence.
    #[must_use]
    pub fn sequence(&self) -> ObservationSequence {
        match self {
            Self::AgentMessageDelta(value) => value.sequence(),
            Self::AgentMessageCompleted(value) => value.sequence(),
            Self::Approval(value) => value.sequence(),
            Self::Compaction(value) => value.sequence(),
            Self::File(value) => value.sequence(),
            Self::NativeAction(value) => value.sequence(),
            Self::Plan(value) => value.sequence(),
            Self::ProcessDiagnostic(value) => value.sequence(),
            Self::ProtocolDiagnostic(value) => value.sequence(),
            Self::Question(value) => value.sequence(),
            Self::ReasoningSummaryCompleted(value) => value.sequence(),
            Self::ReasoningSummaryDelta(value) => value.sequence(),
            Self::Retry(value) => value.sequence(),
            Self::RunState(value) => value.sequence(),
            Self::RunTerminal(value) => value.sequence(),
            Self::Search(value) => value.sequence(),
            Self::Subagent(value) => value.sequence(),
            Self::SubagentTranscript(value) => value.sequence(),
            Self::TerminalActivity(value) => value.sequence(),
            Self::Tool(value) => value.sequence(),
            Self::TurnState(value) => value.sequence(),
            Self::Usage(value) => value.sequence(),
        }
    }

    /// Returns whether this row belongs to a child agent rather than the root
    /// conversation. Child rows never render in the root transcript.
    #[must_use]
    pub const fn is_child(&self) -> bool {
        match self {
            Self::Subagent(_) | Self::SubagentTranscript(_) => true,
            Self::AgentMessageDelta(_)
            | Self::AgentMessageCompleted(_)
            | Self::Approval(_)
            | Self::Compaction(_)
            | Self::File(_)
            | Self::NativeAction(_)
            | Self::Plan(_)
            | Self::ProcessDiagnostic(_)
            | Self::ProtocolDiagnostic(_)
            | Self::Question(_)
            | Self::ReasoningSummaryCompleted(_)
            | Self::ReasoningSummaryDelta(_)
            | Self::Retry(_)
            | Self::RunState(_)
            | Self::RunTerminal(_)
            | Self::Search(_)
            | Self::TerminalActivity(_)
            | Self::Tool(_)
            | Self::TurnState(_)
            | Self::Usage(_) => false,
        }
    }
}
