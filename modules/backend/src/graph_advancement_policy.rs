//! Dependency-free convergence and event-order policy for graph transitions.
//!
//! The TypeScript graph advancement boundary repeatedly evaluates dependency
//! gates and join gates until one wave is empty, then derives aggregate group
//! state once. This native leaf keeps that ordering rule independent of the
//! transaction, repository, effect runtime, event schema, and both evaluators.
//! Callers inject already-ordered event waves through the three callbacks and
//! retain ownership of their durable side effects.

#![allow(clippy::module_name_repetitions)]
#![allow(clippy::struct_field_names)]
#![forbid(unsafe_code)]

use std::fmt;

/// The identity carried by one graph transition.
///
/// These fields mirror the TypeScript `GraphTransitionInput`. The policy
/// treats every value as opaque and passes the same typed input to dependency,
/// join, and aggregate callbacks. No identifier is parsed, normalized, or
/// inferred here.
#[must_use]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct GraphTransitionInput {
    /// Identifier of the observation or command causing this transition.
    pub causation_id: String,
    /// Identifier grouping the events produced by this transition.
    pub correlation_id: String,
    /// Orchestration group whose graph is being advanced.
    pub group_id: String,
    /// Thread that owns the graph event stream.
    pub thread_id: String,
}

impl GraphTransitionInput {
    /// Creates a transition input while preserving all four supplied IDs.
    pub fn new(
        causation_id: impl Into<String>,
        correlation_id: impl Into<String>,
        group_id: impl Into<String>,
        thread_id: impl Into<String>,
    ) -> Self {
        Self {
            causation_id: causation_id.into(),
            correlation_id: correlation_id.into(),
            group_id: group_id.into(),
            thread_id: thread_id.into(),
        }
    }
}

/// Alias for callers that name the four transition IDs as an identity.
pub type GraphTransitionIdentity = GraphTransitionInput;

impl From<&GraphTransitionInput> for GraphTransitionInput {
    fn from(input: &GraphTransitionInput) -> Self {
        input.clone()
    }
}

/// Successful result of one bounded graph advancement.
///
/// `iterations` includes the empty terminal wave that established
/// convergence. `events` contains every dependency event, then every join
/// event, for each wave, followed by the aggregate events returned by the one
/// post-convergence callback.
#[must_use]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraphAdvancementResult<Event> {
    /// The exact typed transition input used by every injected callback.
    pub input: GraphTransitionInput,
    /// All events in wave order, including aggregate events at the end.
    pub events: Vec<Event>,
    /// Number of dependency/join evaluation waves, including the empty one.
    pub iterations: usize,
}

impl<Event> GraphAdvancementResult<Event> {
    /// Borrows the transition input retained by this result.
    pub const fn input(&self) -> &GraphTransitionInput {
        &self.input
    }

    /// Borrows the complete ordered event sequence.
    #[must_use]
    pub fn events(&self) -> &[Event] {
        &self.events
    }

    /// Returns the number of evaluation waves, including the terminal wave.
    #[must_use]
    pub const fn iterations(&self) -> usize {
        self.iterations
    }

    /// Consumes the result and returns the retained input and events together
    /// with the number of evaluation waves.
    pub fn into_parts(self) -> (GraphTransitionInput, Vec<Event>, usize) {
        (self.input, self.events, self.iterations)
    }
}

/// Failure produced when injected evaluators do not converge in the supplied
/// finite number of waves.
///
/// The partial event sequence is retained for diagnostics, but it is not a
/// successful TypeScript-style advancement: the aggregate callback is never
/// invoked for this outcome.
#[must_use]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GraphAdvancementError<Event> {
    /// No empty terminal wave was observed before the permitted waves were
    /// exhausted.
    IterationLimitExceeded {
        /// The exact transition input supplied to the attempted advancement.
        input: Box<GraphTransitionInput>,
        /// Events produced before the finite bound was exhausted.
        events: Vec<Event>,
        /// Number of dependency/join waves that were evaluated.
        iterations: usize,
        /// Caller-supplied maximum number of dependency/join waves.
        max_iterations: usize,
    },
}

impl<Event> GraphAdvancementError<Event> {
    /// Returns the input retained by a non-converged attempt.
    pub fn input(&self) -> &GraphTransitionInput {
        match self {
            Self::IterationLimitExceeded { input, .. } => input,
        }
    }

    /// Borrows events produced before the finite bound was exhausted.
    #[must_use]
    pub fn events(&self) -> &[Event] {
        match self {
            Self::IterationLimitExceeded { events, .. } => events,
        }
    }

    /// Returns the number of waves evaluated before failure.
    #[must_use]
    pub const fn iterations(&self) -> usize {
        match self {
            Self::IterationLimitExceeded { iterations, .. } => *iterations,
        }
    }

    /// Returns the caller-supplied finite wave bound.
    #[must_use]
    pub const fn max_iterations(&self) -> usize {
        match self {
            Self::IterationLimitExceeded { max_iterations, .. } => *max_iterations,
        }
    }

    /// Consumes the failure and returns its retained input, events, and bound
    /// accounting.
    pub fn into_parts(self) -> (GraphTransitionInput, Vec<Event>, usize, usize) {
        match self {
            Self::IterationLimitExceeded {
                input,
                events,
                iterations,
                max_iterations,
            } => (*input, events, iterations, max_iterations),
        }
    }
}

impl<Event> fmt::Display for GraphAdvancementError<Event> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IterationLimitExceeded {
                iterations,
                max_iterations,
                ..
            } => write!(
                formatter,
                "graph advancement did not converge in {iterations} of {max_iterations} waves"
            ),
        }
    }
}

impl<Event: fmt::Debug> std::error::Error for GraphAdvancementError<Event> {}

/// Stateless entry point for the graph advancement policy.
///
/// The policy owns no graph state. It only makes the deterministic ordering
/// decision for one caller-supplied transition.
#[must_use]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct GraphAdvancementPolicy;

impl GraphAdvancementPolicy {
    /// Creates the stateless graph advancement policy.
    pub const fn new() -> Self {
        Self
    }

    /// Advances one graph using injected dependency, join, and aggregate
    /// callbacks.
    ///
    /// Each callback is invoked with an immutable reference to the exact
    /// [`GraphTransitionInput`] retained by the result. One iteration invokes
    /// the dependency callback first and the join callback second. A wave is
    /// terminal only when both return empty vectors; that terminal wave counts
    /// against `max_iterations` and is followed by exactly one aggregate
    /// callback invocation. A zero bound fails without invoking any callback.
    ///
    /// # Errors
    ///
    /// Returns [`GraphAdvancementError::IterationLimitExceeded`] when the
    /// finite bound is exhausted before an empty terminal wave. The error
    /// retains all events produced so far and never invokes the aggregate
    /// callback.
    pub fn advance<Event, DependencyEvaluator, JoinEvaluator, GroupUpdater>(
        input: impl Into<GraphTransitionInput>,
        max_iterations: usize,
        evaluate_dependencies: DependencyEvaluator,
        resolve_joins: JoinEvaluator,
        update_group_state: GroupUpdater,
    ) -> Result<GraphAdvancementResult<Event>, GraphAdvancementError<Event>>
    where
        DependencyEvaluator: FnMut(&GraphTransitionInput) -> Vec<Event>,
        JoinEvaluator: FnMut(&GraphTransitionInput) -> Vec<Event>,
        GroupUpdater: FnMut(&GraphTransitionInput) -> Vec<Event>,
    {
        advance_graph(
            input,
            max_iterations,
            evaluate_dependencies,
            resolve_joins,
            update_group_state,
        )
    }

    /// Static spelling of [`Self::advance`] for callers that do not retain a
    /// policy value.
    pub fn advance_graph<Event, DependencyEvaluator, JoinEvaluator, GroupUpdater>(
        input: impl Into<GraphTransitionInput>,
        max_iterations: usize,
        evaluate_dependencies: DependencyEvaluator,
        resolve_joins: JoinEvaluator,
        update_group_state: GroupUpdater,
    ) -> Result<GraphAdvancementResult<Event>, GraphAdvancementError<Event>>
    where
        DependencyEvaluator: FnMut(&GraphTransitionInput) -> Vec<Event>,
        JoinEvaluator: FnMut(&GraphTransitionInput) -> Vec<Event>,
        GroupUpdater: FnMut(&GraphTransitionInput) -> Vec<Event>,
    {
        advance_graph(
            input,
            max_iterations,
            evaluate_dependencies,
            resolve_joins,
            update_group_state,
        )
    }
}

/// Converges injected graph evaluators and appends aggregate events once.
///
/// This is the dependency-free counterpart of the TypeScript
/// `GraphAdvancement.advance_graph` ordering. The callbacks return already
/// ordered event vectors; this function does not inspect or reinterpret their
/// values.
///
/// # Errors
///
/// Returns [`GraphAdvancementError::IterationLimitExceeded`] if every one of
/// the `max_iterations` permitted waves contains at least one event. In that
/// case all partial events are retained in the error and `update_group_state`
/// is not called.
#[must_use = "handle the graph advancement result"]
pub fn advance_graph<Event, DependencyEvaluator, JoinEvaluator, GroupUpdater>(
    input: impl Into<GraphTransitionInput>,
    max_iterations: usize,
    mut evaluate_dependencies: DependencyEvaluator,
    mut resolve_joins: JoinEvaluator,
    mut update_group_state: GroupUpdater,
) -> Result<GraphAdvancementResult<Event>, GraphAdvancementError<Event>>
where
    DependencyEvaluator: FnMut(&GraphTransitionInput) -> Vec<Event>,
    JoinEvaluator: FnMut(&GraphTransitionInput) -> Vec<Event>,
    GroupUpdater: FnMut(&GraphTransitionInput) -> Vec<Event>,
{
    let input = input.into();
    let mut events = Vec::new();

    for iteration in 0..max_iterations {
        let dependency_events = evaluate_dependencies(&input);
        let join_events = resolve_joins(&input);
        let converged = dependency_events.is_empty() && join_events.is_empty();

        events.extend(dependency_events);
        events.extend(join_events);

        if converged {
            events.extend(update_group_state(&input));
            return Ok(GraphAdvancementResult {
                input,
                events,
                iterations: iteration + 1,
            });
        }
    }

    Err(GraphAdvancementError::IterationLimitExceeded {
        input: Box::new(input),
        events,
        iterations: max_iterations,
        max_iterations,
    })
}
