//! Dependency-free coordination and failure classification for thread quiescence.
//!
//! The TypeScript quiescer asks the agent graph, agent orchestration, terminal
//! sessions, and preview coordinator to quiesce one thread. The requests are
//! intentionally unbounded-concurrent and their successful `void` values are
//! discarded. This module keeps only the typed boundary around that operation:
//! it emits the four exact requests and classifies completion observations that
//! a caller supplies after dispatching them. It does not dispatch, await, stop,
//! start, or erase anything.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

/// Number of resources participating in every thread-quiescence request.
pub const THREAD_RESOURCE_QUIESCE_PARTICIPANT_COUNT: usize = 4;

/// One of the four services asked to quiesce a thread.
///
/// The variants and [`Self::ALL`] order mirror the source `Effect.all` array:
/// agent graph, agent orchestration, terminal sessions, and preview
/// coordinator. The order is an identity/diagnostic order only. It is not an
/// observation of completion order, because the source requests are
/// unbounded-concurrent and expose no such order.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ThreadResourceParticipant {
    /// The `AgentGraphOrchestrator` service.
    AgentGraph,
    /// The `AgentOrchestrator` service.
    AgentOrchestration,
    /// The `TerminalSessionService` service.
    TerminalSessions,
    /// The `PreviewCoordinator` service.
    PreviewCoordinator,
}

impl ThreadResourceParticipant {
    /// The four participants in the exact source declaration order.
    pub const ALL: [Self; THREAD_RESOURCE_QUIESCE_PARTICIPANT_COUNT] = [
        Self::AgentGraph,
        Self::AgentOrchestration,
        Self::TerminalSessions,
        Self::PreviewCoordinator,
    ];

    /// Returns this participant's stable position in [`Self::ALL`].
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::AgentGraph => 0,
            Self::AgentOrchestration => 1,
            Self::TerminalSessions => 2,
            Self::PreviewCoordinator => 3,
        }
    }

    /// Returns a short stable diagnostic label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::AgentGraph => "agent graph",
            Self::AgentOrchestration => "agent orchestration",
            Self::TerminalSessions => "terminal sessions",
            Self::PreviewCoordinator => "preview coordinator",
        }
    }

    /// Returns the exact source service name represented by this participant.
    #[must_use]
    pub const fn service_name(self) -> &'static str {
        match self {
            Self::AgentGraph => "AgentGraphOrchestrator",
            Self::AgentOrchestration => "AgentOrchestrator",
            Self::TerminalSessions => "TerminalSessionService",
            Self::PreviewCoordinator => "PreviewCoordinator",
        }
    }
}

/// Public module-level spelling of the exact participant set and order.
pub const THREAD_RESOURCE_QUIESCE_PARTICIPANTS: [ThreadResourceParticipant;
    THREAD_RESOURCE_QUIESCE_PARTICIPANT_COUNT] = ThreadResourceParticipant::ALL;

/// One exact request handed to a participant adapter.
///
/// The request owns its thread identifier so an adapter cannot accidentally
/// borrow a mutable or unrelated identifier while it performs its own
/// operation. [`ThreadResourceQuiescencePolicy::requests`] returns one value
/// for each participant, all carrying byte-for-byte the supplied identifier.
#[must_use = "a quiescence request must be dispatched or recorded"]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ThreadResourceQuiescenceRequest {
    /// The participant that receives this request.
    pub participant: ThreadResourceParticipant,
    /// The exact thread identifier supplied to the quiescence boundary.
    pub thread_id: String,
}

impl ThreadResourceQuiescenceRequest {
    /// Creates a request without validating, trimming, or normalizing its ID.
    #[must_use = "the request must be dispatched or recorded"]
    pub fn new(participant: ThreadResourceParticipant, thread_id: impl Into<String>) -> Self {
        Self {
            participant,
            thread_id: thread_id.into(),
        }
    }

    /// Returns the participant selected by this request.
    #[must_use]
    pub const fn participant(&self) -> ThreadResourceParticipant {
        self.participant
    }

    /// Borrows the exact thread identifier carried by this request.
    #[must_use]
    pub fn thread_id(&self) -> &str {
        &self.thread_id
    }
}

/// The typed completion observation supplied for one participant.
///
/// `Success` is deliberately generic because an adapter may observe a value
/// while the source quiescer's successful result remains `void`. The policy
/// drops that value. `Failure` is also generic so the caller can preserve its
/// own typed fact without converting an opaque TypeScript cause into text.
#[must_use = "a participant completion must be classified"]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ThreadResourceParticipantCompletion<Success = (), Failure = String> {
    /// The participant completed successfully; the contained value is dropped
    /// by aggregate classification.
    Succeeded(Success),
    /// The participant failed with the caller-supplied typed fact.
    Failed(Failure),
}

impl<Success, Failure> ThreadResourceParticipantCompletion<Success, Failure> {
    /// Wraps one successful participant completion.
    #[must_use = "the participant completion must be classified"]
    pub const fn succeeded(value: Success) -> Self {
        Self::Succeeded(value)
    }

    /// Wraps one failed participant completion while retaining its fact.
    #[must_use = "the participant completion must be classified"]
    pub const fn failed(fact: Failure) -> Self {
        Self::Failed(fact)
    }
}

/// Four typed participant completion observations in fixed source order.
///
/// The shape makes omission and duplicate participant identities
/// unrepresentable at this boundary: one completion is supplied for each
/// named service. The field order is the source array order, not a claim about
/// which unbounded-concurrent effect settled first.
#[must_use = "all participant completions must be classified"]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadResourceQuiescenceCompletions<Success = (), Failure = String> {
    /// Completion observed from the agent graph participant.
    pub agent_graph: ThreadResourceParticipantCompletion<Success, Failure>,
    /// Completion observed from the agent orchestration participant.
    pub agent_orchestration: ThreadResourceParticipantCompletion<Success, Failure>,
    /// Completion observed from the terminal sessions participant.
    pub terminal_sessions: ThreadResourceParticipantCompletion<Success, Failure>,
    /// Completion observed from the preview coordinator participant.
    pub preview_coordinator: ThreadResourceParticipantCompletion<Success, Failure>,
}

impl<Success, Failure> ThreadResourceQuiescenceCompletions<Success, Failure> {
    /// Creates all four observations in the exact source declaration order.
    #[must_use = "all participant completions must be classified"]
    pub const fn new(
        agent_graph: ThreadResourceParticipantCompletion<Success, Failure>,
        agent_orchestration: ThreadResourceParticipantCompletion<Success, Failure>,
        terminal_sessions: ThreadResourceParticipantCompletion<Success, Failure>,
        preview_coordinator: ThreadResourceParticipantCompletion<Success, Failure>,
    ) -> Self {
        Self {
            agent_graph,
            agent_orchestration,
            terminal_sessions,
            preview_coordinator,
        }
    }

    /// Moves the observations into the exact participant identity order.
    ///
    /// This order is deterministic for diagnostics. It must not be read as a
    /// completion timeline: the source uses unbounded concurrency and does not
    /// expose one.
    #[must_use = "the ordered participant completions must be classified"]
    pub fn into_ordered(
        self,
    ) -> [(
        ThreadResourceParticipant,
        ThreadResourceParticipantCompletion<Success, Failure>,
    ); THREAD_RESOURCE_QUIESCE_PARTICIPANT_COUNT] {
        [
            (ThreadResourceParticipant::AgentGraph, self.agent_graph),
            (
                ThreadResourceParticipant::AgentOrchestration,
                self.agent_orchestration,
            ),
            (
                ThreadResourceParticipant::TerminalSessions,
                self.terminal_sessions,
            ),
            (
                ThreadResourceParticipant::PreviewCoordinator,
                self.preview_coordinator,
            ),
        ]
    }
}

/// All input supplied to one aggregate quiescence classification.
#[must_use = "a quiescence input must be classified"]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadResourceQuiescenceInput<Success = (), Failure = String> {
    /// The exact thread identifier requested from all four participants.
    pub thread_id: String,
    /// The four participant completion observations.
    pub completions: ThreadResourceQuiescenceCompletions<Success, Failure>,
}

impl<Success, Failure> ThreadResourceQuiescenceInput<Success, Failure> {
    /// Creates an input without changing the supplied thread identifier.
    #[must_use = "the quiescence input must be classified"]
    pub fn new(
        thread_id: impl Into<String>,
        completions: ThreadResourceQuiescenceCompletions<Success, Failure>,
    ) -> Self {
        Self {
            thread_id: thread_id.into(),
            completions,
        }
    }
}

/// One participant failure retained by an aggregate quiescence failure.
#[must_use = "a participant failure fact must be handled or returned"]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadResourceParticipantFailure<Failure = String> {
    /// The participant whose completion failed.
    pub participant: ThreadResourceParticipant,
    /// The exact typed failure fact supplied by that participant adapter.
    pub fact: Failure,
}

impl<Failure> ThreadResourceParticipantFailure<Failure> {
    /// Creates a participant-tagged failure fact.
    #[must_use = "the participant failure fact must be handled or returned"]
    pub const fn new(participant: ThreadResourceParticipant, fact: Failure) -> Self {
        Self { participant, fact }
    }
}

/// The single aggregate failure returned when one or more participants fail.
///
/// The `failures` collection is ordered by the fixed participant identity
/// order in [`ThreadResourceParticipant::ALL`]. It intentionally does not
/// claim to preserve completion order, which the source's unbounded
/// concurrency does not expose. Successful completion values are not present
/// in this value.
#[must_use = "a thread quiescence failure must be handled or returned"]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadResourceQuiescenceFailure<Failure = String> {
    /// The exact thread identifier associated with the failed classification.
    pub thread_id: String,
    /// Every failed participant, in canonical participant identity order.
    pub failures: Vec<ThreadResourceParticipantFailure<Failure>>,
}

impl<Failure> ThreadResourceQuiescenceFailure<Failure> {
    /// Borrows all retained failure facts in canonical participant order.
    #[must_use = "the retained failure facts must be handled"]
    pub fn failures(&self) -> &[ThreadResourceParticipantFailure<Failure>] {
        &self.failures
    }

    /// Returns the number of participants whose completion failed.
    #[must_use]
    pub fn failure_count(&self) -> usize {
        self.failures.len()
    }
}

/// Stateless policy for planning and classifying thread-resource quiescence.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ThreadResourceQuiescencePolicy;

impl ThreadResourceQuiescencePolicy {
    /// Creates the stateless quiescence policy.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }

    /// Emits exactly one request per participant for the exact supplied ID.
    ///
    /// The returned array is a request-set description. A caller that owns the
    /// four service adapters may submit all four requests with unbounded
    /// concurrency and then pass their four observations to [`Self::classify`].
    /// This method itself performs no dispatch or asynchronous work.
    #[must_use = "the participant requests must be dispatched or recorded"]
    pub fn requests(
        thread_id: impl Into<String>,
    ) -> [ThreadResourceQuiescenceRequest; THREAD_RESOURCE_QUIESCE_PARTICIPANT_COUNT] {
        let thread_id = thread_id.into();
        ThreadResourceParticipant::ALL
            .map(|participant| ThreadResourceQuiescenceRequest::new(participant, thread_id.clone()))
    }

    /// Classifies all four supplied completion observations.
    ///
    /// Every field is inspected, including fields after a failure. `Ok(())`
    /// is returned only when all four participants succeeded; otherwise one
    /// aggregate failure retains every failed participant and its typed fact.
    /// Any successful values are consumed and discarded, matching the source
    /// `Effect.all(..., { discard: true })` result shape.
    ///
    /// # Errors
    ///
    /// Returns [`ThreadResourceQuiescenceFailure`] when at least one supplied
    /// participant completion is [`ThreadResourceParticipantCompletion::Failed`].
    #[must_use = "a quiescence classification must be handled"]
    pub fn classify<Success, Failure>(
        input: ThreadResourceQuiescenceInput<Success, Failure>,
    ) -> Result<(), ThreadResourceQuiescenceFailure<Failure>> {
        let ThreadResourceQuiescenceInput {
            thread_id,
            completions,
        } = input;
        let mut failures = Vec::with_capacity(THREAD_RESOURCE_QUIESCE_PARTICIPANT_COUNT);

        for (participant, completion) in completions.into_ordered() {
            match completion {
                ThreadResourceParticipantCompletion::Succeeded(_value) => {}
                ThreadResourceParticipantCompletion::Failed(fact) => {
                    failures.push(ThreadResourceParticipantFailure::new(participant, fact));
                }
            }
        }

        if failures.is_empty() {
            Ok(())
        } else {
            Err(ThreadResourceQuiescenceFailure {
                thread_id,
                failures,
            })
        }
    }

    /// Alias for [`Self::classify`] using the source operation's verb.
    ///
    /// # Errors
    ///
    /// Returns the same aggregate failure as [`Self::classify`].
    #[must_use = "a quiescence result must be handled"]
    pub fn quiesce<Success, Failure>(
        input: ThreadResourceQuiescenceInput<Success, Failure>,
    ) -> Result<(), ThreadResourceQuiescenceFailure<Failure>> {
        Self::classify(input)
    }
}

/// Emits the four exact participant requests for one thread.
#[must_use = "the participant requests must be dispatched or recorded"]
pub fn participant_requests(
    thread_id: impl Into<String>,
) -> [ThreadResourceQuiescenceRequest; THREAD_RESOURCE_QUIESCE_PARTICIPANT_COUNT] {
    ThreadResourceQuiescencePolicy::requests(thread_id)
}

/// Classifies four supplied participant completion observations.
///
/// # Errors
///
/// Returns [`ThreadResourceQuiescenceFailure`] when one or more supplied
/// participant completions failed.
#[must_use = "a quiescence classification must be handled"]
pub fn classify_thread_resource_quiescence<Success, Failure>(
    input: ThreadResourceQuiescenceInput<Success, Failure>,
) -> Result<(), ThreadResourceQuiescenceFailure<Failure>> {
    ThreadResourceQuiescencePolicy::classify(input)
}
