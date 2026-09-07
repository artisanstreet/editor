//! Pure durable-event notification policy.
//!
//! This is the native equivalent of
//! `modules/frontend/src/lib/notifications/events.ts`. It intentionally
//! models only the event fields the policy reads and returns an owned
//! notification value for the host-facing layer to carry out. There is no
//! host, protocol, service, or runtime dependency here.

/// The lifecycle states that can be carried by a durable run event.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RunLifecycleState {
    /// The run is queued but has not started.
    Queued,
    /// The run is actively executing.
    Running,
    /// The run is waiting for an external condition.
    Waiting,
    /// The run was interrupted.
    Interrupted,
    /// The run completed successfully.
    Completed,
    /// The run was cancelled.
    Cancelled,
    /// The run failed.
    Failed,
    /// The run was closed after its lifecycle completed.
    Closed,
}

/// The two states shared by approval and question interactions.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum InteractionState {
    /// The interaction is waiting for the reader.
    Requested,
    /// The interaction has been answered or otherwise resolved.
    Resolved,
}

/// The minimal durable payload surface consumed by notification policy.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EventPayload<'a> {
    /// A change in the lifecycle of a run.
    RunLifecycle {
        /// The new lifecycle state.
        state: RunLifecycleState,
    },
    /// An approval request or its resolution.
    ApprovalInteraction {
        /// The durable approval identity.
        approval_id: &'a str,
        /// The provider-authored description shown in the notification.
        description: &'a str,
        /// Whether the approval is waiting or resolved.
        state: InteractionState,
    },
    /// A question request or its resolution.
    QuestionInteraction {
        /// The durable question identity.
        question_id: &'a str,
        /// The provider-authored question shown in the notification.
        text: &'a str,
        /// Whether the question is waiting or resolved.
        state: InteractionState,
    },
    /// Any durable event family that notification policy ignores.
    Irrelevant,
}

/// The envelope fields needed to decide a notification action.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct EventEnvelope<'a> {
    /// The run identity, when the event belongs to a run.
    pub run_id: Option<&'a str>,
    /// The durable thread identity owning the event.
    pub thread_id: &'a str,
    /// The typed event payload.
    pub payload: EventPayload<'a>,
}

impl<'a> EventEnvelope<'a> {
    /// Creates an envelope without a separate run identity.
    #[must_use]
    pub const fn new(thread_id: &'a str, payload: EventPayload<'a>) -> Self {
        Self {
            run_id: None,
            thread_id,
            payload,
        }
    }

    /// Adds the optional run identity used by completed or failed run IDs.
    #[must_use]
    pub const fn with_run_id(mut self, run_id: Option<&'a str>) -> Self {
        self.run_id = run_id;
        self
    }
}

/// The renderer context used to decide whether a notification is redundant.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SystemNotificationContext<'a> {
    /// The thread whose transcript is currently on screen, if any.
    pub active_thread_id: Option<&'a str>,
    /// Whether the reader is looking at this window.
    pub focused: bool,
    /// The already-resolved route for the event's thread.
    pub route_path: &'a str,
    /// The thread title, when one exists.
    pub thread_title: Option<&'a str>,
}

impl<'a> SystemNotificationContext<'a> {
    /// Creates notification context without changing route or title values.
    #[must_use]
    pub const fn new(
        active_thread_id: Option<&'a str>,
        focused: bool,
        route_path: &'a str,
        thread_title: Option<&'a str>,
    ) -> Self {
        Self {
            active_thread_id,
            focused,
            route_path,
            thread_title,
        }
    }
}

/// The canonical category carried by a host notification.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SystemNotificationCategory {
    /// A pending approval interaction.
    Approval,
    /// A pending question interaction.
    Question,
    /// A successfully completed run.
    RunCompleted,
    /// A failed run.
    RunFailed,
}

impl SystemNotificationCategory {
    /// Returns the exact durable category value used by the TypeScript policy.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Approval => "approval",
            Self::Question => "question",
            Self::RunCompleted => "run_completed",
            Self::RunFailed => "run_failed",
        }
    }
}

/// One notification produced by the pure decision policy.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SystemNotification {
    /// The one-line body presented by the host.
    pub body: String,
    /// The semantic notification category.
    pub category: SystemNotificationCategory,
    /// The durable identity used for deduplication and revocation.
    pub id: String,
    /// The route opened when the notification is activated.
    pub route_path: String,
    /// The title presented by the host.
    pub title: String,
}

/// The action the host notification layer should take for one durable event.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum SystemNotificationDecision {
    /// Do nothing.
    Ignore,
    /// Remove the notification with this durable identity.
    Revoke {
        /// The host notification identity to dismiss.
        id: String,
    },
    /// Present this notification unless the host layer is otherwise disabled.
    Show {
        /// The notification to present.
        notification: SystemNotification,
    },
}

/// Returns whether the event belongs to one of the three notifiable families.
///
/// Lifecycle events are considered notifiable even when their state is an
/// intermediate one; the state-specific decision later ignores those events.
#[must_use]
pub fn is_notifiable_event(envelope: &EventEnvelope<'_>) -> bool {
    matches!(
        envelope.payload,
        EventPayload::RunLifecycle { .. }
            | EventPayload::ApprovalInteraction { .. }
            | EventPayload::QuestionInteraction { .. }
    )
}

/// Collapses ECMAScript whitespace to one ASCII space and bounds the result.
///
/// The source policy uses JavaScript `/\s+/u` followed by `trim()`. The
/// explicit predicate below preserves that whitespace set, including U+FEFF
/// and excluding U+0085. Unlike JavaScript's UTF-16 `length`/`slice`, this
/// native policy counts Unicode scalar values, so the returned value is at
/// most 120 scalars and never cuts a UTF-8 sequence. Long values keep the
/// first 119 trimmed scalars and append U+2026.
#[must_use]
pub fn summarize(text: &str) -> String {
    let mut collapsed = String::with_capacity(text.len());
    let mut pending_space = false;

    for character in text.chars() {
        if is_ecmascript_whitespace(character) {
            pending_space = true;
            continue;
        }

        if pending_space && !collapsed.is_empty() {
            collapsed.push(' ');
        }
        collapsed.push(character);
        pending_space = false;
    }

    if collapsed.chars().count() <= 120 {
        return collapsed;
    }

    let mut truncated: String = collapsed.chars().take(119).collect();
    while truncated.ends_with(' ') {
        truncated.pop();
    }
    truncated.push('…');
    truncated
}

/// Returns the decision for one durable event and renderer context.
///
/// Resolved interactions revoke their identity before focus suppression is
/// considered. A focused window suppresses only a show for its active thread;
/// it never suppresses a revoke.
#[must_use]
pub fn system_notification_decision_for(
    envelope: &EventEnvelope<'_>,
    context: &SystemNotificationContext<'_>,
) -> SystemNotificationDecision {
    let resolved = matches!(
        envelope.payload,
        EventPayload::ApprovalInteraction {
            state: InteractionState::Resolved,
            ..
        } | EventPayload::QuestionInteraction {
            state: InteractionState::Resolved,
            ..
        }
    );

    if resolved {
        let Some(id) = interaction_notification_id(envelope) else {
            return SystemNotificationDecision::Ignore;
        };
        return SystemNotificationDecision::Revoke { id };
    }

    let Some(notification) = notification_for(envelope, context) else {
        return SystemNotificationDecision::Ignore;
    };

    if context.focused && context.active_thread_id == Some(envelope.thread_id) {
        return SystemNotificationDecision::Ignore;
    }

    SystemNotificationDecision::Show { notification }
}

fn interaction_notification_id(envelope: &EventEnvelope<'_>) -> Option<String> {
    match envelope.payload {
        EventPayload::ApprovalInteraction { approval_id, .. } => {
            Some(format!("approval:{approval_id}"))
        }
        EventPayload::QuestionInteraction { question_id, .. } => {
            Some(format!("question:{question_id}"))
        }
        EventPayload::RunLifecycle { .. } | EventPayload::Irrelevant => None,
    }
}

fn notification_for(
    envelope: &EventEnvelope<'_>,
    context: &SystemNotificationContext<'_>,
) -> Option<SystemNotification> {
    let route_path = context.route_path.to_owned();
    let title = context.thread_title.unwrap_or("Artisan").to_owned();

    match envelope.payload {
        EventPayload::RunLifecycle { state } => {
            let (body, category) = match state {
                RunLifecycleState::Completed => {
                    ("Finished.", SystemNotificationCategory::RunCompleted)
                }
                RunLifecycleState::Failed => {
                    ("The run failed.", SystemNotificationCategory::RunFailed)
                }
                RunLifecycleState::Queued
                | RunLifecycleState::Running
                | RunLifecycleState::Waiting
                | RunLifecycleState::Interrupted
                | RunLifecycleState::Cancelled
                | RunLifecycleState::Closed => return None,
            };
            let run_identity = envelope.run_id.unwrap_or(envelope.thread_id);
            Some(SystemNotification {
                body: body.to_owned(),
                category,
                id: format!("run:{run_identity}"),
                route_path,
                title,
            })
        }
        EventPayload::ApprovalInteraction {
            approval_id,
            description,
            state: InteractionState::Requested,
        } => Some(SystemNotification {
            body: format!("Needs approval — {}", summarize(description)),
            category: SystemNotificationCategory::Approval,
            id: format!("approval:{approval_id}"),
            route_path,
            title,
        }),
        EventPayload::QuestionInteraction {
            question_id,
            text,
            state: InteractionState::Requested,
        } => Some(SystemNotification {
            body: format!("Has a question — {}", summarize(text)),
            category: SystemNotificationCategory::Question,
            id: format!("question:{question_id}"),
            route_path,
            title,
        }),
        EventPayload::ApprovalInteraction {
            state: InteractionState::Resolved,
            ..
        }
        | EventPayload::QuestionInteraction {
            state: InteractionState::Resolved,
            ..
        }
        | EventPayload::Irrelevant => None,
    }
}

fn is_ecmascript_whitespace(character: char) -> bool {
    matches!(
        character,
        '\u{0009}'
            | '\u{000A}'
            | '\u{000B}'
            | '\u{000C}'
            | '\u{000D}'
            | '\u{0020}'
            | '\u{00A0}'
            | '\u{1680}'
            | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}
