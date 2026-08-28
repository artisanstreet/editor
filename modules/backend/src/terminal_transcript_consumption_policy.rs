//! Dependency-free terminal-transcript consumption policy.
//!
//! The TypeScript orchestration boundary owns the transaction and its three
//! durable lookups. This module receives only the facts those lookups expose
//! and returns a deterministic no-op or a precisely guarded delete action. It
//! does not access a database, execute SQL, decode transcript content, or
//! retain runtime state.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

/// Root-run statuses that the orchestration persistence boundary can expose.
///
/// The oracle treats only `queued`, `running`, and `waiting` as active. Every
/// other status is settled for this policy, including a future status carried
/// by [`Self::Other`]. Keeping the unknown spelling preserves the open text
/// column without allowing an unknown value to accidentally become active.
#[must_use]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum RootRunStatus {
    /// The root has been admitted but has not started.
    Queued,
    /// The root is actively executing.
    Running,
    /// The root is waiting for an external response.
    Waiting,
    /// The root completed successfully.
    Completed,
    /// The root settled with a failure.
    Failed,
    /// The root was cancelled.
    Cancelled,
    /// The root was interrupted and did not complete normally.
    Interrupted,
    /// A status not yet named by the native vocabulary.
    Other(String),
}

impl RootRunStatus {
    /// The active root statuses in the exact oracle membership order.
    pub const ACTIVE: [Self; 3] = [Self::Queued, Self::Running, Self::Waiting];

    /// The known settled root statuses in their durable vocabulary order.
    pub const SETTLED: [Self; 4] = [
        Self::Completed,
        Self::Failed,
        Self::Cancelled,
        Self::Interrupted,
    ];

    /// Every known root status in active-then-settled order.
    pub const ALL: [Self; 7] = [
        Self::Queued,
        Self::Running,
        Self::Waiting,
        Self::Completed,
        Self::Failed,
        Self::Cancelled,
        Self::Interrupted,
    ];

    /// Converts one persisted status spelling into typed policy data.
    ///
    /// Unknown spellings remain settled, exactly as they do when the oracle
    /// checks membership in its three-element active set.
    #[must_use = "retain the typed root status"]
    pub fn from_runtime(value: &str) -> Self {
        match value {
            "queued" => Self::Queued,
            "running" => Self::Running,
            "waiting" => Self::Waiting,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            "interrupted" => Self::Interrupted,
            _ => Self::Other(value.to_owned()),
        }
    }

    /// Returns the exact persisted spelling represented by this status.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Interrupted => "interrupted",
            Self::Other(value) => value,
        }
    }

    /// Returns whether this status belongs to the oracle's active set.
    #[must_use]
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Queued | Self::Running | Self::Waiting)
            || matches!(self, Self::Other(value) if matches!(value.as_str(), "queued" | "running" | "waiting"))
    }

    /// Returns whether this status is eligible for the settled-root branch.
    #[must_use]
    pub fn is_settled(&self) -> bool {
        !self.is_active()
    }
}

impl From<&str> for RootRunStatus {
    fn from(value: &str) -> Self {
        Self::from_runtime(value)
    }
}

impl From<String> for RootRunStatus {
    fn from(value: String) -> Self {
        Self::from_runtime(&value)
    }
}

/// The root-run fact selected for a transcript-consumption decision.
#[must_use]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct RootRunObservation {
    /// The exact persisted status of the root run.
    pub status: RootRunStatus,
}

impl RootRunObservation {
    /// Creates a root fact without rewriting its status.
    #[must_use = "retain the root-run observation"]
    pub const fn new(status: RootRunStatus) -> Self {
        Self { status }
    }
}

/// The pending columns selected from one transcript-inbox observation.
///
/// The observation ID is the explicit consume-call identity on
/// [`TerminalTranscriptConsumptionInput`], matching the oracle's initial
/// `observation_id` predicate. Transcript content, sequence, parent identity,
/// and timestamps are intentionally outside this policy because none is
/// consulted by the consume decision.
#[must_use]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct PendingTranscriptObservation {
    /// Root run named by the inbox row.
    pub root_run: String,
    /// Engine named by the inbox row.
    pub engine: String,
    /// Provider-native child thread named by the inbox row.
    pub agent_native_thread: String,
}

impl PendingTranscriptObservation {
    /// Creates the exact selected pending columns without normalization.
    #[must_use = "retain the pending transcript observation"]
    pub fn new(
        root_run_id: impl Into<String>,
        engine_id: impl Into<String>,
        agent_native_thread_id: impl Into<String>,
    ) -> Self {
        Self {
            root_run: root_run_id.into(),
            engine: engine_id.into(),
            agent_native_thread: agent_native_thread_id.into(),
        }
    }
}

/// The result of the inbox lookup that precedes root and binding lookups.
///
/// A processed row is intentionally distinct from a missing row at the input
/// boundary even though both have the same no-op decision, preserving the two
/// oracle cases without exposing a timestamp or a database row.
#[must_use]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum TerminalTranscriptInboxObservation {
    /// No row matched the requested identity and unprocessed predicate.
    Missing,
    /// A row exists for the identity but has already been processed.
    AlreadyProcessed,
    /// A row matched the identity and is still unprocessed.
    Pending(PendingTranscriptObservation),
}

impl TerminalTranscriptInboxObservation {
    /// Creates a missing-inbox fact.
    pub const fn missing() -> Self {
        Self::Missing
    }

    /// Creates an already-processed-inbox fact.
    pub const fn already_processed() -> Self {
        Self::AlreadyProcessed
    }

    /// Wraps the exact selected columns of an unprocessed inbox row.
    #[must_use = "retain the pending inbox observation"]
    pub const fn pending(observation: PendingTranscriptObservation) -> Self {
        Self::Pending(observation)
    }
}

/// A durable child binding candidate selected from the native-subagent table.
///
/// A candidate is an exact match only when all three fields equal the pending
/// inbox observation. Binding IDs and all other durable columns are irrelevant
/// to the oracle's existence check and are therefore not represented.
#[must_use]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct NativeSubagentBinding {
    /// Engine identity in the durable binding.
    pub engine: String,
    /// Root-run identity in the durable binding.
    pub root_run: String,
    /// Provider-native child thread identity in the durable binding.
    pub agent_native_thread: String,
}

impl NativeSubagentBinding {
    /// Creates one binding candidate without normalization.
    #[must_use = "retain the durable child binding"]
    pub fn new(
        engine_id: impl Into<String>,
        root_run_id: impl Into<String>,
        agent_native_thread_id: impl Into<String>,
    ) -> Self {
        Self {
            engine: engine_id.into(),
            root_run: root_run_id.into(),
            agent_native_thread: agent_native_thread_id.into(),
        }
    }

    /// Returns whether this candidate satisfies the oracle's three-field
    /// durable child-binding predicate.
    #[must_use]
    pub fn matches(&self, pending: &PendingTranscriptObservation) -> bool {
        self.engine == pending.engine
            && self.root_run == pending.root_run
            && self.agent_native_thread == pending.agent_native_thread
    }
}

/// All facts supplied to one terminal-transcript consumption decision.
#[must_use]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct TerminalTranscriptConsumptionInput {
    /// Exact identity supplied to the consume operation.
    pub observation_id: String,
    /// Result of the identity-plus-unprocessed inbox lookup.
    pub inbox: TerminalTranscriptInboxObservation,
    /// Optional root-run row selected by the inbox row's root ID.
    pub root_run: Option<RootRunObservation>,
    /// All binding candidates visible to the durable existence lookup.
    pub bindings: Vec<NativeSubagentBinding>,
}

impl TerminalTranscriptConsumptionInput {
    /// Creates a complete typed fact set while preserving all supplied values.
    #[must_use = "retain the transcript-consumption input"]
    pub fn new<I>(
        observation_id: impl Into<String>,
        inbox: TerminalTranscriptInboxObservation,
        root_run: Option<RootRunObservation>,
        bindings: I,
    ) -> Self
    where
        I: IntoIterator<Item = NativeSubagentBinding>,
    {
        Self {
            observation_id: observation_id.into(),
            inbox,
            root_run,
            bindings: bindings.into_iter().collect(),
        }
    }
}

/// A typed guard proving that a delete must still require an unprocessed row.
///
/// This marker carries no timestamp and performs no lookup. Its presence in
/// [`ConditionalDeleteTranscriptAction`] keeps the second oracle delete
/// predicate explicit at the action boundary.
#[must_use]
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct UnprocessedTranscriptGuard;

impl UnprocessedTranscriptGuard {
    /// Creates the only guard admitted by the consumption policy.
    pub const fn new() -> Self {
        Self
    }

    /// Confirms that this guard means `processed_at IS NULL`.
    #[must_use]
    pub const fn is_unprocessed() -> bool {
        true
    }
}

/// The exact conditional delete admitted for an unbound settled observation.
#[must_use]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ConditionalDeleteTranscriptAction {
    /// Exact inbox identity to retain in the delete predicate.
    pub observation_id: String,
    /// Required unprocessed-state fence for the delete predicate.
    pub guard: UnprocessedTranscriptGuard,
}

impl ConditionalDeleteTranscriptAction {
    /// Creates the guarded action for one exact observation identity.
    #[must_use = "retain the guarded transcript-delete action"]
    pub fn new(observation_id: impl Into<String>) -> Self {
        Self {
            observation_id: observation_id.into(),
            guard: UnprocessedTranscriptGuard::new(),
        }
    }

    /// Borrows the exact identity carried by the delete predicate.
    #[must_use]
    pub fn observation_id(&self) -> &str {
        &self.observation_id
    }

    /// Returns the unprocessed-state fence carried by this action.
    pub const fn guard(&self) -> UnprocessedTranscriptGuard {
        self.guard
    }
}

/// The deterministic result of one terminal-transcript consume evaluation.
#[must_use = "a transcript-consumption decision must be enforced by its caller"]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum TerminalTranscriptConsumptionDecision {
    /// No durable action is admitted.
    Noop,
    /// Delete only the exact still-unprocessed inbox observation.
    DeleteUnprocessed(ConditionalDeleteTranscriptAction),
}

impl TerminalTranscriptConsumptionDecision {
    /// Returns whether this evaluation emits no action.
    #[must_use]
    pub const fn is_noop(&self) -> bool {
        matches!(self, Self::Noop)
    }

    /// Borrows the guarded delete action, when one was admitted.
    #[must_use]
    pub const fn delete_action(&self) -> Option<&ConditionalDeleteTranscriptAction> {
        match self {
            Self::Noop => None,
            Self::DeleteUnprocessed(action) => Some(action),
        }
    }
}

/// Stateless entry point for terminal-transcript consumption.
#[must_use]
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct TerminalTranscriptConsumptionPolicy;

impl TerminalTranscriptConsumptionPolicy {
    /// Creates the stateless policy marker.
    pub const fn new() -> Self {
        Self
    }

    /// Decides the no-op or guarded-delete action for supplied durable facts.
    #[must_use = "enforce the terminal-transcript consumption decision"]
    pub fn decide(
        input: TerminalTranscriptConsumptionInput,
    ) -> TerminalTranscriptConsumptionDecision {
        terminal_transcript_consumption_decision(input)
    }

    /// Source-shaped alias for [`Self::decide`].
    #[must_use = "enforce the terminal-transcript consumption decision"]
    pub fn consume(
        input: TerminalTranscriptConsumptionInput,
    ) -> TerminalTranscriptConsumptionDecision {
        Self::decide(input)
    }
}

/// Applies the oracle's terminal-transcript consumption decision to typed facts.
///
/// A missing or already-processed inbox observation, a missing root, and every
/// active root status return [`TerminalTranscriptConsumptionDecision::Noop`].
/// For a settled root, any exact three-field binding preserves the row for
/// normal child projection. Only a settled root with no exact binding emits a
/// delete action fenced by the supplied observation identity and an
/// unprocessed-state guard.
#[must_use = "enforce the terminal-transcript consumption decision"]
pub fn terminal_transcript_consumption_decision(
    input: TerminalTranscriptConsumptionInput,
) -> TerminalTranscriptConsumptionDecision {
    let TerminalTranscriptConsumptionInput {
        observation_id,
        inbox,
        root_run,
        bindings,
    } = input;

    let TerminalTranscriptInboxObservation::Pending(pending) = inbox else {
        return TerminalTranscriptConsumptionDecision::Noop;
    };

    let Some(root_run) = root_run else {
        return TerminalTranscriptConsumptionDecision::Noop;
    };

    if root_run.status.is_active() {
        return TerminalTranscriptConsumptionDecision::Noop;
    }

    if bindings.iter().any(|binding| binding.matches(&pending)) {
        return TerminalTranscriptConsumptionDecision::Noop;
    }

    TerminalTranscriptConsumptionDecision::DeleteUnprocessed(
        ConditionalDeleteTranscriptAction::new(observation_id),
    )
}
