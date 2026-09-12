//! Agent message and reasoning streams plus renderer-safe subagent
//! transcript content.

use super::tooling::{
    FileAction, SearchScope, SearchState, TerminalActivityState, TerminalChannel, ToolAction,
};
use super::{
    MessagePhase, OBSERVATION_COMMAND_MAX_BYTES, OBSERVATION_DELTA_MAX_BYTES,
    OBSERVATION_LABEL_MAX_BYTES, OBSERVATION_MESSAGE_MAX_BYTES, OBSERVATION_OUTPUT_MAX_BYTES,
    OBSERVATION_PATH_MAX_BYTES, OBSERVATION_QUERY_MAX_BYTES, OBSERVATION_SUMMARY_INDEX_MAX,
    OBSERVATION_TEXT_MAX_BYTES, Observation, ObservationError, ObservationId, ObservationSequence,
    check_count, check_nonempty_text, check_optional_text, check_text,
};

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
// Subagent transcript projection
// ---------------------------------------------------------------------------

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
