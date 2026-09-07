//! Pure route-aware tracking of root-visible thread activity.
//!
//! This is the native counterpart of
//! `modules/frontend/src/lib/root/thread-read-tracker.ts`. It remembers the
//! thread and activity stamp that were observed on the current route, then
//! emits one acknowledgement only when a real route departure makes that
//! previously observed activity eligible.
//!
//! The existing Rust [`artisan_domain::ThreadSummary`] deliberately does not
//! carry reader cursors or run authority, and importing the full protocol
//! `ThreadListItem` would add unrelated fields to this pure leaf. The owned
//! [`ThreadReadSnapshot`] therefore contains exactly the four facts this
//! transition needs: thread identity, root-visible reader activity, durable
//! acknowledgement, and active-work state. The module has no runtime or
//! serialization dependency, so its transition can be exercised in a small
//! standalone Rust harness until shared frontend registration is added.

/// The minimal thread projection required by the read-tracking transition.
///
/// `reader_activity_at` is already the resolved root-visible value that the
/// legacy `ThreadReaderActivityAt` helper returns (`reader_activity_at` with
/// its `last_activity_at` fallback applied by the caller). Timestamp values
/// stay as owned strings so this pure port preserves their exact wire value;
/// no parsing or ordering is part of the legacy behavior.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadReadSnapshot {
    /// Forge-minted thread identity.
    pub thread_id: String,
    /// The activity cursor visible in the root conversation.
    pub reader_activity_at: String,
    /// The activity cursor most recently acknowledged by Forge, if any.
    pub reader_acknowledged_activity_at: Option<String>,
    /// Whether Forge still owns active work for this thread.
    pub has_active_work: bool,
}

impl ThreadReadSnapshot {
    /// Builds the minimal snapshot consumed by the transition.
    #[must_use]
    pub fn new(
        thread_id: impl Into<String>,
        reader_activity_at: impl Into<String>,
        reader_acknowledged_activity_at: Option<String>,
        has_active_work: bool,
    ) -> Self {
        Self {
            thread_id: thread_id.into(),
            reader_activity_at: reader_activity_at.into(),
            reader_acknowledged_activity_at,
            has_active_work,
        }
    }
}

/// State retained between route/root observations.
///
/// The state is intentionally a plain value: callers replace it with the
/// [`ThreadReadTrackingTransition::state`] returned by
/// [`advance_thread_read_tracking`]. A route change clears the observed
/// cursor even when the replacement route is hidden, while an unchanged route
/// may retain it when the root is not visible.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ThreadReadTrackingState {
    /// The last route identity supplied to the transition.
    pub route_id: Option<String>,
    /// The latest snapshot retained for the current route.
    pub thread: Option<ThreadReadSnapshot>,
    /// The activity cursor actually observed while the root was visible.
    pub observed_activity_at: Option<String>,
}

/// One root/route observation supplied to the transition.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadReadTrackingInput {
    /// Whether the root conversation is currently visible to the reader.
    pub root_visible: bool,
    /// The route identity after the observation, if a thread route exists.
    pub route_id: Option<String>,
    /// The resolved thread snapshot after the observation, if one exists.
    pub thread: Option<ThreadReadSnapshot>,
}

impl ThreadReadTrackingInput {
    /// Builds one route/root observation.
    #[must_use]
    pub fn new(
        root_visible: bool,
        route_id: Option<String>,
        thread: Option<ThreadReadSnapshot>,
    ) -> Self {
        Self {
            root_visible,
            route_id,
            thread,
        }
    }
}

/// The durable acknowledgement request emitted when a settled thread leaves.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadReadAcknowledgement {
    /// The exact root-visible activity cursor being acknowledged.
    pub reader_activity_at: String,
    /// The departed thread receiving the acknowledgement.
    pub thread_id: String,
}

/// Result of advancing one route/root observation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ThreadReadTrackingTransition {
    /// The one acknowledgement to submit, if the departed thread was eligible.
    pub acknowledgement: Option<ThreadReadAcknowledgement>,
    /// State to retain for the next observation.
    pub state: ThreadReadTrackingState,
}

/// Advances visible-thread tracking and acknowledges only a real route
/// departure.
///
/// This follows the legacy transition in branch order:
///
/// - an unchanged route merges an incoming thread over the retained thread;
/// - a visible root refreshes the observed cursor from that merged thread,
///   while a hidden root retains the previous cursor;
/// - a changed route replaces route/thread state and resets observation unless
///   the replacement thread is visible immediately;
/// - departure acknowledgement uses the *previously observed* cursor and is
///   suppressed for a missing thread/cursor, active work, or exact durable
///   acknowledgement.
///
/// No timestamp comparison is performed: acknowledgement eligibility depends
/// only on exact string equality, as in the TypeScript source.
#[must_use]
pub fn advance_thread_read_tracking(
    previous: &ThreadReadTrackingState,
    input: &ThreadReadTrackingInput,
) -> ThreadReadTrackingTransition {
    if previous.route_id == input.route_id {
        let thread = input.thread.clone().or_else(|| previous.thread.clone());
        let observed_activity_at = match (input.root_visible, thread.as_ref()) {
            (true, Some(thread)) => Some(thread.reader_activity_at.clone()),
            _ => previous.observed_activity_at.clone(),
        };

        return ThreadReadTrackingTransition {
            acknowledgement: None,
            state: ThreadReadTrackingState {
                route_id: previous.route_id.clone(),
                thread,
                observed_activity_at,
            },
        };
    }

    let acknowledgement = acknowledgement_for_departure(
        previous.thread.as_ref(),
        previous.observed_activity_at.as_deref(),
    );
    let state = ThreadReadTrackingState {
        route_id: input.route_id.clone(),
        thread: input.thread.clone(),
        observed_activity_at: if input.root_visible {
            input
                .thread
                .as_ref()
                .map(|thread| thread.reader_activity_at.clone())
        } else {
            None
        },
    };

    ThreadReadTrackingTransition {
        acknowledgement,
        state,
    }
}

fn acknowledgement_for_departure(
    departed: Option<&ThreadReadSnapshot>,
    observed_activity_at: Option<&str>,
) -> Option<ThreadReadAcknowledgement> {
    let departed = departed?;
    let observed_activity_at = observed_activity_at?;

    if departed.has_active_work
        || departed.reader_acknowledged_activity_at.as_deref() == Some(observed_activity_at)
    {
        return None;
    }

    Some(ThreadReadAcknowledgement {
        reader_activity_at: observed_activity_at.to_owned(),
        thread_id: departed.thread_id.clone(),
    })
}
