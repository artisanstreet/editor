//! GPUI projection of the Forge message outbox and usage state.
//!
//! `NativeComposerControls` already owns the keyboard/pointer action buttons
//! for pending steering and failure cards. This module supplies that
//! component's exact rows from the Forge outbox and resolves controls events
//! back to a generation-fenced identity. It never withdraws, retries, or
//! recovers anything itself; the application sends those Forge commands.

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
    /// The caller asks the Forge to move the failed prompt into a new thread.
    NewThread(ComposerQueueIdentity),
    /// The caller asks the Forge to dispatch the failed message again.
    Retry(ComposerQueueIdentity),
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
    .with_compaction_at(usage.compaction_at_tokens)
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
    .retryable(entry.retryable())
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
        NativeComposerControlsEvent::RetryFailedDispatch {
            command_id,
            generation,
        } => (command_id.as_str(), *generation, QueueIntentKind::Retry),
        _ => return None,
    };
    let identity = ComposerQueueIdentity::new(command_id, generation);
    match intent {
        QueueIntentKind::NewThread => {
            return state
                .failed_entry_for_identity(&identity)
                .is_some()
                .then_some(QueueControlIntent::NewThread(identity));
        }
        QueueIntentKind::Retry => {
            return state
                .failed_entry_for_identity(&identity)
                .is_some_and(FailedQueueEntry::retryable)
                .then_some(QueueControlIntent::Retry(identity));
        }
        QueueIntentKind::Edit | QueueIntentKind::Discard => {}
    }
    let editable = state
        .pending_lip_rows()
        .into_iter()
        .any(|row| row.command_id == command_id && row.generation == generation && row.editable);
    editable.then_some(match intent {
        QueueIntentKind::Edit => QueueControlIntent::Edit(identity),
        _ => QueueControlIntent::Discard(identity),
    })
}

#[derive(Clone, Copy)]
enum QueueIntentKind {
    Edit,
    Discard,
    NewThread,
    Retry,
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
        AuthoredText, DispatchError, EngineId, FailedMessageListing, FailedMessageSummary,
        MessageId, MessageOutbox, QueuedMessageListOrder, QueuedMessageListing, QueuedMessageState,
        QueuedMessageSummary, RequestId, ThreadId, UnixMillis,
    };

    fn thread(value: &str) -> ThreadId {
        ThreadId::parse(value).expect("thread")
    }

    fn request(value: &str) -> RequestId {
        RequestId::parse(value).expect("request")
    }

    fn queued(thread_id: &ThreadId, id: &str, error: Option<&str>) -> QueuedMessageSummary {
        QueuedMessageSummary {
            message_id: MessageId::parse(format!("message-{id}")).expect("message"),
            thread_id: thread_id.clone(),
            original_request_id: request(&format!("command-{id}")),
            text: Some(AuthoredText::parse(id).expect("text")),
            attachments: Vec::new(),
            accepted_at: UnixMillis::EPOCH,
            last_error: error.map(|error| DispatchError::parse(error.to_owned()).expect("error")),
            state: QueuedMessageState::Queued,
            engine: Some(EngineId::Codex),
        }
    }

    fn failed(thread_id: &ThreadId, retryable: bool) -> FailedMessageSummary {
        FailedMessageSummary {
            message_id: MessageId::parse("message-failed").expect("message"),
            thread_id: thread_id.clone(),
            original_request_id: request("command-failed"),
            text: Some(AuthoredText::parse("hello").expect("text")),
            attachments: Vec::new(),
            accepted_at: UnixMillis::EPOCH,
            failed_at: UnixMillis::from_millis(500),
            reason: DispatchError::parse("engine profile unavailable".to_owned())
                .expect("diagnostic"),
            retryable,
        }
    }

    fn state_with(
        generation: u64,
        queued_rows: Vec<QueuedMessageSummary>,
        failed_rows: Vec<FailedMessageSummary>,
    ) -> ComposerQueueState {
        let thread_id = thread("thread-a");
        let mut state = ComposerQueueState::new();
        state.set_scope(Some(thread_id.clone()), generation);
        let outbox = MessageOutbox::new(
            QueuedMessageListing::new(
                thread_id.clone(),
                QueuedMessageListOrder::OldestFirst,
                32,
                queued_rows.len() as u64,
                queued_rows,
            )
            .expect("queued"),
            FailedMessageListing::new(thread_id, 32, failed_rows.len() as u64, failed_rows)
                .expect("failed"),
        )
        .expect("outbox");
        state.apply_outbox(&outbox).expect("outbox applies");
        state
    }

    #[test]
    fn controls_projection_preserves_forge_order_and_exact_count() {
        let thread_id = thread("thread-a");
        let state = state_with(
            5,
            vec![
                queued(&thread_id, "a", Some("engine profile unavailable")),
                queued(&thread_id, "b", None),
            ],
            Vec::new(),
        );
        let rows = state.pending_lip_rows();
        assert_eq!(
            rows.iter()
                .map(|row| row.command_id.as_str())
                .collect::<Vec<_>>(),
            ["command-a", "command-b"]
        );
        assert_eq!(queue_count_label(&state).as_deref(), Some("2 queued"));
        let mut snapshot = NativeComposerControlsSnapshot::default();
        project_controls_snapshot(&state, &mut snapshot, false);
        assert_eq!(snapshot.pending_steering.len(), 2);
        assert_eq!(snapshot.pending_steering[1].text, "b");
        assert_eq!(
            state.entries()[0].dispatch_error(),
            Some("engine profile unavailable")
        );
        assert_eq!(state.entries()[1].dispatch_error(), None);
    }

    #[test]
    fn stale_controls_generation_cannot_resolve_a_reused_command() {
        let thread_id = thread("thread-a");
        let state = state_with(7, vec![queued(&thread_id, "a", None)], Vec::new());
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

    #[test]
    fn failure_projection_carries_the_forge_verdicts_into_the_snapshot() {
        let thread_id = thread("thread-a");
        let state = state_with(9, Vec::new(), vec![failed(&thread_id, true)]);
        let mut snapshot = NativeComposerControlsSnapshot::default();
        project_controls_snapshot(&state, &mut snapshot, true);
        let [row] = snapshot.failed_dispatches.as_slice() else {
            panic!("one failure row");
        };
        assert_eq!(row.command_id(), "command-failed");
        assert_eq!(row.generation(), 9);
        assert_eq!(row.text, "hello");
        assert!(row.retryable);
        assert_eq!(row.reason, "engine profile unavailable");
        assert!(snapshot.failed_new_chat_ready);

        for (event, expected) in [
            (
                NativeComposerControlsEvent::StartNewThreadWithFailedPrompt {
                    command_id: "command-failed".to_owned(),
                    generation: 9,
                },
                true,
            ),
            (
                NativeComposerControlsEvent::RetryFailedDispatch {
                    command_id: "command-failed".to_owned(),
                    generation: 9,
                },
                true,
            ),
            (
                NativeComposerControlsEvent::RetryFailedDispatch {
                    command_id: "command-failed".to_owned(),
                    generation: 8,
                },
                false,
            ),
        ] {
            assert_eq!(resolve_controls_event(&state, &event).is_some(), expected);
        }
    }

    #[test]
    fn a_failure_the_forge_will_not_retry_offers_no_retry() {
        let thread_id = thread("thread-a");
        let state = state_with(3, Vec::new(), vec![failed(&thread_id, false)]);
        let mut snapshot = NativeComposerControlsSnapshot::default();
        project_controls_snapshot(&state, &mut snapshot, true);
        assert!(!snapshot.failed_dispatches[0].retryable);
        let retry = NativeComposerControlsEvent::RetryFailedDispatch {
            command_id: "command-failed".to_owned(),
            generation: 3,
        };
        assert!(resolve_controls_event(&state, &retry).is_none());
    }
}
