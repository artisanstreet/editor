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
//!
//! The vocabulary is split into private family submodules whose items are
//! re-exported here: `conversation` holds agent message and reasoning
//! streams plus renderer-safe subagent transcript content; `tooling` holds
//! tool, file, search, and terminal activity; `interaction` holds approvals
//! and questions; `lifecycle` holds plan, compaction, retry, run/turn state,
//! subagent lifecycle, and run terminal outcomes; `usage` holds provider
//! usage measurements; and `diagnostics` holds artisan error codes, error
//! references, native actions, and process/protocol diagnostics.

use thiserror::Error;

use crate::bounds::{
    CONVERSATION_TEXT_FRAGMENT_MAX_BYTES, ENGINE_RUNTIME_MAX_MILLIS, IDENTIFIER_MAX_BYTES,
    MESSAGE_BODY_MAX_BYTES, THREAD_TITLE_MAX_BYTES,
};
use crate::identifiers::IdentifierError;

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

mod conversation;
mod diagnostics;
mod interaction;
mod lifecycle;
mod tooling;
mod usage;

pub use conversation::{
    AgentMessageCompletedObservation, AgentMessageDeltaObservation,
    ReasoningSummaryCompletedObservation, ReasoningSummaryDeltaObservation,
    SubagentTranscriptObservation, TranscriptAgentMessageCompleted, TranscriptAgentMessageDelta,
    TranscriptContent, TranscriptFile, TranscriptReasoningSummaryCompleted,
    TranscriptReasoningSummaryDelta, TranscriptSearch, TranscriptTerminalActivity, TranscriptTool,
};
pub use diagnostics::{
    ArtisanCode, DiagnosticLevel, EngineErrorRef, EngineErrorRefInput, LimitScope,
    NativeActionObservation, ProcessDiagnosticObservation, ProtocolDiagnosticObservation,
};
pub use interaction::{
    ApprovalKind, ApprovalObservation, ApprovalRequest, ApprovalState, QuestionInput,
    QuestionObservation, QuestionOption, QuestionState,
};
pub use lifecycle::{
    CompactionObservation, CompactionState, PlanEntry, PlanEntryStatus, PlanObservation,
    RetryAttemptState, RetryObservation, RunState, RunStateObservation, RunTerminalObservation,
    RunTerminalState, SubagentInput, SubagentObservation, SubagentState, TurnState,
    TurnStateObservation,
};
pub use tooling::{
    FileAction, FileObservation, SearchObservation, SearchScope, SearchState,
    TerminalActivityInput, TerminalActivityObservation, TerminalActivityState, TerminalChannel,
    ToolAction, ToolObservation,
};
pub use usage::{UsageBasis, UsageInput, UsageObservation};

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
