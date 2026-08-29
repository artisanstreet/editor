//! Pure acknowledgement policy for a submitted conversation steer.
//!
//! This is the dependency-free Rust counterpart of
//! `modules/frontend/src/lib/conversation/steering.ts`. The small input
//! structs intentionally contain only the fields this predicate reads; they
//! are not protocol, transport, store, or renderer types.
//!
//! A matching user message is acknowledged immediately when it has no run.
//! For a bound run, all of that run's turns must be present and settled before
//! the run itself can acknowledge the steer. If that has not happened, any
//! later non-user item acknowledges it by durable ordinal, even when the item
//! belongs to another run. Later user messages never count as engine work.

#![allow(clippy::module_name_repetitions)]

/// A source identity attached to a projected conversation item.
///
/// The canonical `reference` and optional `event_id` are the only source
/// fields used by the steering policy. Provider and journal metadata remain
/// outside this pure leaf.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ConversationSourceRef {
    /// Canonical source reference.
    pub reference: String,
    /// Optional event identity accepted as an alias for `reference`.
    pub event_id: Option<String>,
}

impl ConversationSourceRef {
    /// Creates a source reference without an event-id alias.
    #[must_use]
    pub fn new(reference: impl Into<String>) -> Self {
        Self {
            reference: reference.into(),
            event_id: None,
        }
    }

    /// Creates a source reference with an event-id alias.
    #[must_use]
    pub fn with_event_id(reference: impl Into<String>, event_id: impl Into<String>) -> Self {
        Self {
            reference: reference.into(),
            event_id: Some(event_id.into()),
        }
    }
}

/// The only item-kind distinction needed by the acknowledgement policy.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ConversationSteeringItemKind {
    /// A durable user-authored message.
    UserMessage,
    /// Any projected item that is not a user-authored message.
    Other,
}

/// The renderer-visible lifecycle vocabulary used by conversation turns.
///
/// The acknowledgement policy deliberately treats exactly four variants as
/// settled. In particular, `interrupted` is settled for acknowledgement even
/// though it can be resumed by an explicit later action.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ConversationLifecycle {
    /// Work has not started.
    Pending,
    /// Incremental output is arriving.
    Streaming,
    /// Work is actively progressing.
    Active,
    /// Work is waiting for input or another dependency.
    Waiting,
    /// Work completed successfully.
    Completed,
    /// Work ended because of a failure.
    Failed,
    /// Work was externally interrupted.
    Interrupted,
    /// Work was deliberately cancelled.
    Cancelled,
}

impl ConversationLifecycle {
    /// Whether this lifecycle belongs to the exact acknowledgement set.
    #[must_use]
    pub const fn is_settled(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Interrupted | Self::Cancelled
        )
    }
}

/// The minimal projected item consumed by the steering acknowledgement
/// predicate.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ConversationSteeringItem {
    /// Stable item identity retained to mirror the source shape.
    pub id: String,
    /// Durable conversation position.
    pub ordinal: u64,
    /// Run that owns this item, when one has been bound.
    pub run_id: Option<String>,
    /// Source identities attached to this item.
    pub source_refs: Vec<ConversationSourceRef>,
    /// Whether this item is a user message or another projected item.
    pub kind: ConversationSteeringItemKind,
}

impl ConversationSteeringItem {
    /// Creates an item with the complete minimal policy shape.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        ordinal: u64,
        run_id: Option<String>,
        source_refs: Vec<ConversationSourceRef>,
        kind: ConversationSteeringItemKind,
    ) -> Self {
        Self {
            id: id.into(),
            ordinal,
            run_id,
            source_refs,
            kind,
        }
    }

    /// Creates a user-message item.
    #[must_use]
    pub fn user_message(
        id: impl Into<String>,
        ordinal: u64,
        run_id: Option<String>,
        source_refs: Vec<ConversationSourceRef>,
    ) -> Self {
        Self::new(
            id,
            ordinal,
            run_id,
            source_refs,
            ConversationSteeringItemKind::UserMessage,
        )
    }

    /// Creates a non-user item without a run or source identities.
    #[must_use]
    pub fn other(id: impl Into<String>, ordinal: u64) -> Self {
        Self::new(
            id,
            ordinal,
            None,
            Vec::new(),
            ConversationSteeringItemKind::Other,
        )
    }

    /// Creates a non-user item associated with a run.
    #[must_use]
    pub fn other_for_run(id: impl Into<String>, ordinal: u64, run_id: impl Into<String>) -> Self {
        Self::new(
            id,
            ordinal,
            Some(run_id.into()),
            Vec::new(),
            ConversationSteeringItemKind::Other,
        )
    }

    /// Whether this item is a user-authored message.
    #[must_use]
    pub const fn is_user_message(&self) -> bool {
        matches!(self.kind, ConversationSteeringItemKind::UserMessage)
    }

    /// Whether one source reference or event id matches `source_reference`.
    #[must_use]
    fn has_source_reference(&self, source_reference: &str) -> bool {
        self.source_refs.iter().any(|source| {
            source.reference == source_reference
                || source.event_id.as_deref() == Some(source_reference)
        })
    }
}

/// The minimal run-owned turn state consumed by the acknowledgement policy.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ConversationSteeringTurn {
    /// Run associated with this turn, when the projection supplies one.
    pub run_id: Option<String>,
    /// Current renderer-visible lifecycle.
    pub lifecycle: ConversationLifecycle,
}

impl ConversationSteeringTurn {
    /// Creates a turn with an optional run identity.
    #[must_use]
    pub fn new(run_id: Option<String>, lifecycle: ConversationLifecycle) -> Self {
        Self { run_id, lifecycle }
    }

    /// Creates a turn bound to `run_id`.
    #[must_use]
    pub fn for_run(run_id: impl Into<String>, lifecycle: ConversationLifecycle) -> Self {
        Self::new(Some(run_id.into()), lifecycle)
    }

    /// Creates a turn without a run identity.
    #[must_use]
    pub const fn unbound(lifecycle: ConversationLifecycle) -> Self {
        Self {
            run_id: None,
            lifecycle,
        }
    }
}

/// Reports whether the engine has acknowledged a steering message.
///
/// The first matching user item in `items` is the steer under evaluation,
/// matching the source policy's `find` semantics. An unbound steer is
/// acknowledged immediately. A bound steer is acknowledged when at least one
/// turn for its run exists and every such turn has one of the exact settled
/// lifecycles: completed, failed, interrupted, or cancelled. Otherwise, a
/// later non-user item acknowledges it regardless of that item's run; the
/// comparison is strict and uses the durable item ordinals supplied by the
/// caller.
#[must_use]
pub fn conversation_steering_acknowledged(
    items: &[ConversationSteeringItem],
    turns: &[ConversationSteeringTurn],
    source_reference: &str,
) -> bool {
    let Some(steer) = items
        .iter()
        .find(|item| item.is_user_message() && item.has_source_reference(source_reference))
    else {
        return false;
    };

    let Some(run_id) = steer.run_id.as_deref() else {
        return true;
    };

    let mut has_run_turn = false;
    let mut every_run_turn_settled = true;
    for turn in turns {
        if turn.run_id.as_deref() == Some(run_id) {
            has_run_turn = true;
            every_run_turn_settled &= turn.lifecycle.is_settled();
        }
    }
    if has_run_turn && every_run_turn_settled {
        return true;
    }

    items
        .iter()
        .any(|item| item.ordinal > steer.ordinal && !item.is_user_message())
}
