//! Pure conversation-plan selection for the thread checklist.
//!
//! This is the dependency-free portion of
//! `modules/frontend/src/lib/conversation/checklist.ts`. It deliberately
//! stops at the data needed to answer two questions:
//!
//! - does a plan still contain work that is pending or active?; and
//! - which plan is the latest eligible plan for a live owning turn?
//!
//! Effect services, leases, streams, refs, publication locking, and rendering
//! remain outside this module. Selection borrows its inputs and never sorts or
//! otherwise mutates them.

/// The state of one entry in a conversation plan.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ConversationPlanEntryState {
    /// The work has not started.
    Pending,
    /// The work is currently being performed.
    Active,
    /// The work finished successfully.
    Completed,
    /// The work was intentionally skipped.
    Skipped,
}

/// The lifecycle of a canonical conversation turn.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ConversationLifecycle {
    /// The turn exists but work has not started.
    Pending,
    /// The turn is receiving incremental output.
    Streaming,
    /// The turn is actively progressing.
    Active,
    /// The turn is waiting for input or another dependency.
    Waiting,
    /// The turn completed successfully.
    Completed,
    /// The turn failed.
    Failed,
    /// The turn was stopped externally.
    Interrupted,
    /// The turn was deliberately cancelled.
    Cancelled,
}

impl ConversationLifecycle {
    /// Returns whether this lifecycle keeps a plan eligible for the checklist.
    #[must_use]
    pub const fn is_checklist_eligible(self) -> bool {
        matches!(
            self,
            Self::Pending | Self::Streaming | Self::Active | Self::Waiting
        )
    }
}

/// The state of a conversation plan itself.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ConversationPlanState {
    /// The plan is still being drafted.
    Draft,
    /// The plan is the currently executing plan.
    Active,
    /// The plan finished all of its work.
    Completed,
    /// The plan was abandoned before completion.
    Abandoned,
}

/// The minimal entry representation needed by the checklist policy.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ConversationPlanEntry {
    /// Current work state for this entry.
    pub state: ConversationPlanEntryState,
}

impl ConversationPlanEntry {
    /// Creates an entry with the supplied state.
    #[must_use]
    pub const fn new(state: ConversationPlanEntryState) -> Self {
        Self { state }
    }
}

/// The minimal plan representation needed by the checklist policy.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ConversationPlan {
    /// Stable plan identity used for the second selection key.
    pub id: String,
    /// Identity of the turn that owns this plan.
    pub turn_id: String,
    /// Position of this plan in conversation order.
    pub ordinal: u64,
    /// Revision of this plan at its identity and ordinal.
    pub revision: u64,
    /// Current lifecycle/state of the plan.
    pub state: ConversationPlanState,
    /// Entries that make up the plan.
    pub entries: Vec<ConversationPlanEntry>,
}

impl ConversationPlan {
    /// Creates a minimal plan record.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        turn_id: impl Into<String>,
        ordinal: u64,
        revision: u64,
        state: ConversationPlanState,
        entries: Vec<ConversationPlanEntry>,
    ) -> Self {
        Self {
            id: id.into(),
            turn_id: turn_id.into(),
            ordinal,
            revision,
            state,
            entries,
        }
    }

    /// Returns whether any entry is still pending or active.
    #[must_use]
    pub fn has_open_entries(&self) -> bool {
        self.entries.iter().any(|entry| {
            matches!(
                entry.state,
                ConversationPlanEntryState::Pending | ConversationPlanEntryState::Active
            )
        })
    }
}

/// The minimal turn representation needed to validate a plan owner.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ConversationTurn {
    /// Stable turn identity.
    pub id: String,
    /// Current turn lifecycle.
    pub lifecycle: ConversationLifecycle,
}

impl ConversationTurn {
    /// Creates a minimal turn record.
    #[must_use]
    pub fn new(id: impl Into<String>, lifecycle: ConversationLifecycle) -> Self {
        Self {
            id: id.into(),
            lifecycle,
        }
    }
}

/// The item projection needed to distinguish plans from all other items.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum ConversationItem {
    /// A plan candidate for checklist selection.
    Plan(ConversationPlan),
    /// Any non-plan conversation item.
    Other,
}

/// Returns whether a plan has any pending or active entry.
///
/// Plan state is intentionally not consulted here. This mirrors the
/// TypeScript predicate, which asks only about entry state; callers that need
/// an eligible checklist plan also use [`latest_conversation_plan`], whose
/// separate policy requires an active plan and an eligible owning turn.
#[must_use]
pub fn conversation_plan_has_open_entries(plan: &ConversationPlan) -> bool {
    plan.has_open_entries()
}

/// Selects the canonical latest eligible plan without mutating either input.
///
/// Plan candidates are compared in this order:
///
/// 1. highest ordinal;
/// 2. lexicographically highest ID when ordinals match; and
/// 3. highest revision when both ID and ordinal match.
///
/// An equal key keeps the first candidate, matching the source loop's stable
/// behavior. After selection, the plan must be `Active`, and the first turn
/// with the matching ID must have a `Pending`, `Streaming`, `Active`, or
/// `Waiting` lifecycle. A missing owner or any other lifecycle returns `None`.
#[must_use]
pub fn latest_conversation_plan<'a>(
    items: &'a [ConversationItem],
    turns: &[ConversationTurn],
) -> Option<&'a ConversationPlan> {
    let mut latest: Option<&ConversationPlan> = None;

    for item in items {
        let ConversationItem::Plan(candidate) = item else {
            continue;
        };

        if latest.is_none_or(|current| is_newer(candidate, current)) {
            latest = Some(candidate);
        }
    }

    let latest = latest?;
    if latest.state != ConversationPlanState::Active {
        return None;
    }

    turns
        .iter()
        .find(|turn| turn.id == latest.turn_id)
        .filter(|turn| turn.lifecycle.is_checklist_eligible())
        .map(|_| latest)
}

fn is_newer(candidate: &ConversationPlan, current: &ConversationPlan) -> bool {
    candidate.ordinal > current.ordinal
        || (candidate.ordinal == current.ordinal
            && (candidate.id > current.id
                || (candidate.id == current.id && candidate.revision > current.revision)))
}
