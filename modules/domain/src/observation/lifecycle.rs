//! Plan, compaction, retry, run/turn state, subagent, and run terminal
//! lifecycle observations.

use super::diagnostics::EngineErrorRef;
use super::{
    OBSERVATION_DURATION_MAX_MILLIS, OBSERVATION_PATH_MAX_BYTES, OBSERVATION_PLAN_MAX_ENTRIES,
    OBSERVATION_PLAN_TEXT_MAX_BYTES, OBSERVATION_TEXT_MAX_BYTES, OBSERVATION_TITLE_MAX_BYTES,
    ObservationError, ObservationId, ObservationSequence, check_nonempty_text, check_optional_text,
};

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
// Subagent lifecycle
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
/// separately through
/// [`Observation::SubagentTranscript`](super::Observation::SubagentTranscript).
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

// ---------------------------------------------------------------------------
// Run terminal
// ---------------------------------------------------------------------------

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
