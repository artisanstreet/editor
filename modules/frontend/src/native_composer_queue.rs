//! GPUI projection for the authoritative queued-message lip and usage state.
//!
//! `NativeComposerControls` already owns the keyboard/pointer action buttons
//! for pending steering. This module supplies that component's exact rows,
//! renders the bounded count/status companion, and resolves controls events
//! back to a generation-fenced queue identity. It never withdraws, reads, or
//! restores anything itself; the application owns those transport and draft
//! operations.

#![forbid(unsafe_code)]
#![allow(clippy::module_name_repetitions)]

use crate::composer_queue_state::{
    ComposerQueueIdentity, ComposerQueueState, FailedQueueEntry, QueueLipRow, ReportingUsage,
};
use crate::native_composer_controls::{
    FailedDispatchRow, NativeComposerControlsEvent, NativeComposerControlsSnapshot,
    PendingSteeringRow,
};
use crate::native_context_usage::NativeContextUsage;

/// A controls action resolved to one exact byte-free queue identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum QueueControlIntent {
    /// The caller must capture the empty composer target before beginning it.
    Edit(ComposerQueueIdentity),
    /// The caller can begin withdrawal without reading image bytes.
    Discard(ComposerQueueIdentity),
    /// The caller must recall the exact failed payload and carry it into a
    /// new thread as an unsent draft. No retry on the failed thread exists.
    NewThread(ComposerQueueIdentity),
}

/// Copies the queue/usage projection into the existing controls snapshot.
///
/// Rows remain in the authoritative page order and leave the lip only after a
/// later parent projection omits them. Context usage is made from the last
/// reporting run, if any; the existing `NativeContextUsage::presentation`
/// hides a gauge when the report omitted context or its window.
pub(crate) fn project_controls_snapshot(
    state: &ComposerQueueState,
    snapshot: &mut NativeComposerControlsSnapshot,
    failed_new_chat_ready: bool,
) {
    snapshot.pending_steering = state
        .pending_lip_rows()
        .into_iter()
        .map(pending_steering_row)
        .collect();
    snapshot.failed_dispatches = state
        .failed_entries()
        .iter()
        .map(failed_dispatch_row)
        .collect();
    snapshot.failed_new_chat_ready = failed_new_chat_ready;
    snapshot.context_usage = native_context_usage_for(state, snapshot.run_id.as_deref());
    if snapshot.run_id.is_none() {
        snapshot.run_id = snapshot
            .context_usage
            .as_ref()
            .map(|usage| usage.reporting_run_id.clone());
    }
    if snapshot.context_usage.is_none() {
        snapshot.context_usage_open = false;
    }
}

/// Suppresses old attempts without deleting the recoverable failed payloads.
pub(crate) fn hide_failures_before(
    state: &ComposerQueueState,
    snapshot: &mut NativeComposerControlsSnapshot,
    latest: Option<artisan_domain::UnixMillis>,
) {
    snapshot.failed_dispatches.retain(|row| {
        state.failed_entries().iter().any(|entry| {
            entry.identity().command_id() == row.command_id()
                && latest.is_none_or(|latest| entry.accepted_at() >= latest)
        })
    });
}

fn native_context_usage_for(
    state: &ComposerQueueState,
    current_run_id: Option<&str>,
) -> Option<NativeContextUsage> {
    let usage = state.reporting_usage_for(current_run_id)?;
    Some(native_context_usage(usage))
}

fn native_context_usage(usage: ReportingUsage) -> NativeContextUsage {
    NativeContextUsage::new(
        usage.run_id,
        usage.engine_id,
        usage.model_id,
        usage.model_name,
        usage.context_tokens,
        usage.context_window_tokens,
    )
    .with_breakdown(
        usage.input_tokens,
        usage.cached_input_tokens,
        usage.output_tokens,
    )
}

fn pending_steering_row(row: QueueLipRow) -> PendingSteeringRow {
    PendingSteeringRow::new(row.command_id, row.generation, row.text, row.editable)
}

fn failed_dispatch_row(entry: &FailedQueueEntry) -> FailedDispatchRow {
    FailedDispatchRow::new(
        entry.identity().command_id(),
        entry.identity().generation(),
        entry.card_text(),
        entry.has_attachments(),
        entry.reason(),
    )
}

/// Resolves an existing controls event without performing its side effect.
///
/// The generation and original queue command id must still be present in the
/// current page and editable projection. The parent then obtains the entry's
/// exact thread/message/original-request ids from `ComposerQueueState` before
/// creating a transport command.
#[must_use]
pub(crate) fn resolve_controls_event(
    state: &ComposerQueueState,
    event: &NativeComposerControlsEvent,
) -> Option<QueueControlIntent> {
    let (command_id, generation, intent) = match event {
        NativeComposerControlsEvent::EditQueuedSteer {
            command_id,
            generation,
        } => (command_id.as_str(), *generation, QueueIntentKind::Edit),
        NativeComposerControlsEvent::DiscardQueuedSteer {
            command_id,
            generation,
        } => (command_id.as_str(), *generation, QueueIntentKind::Discard),
        NativeComposerControlsEvent::StartNewThreadWithFailedPrompt {
            command_id,
            generation,
        } => (command_id.as_str(), *generation, QueueIntentKind::NewThread),
        _ => return None,
    };
    let identity = ComposerQueueIdentity::new(command_id, generation);
    if matches!(intent, QueueIntentKind::NewThread) {
        return state
            .failed_entries()
            .iter()
            .any(|entry| entry.identity() == &identity)
            .then_some(QueueControlIntent::NewThread(identity));
    }
    let editable = state
        .pending_lip_rows()
        .into_iter()
        .any(|row| row.command_id == command_id && row.generation == generation && row.editable);
    editable.then_some(match intent {
        QueueIntentKind::Edit => QueueControlIntent::Edit(identity),
        QueueIntentKind::Discard => QueueControlIntent::Discard(identity),
        QueueIntentKind::NewThread => QueueControlIntent::NewThread(identity),
    })
}

#[derive(Clone, Copy)]
enum QueueIntentKind {
    Edit,
    Discard,
    NewThread,
}

/// Returns the exact queue count copy, or no copy when no rows/count are
/// authoritative yet. A count remains exact even when the server says the
/// bounded page has more rows than it can display.
#[must_use]
#[cfg(test)]
pub(crate) fn queue_count_label(state: &ComposerQueueState) -> Option<String> {
    if state.total_count() == 0 && state.entries().is_empty() {
        return None;
    }
    if state.has_more() {
        Some(format!(
            "{} queued · showing {} here",
            state.total_count(),
            state.entries().len()
        ))
    } else {
        Some(format!("{} queued", state.total_count()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use artisan_domain::{
        AuthoredText, DispatchError, FailedMessageListing, FailedMessageSummary, MessageId,
        QueuedMessageListOrder, QueuedMessageListing, QueuedMessageSummary, RequestId, ThreadId,
        UnixMillis,
    };

    fn thread(value: &str) -> ThreadId {
        ThreadId::parse(value).expect("thread")
    }

    fn request(value: &str) -> RequestId {
        RequestId::parse(value).expect("request")
    }

    fn page(thread_id: &ThreadId) -> QueuedMessageListing {
        QueuedMessageListing::new(
            thread_id.clone(),
            QueuedMessageListOrder::OldestFirst,
            2,
            2,
            vec![
                QueuedMessageSummary {
                    message_id: MessageId::parse("message-a").expect("message"),
                    thread_id: thread_id.clone(),
                    original_request_id: request("command-a"),
                    text: Some(AuthoredText::parse("first").expect("text")),
                    attachments: Vec::new(),
                    accepted_at: UnixMillis::EPOCH,
                    last_error: Some(
                        artisan_domain::DispatchError::parse("engine unconfigured".to_owned())
                            .expect("dispatcher diagnostic"),
                    ),
                },
                QueuedMessageSummary {
                    message_id: MessageId::parse("message-b").expect("message"),
                    thread_id: thread_id.clone(),
                    original_request_id: request("command-b"),
                    text: Some(AuthoredText::parse("second").expect("text")),
                    attachments: Vec::new(),
                    accepted_at: UnixMillis::EPOCH,
                    last_error: None,
                },
            ],
        )
        .expect("page")
    }

    #[test]
    fn controls_projection_preserves_order_and_exact_count() {
        let thread_id = thread("thread-a");
        let mut state = ComposerQueueState::new();
        state.set_scope(Some(thread_id.clone()), 5);
        let token = state
            .begin_queue_refresh(true, true, false, false, true)
            .expect("refresh");
        state
            .apply_queue_listing(&token, &page(&thread_id))
            .expect("page");

        let rows = state.pending_lip_rows();
        assert_eq!(
            rows.iter()
                .map(|row| row.command_id.as_str())
                .collect::<Vec<_>>(),
            ["command-a", "command-b"]
        );
        assert_eq!(queue_count_label(&state).as_deref(), Some("2 queued"));

        let mut snapshot = NativeComposerControlsSnapshot {
            run_id: None,
            ..Default::default()
        };
        project_controls_snapshot(&state, &mut snapshot, false);
        assert_eq!(snapshot.pending_steering.len(), 2);
        assert_eq!(snapshot.pending_steering[1].text, "second");
        assert_eq!(
            state.entries()[0].dispatch_error(),
            Some("engine unconfigured")
        );
        assert_eq!(state.entries()[1].dispatch_error(), None);
    }

    #[test]
    fn stale_controls_generation_cannot_resolve_a_reused_command() {
        let thread_id = thread("thread-a");
        let mut state = ComposerQueueState::new();
        state.set_scope(Some(thread_id.clone()), 7);
        let token = state
            .begin_queue_refresh(true, true, false, false, true)
            .expect("refresh");
        state
            .apply_queue_listing(&token, &page(&thread_id))
            .expect("page");
        let outdated = NativeComposerControlsEvent::EditQueuedSteer {
            command_id: "command-a".to_owned(),
            generation: 6,
        };
        assert!(resolve_controls_event(&state, &outdated).is_none());
        let current = NativeComposerControlsEvent::DiscardQueuedSteer {
            command_id: "command-a".to_owned(),
            generation: 7,
        };
        assert!(matches!(
            resolve_controls_event(&state, &current),
            Some(QueueControlIntent::Discard(_))
        ));
    }

    fn failed_page(thread_id: &ThreadId) -> FailedMessageListing {
        FailedMessageListing::new(
            thread_id.clone(),
            32,
            1,
            vec![FailedMessageSummary {
                message_id: MessageId::parse("message-failed").expect("message"),
                thread_id: thread_id.clone(),
                original_request_id: request("command-failed"),
                text: Some(AuthoredText::parse("hello").expect("text")),
                attachments: Vec::new(),
                accepted_at: UnixMillis::EPOCH,
                failed_at: UnixMillis::from_millis(500),
                reason: DispatchError::parse(
                    "provider continuation unavailable: the prior run was interrupted with unknown outcome; start a new chat to continue".to_owned(),
                )
                .expect("diagnostic"),
            }],
        )
        .expect("failed page")
    }

    #[test]
    fn newer_accepted_message_hides_stale_failure_without_discarding_payload() {
        let thread_id = thread("thread-a");
        let mut state = ComposerQueueState::new();
        state.set_scope(Some(thread_id.clone()), 9);
        let token = state.begin_failed_refresh(true, true, false, true).unwrap();
        state
            .apply_failed_listing(&token, &failed_page(&thread_id))
            .unwrap();
        let mut snapshot = NativeComposerControlsSnapshot::default();
        project_controls_snapshot(&state, &mut snapshot, true);
        hide_failures_before(&state, &mut snapshot, Some(UnixMillis::EPOCH));
        assert_eq!(
            snapshot.failed_dispatches.len(),
            1,
            "current failure stays actionable"
        );
        hide_failures_before(&state, &mut snapshot, Some(UnixMillis::from_millis(1)));
        assert!(snapshot.failed_dispatches.is_empty());
        assert_eq!(
            state.failed_entries().len(),
            1,
            "recovery payload remains stored"
        );
    }

    #[test]
    fn failed_projection_carries_reason_and_readiness_into_snapshot() {
        let thread_id = thread("thread-a");
        let mut state = ComposerQueueState::new();
        state.set_scope(Some(thread_id.clone()), 9);
        let token = state
            .begin_failed_refresh(true, true, false, true)
            .expect("failed refresh");
        state
            .apply_failed_listing(&token, &failed_page(&thread_id))
            .expect("failed page");

        let mut snapshot = NativeComposerControlsSnapshot::default();
        project_controls_snapshot(&state, &mut snapshot, true);
        assert_eq!(snapshot.failed_dispatches.len(), 1);
        let row = &snapshot.failed_dispatches[0];
        assert_eq!(row.command_id(), "command-failed");
        assert_eq!(row.generation(), 9);
        assert_eq!(row.text, "hello");
        assert!(!row.has_attachments);
        assert_eq!(
            row.reason,
            "provider continuation unavailable: the prior run was interrupted with unknown outcome; start a new chat to continue"
        );
        assert!(snapshot.failed_new_chat_ready);

        let mut blocked = NativeComposerControlsSnapshot::default();
        project_controls_snapshot(&state, &mut blocked, false);
        assert_eq!(blocked.failed_dispatches.len(), 1);
        assert!(!blocked.failed_new_chat_ready);

        let event = NativeComposerControlsEvent::StartNewThreadWithFailedPrompt {
            command_id: "command-failed".to_owned(),
            generation: 9,
        };
        assert!(matches!(
            resolve_controls_event(&state, &event),
            Some(QueueControlIntent::NewThread(_))
        ));
        let outdated = NativeComposerControlsEvent::StartNewThreadWithFailedPrompt {
            command_id: "command-failed".to_owned(),
            generation: 8,
        };
        assert!(resolve_controls_event(&state, &outdated).is_none());
    }
}
