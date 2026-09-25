//! The Forge message outbox as the Editor renders it.
//!
//! The Forge pushes each subscribed thread's outbox (accepted messages that
//! have not reached the transcript, with their delivery state, plus the
//! failures it still offers). The transcript tail shows exactly those rows;
//! when the Forge delivers a message its row leaves the outbox in the same
//! delivery that carries its transcript item. Nothing here matches echoes
//! or keeps a copy of a sent message: the only local state is the entrance
//! animation of a row's first appearance, which is pure view state.

use artisan_domain::{MessageOutbox, QueuedMessageState};

use super::*;
use crate::conversation_surface::PendingMessageRow;

impl NativeApplication {
    /// Installs a pushed outbox for the selected thread.
    pub(super) fn apply_message_outbox(&mut self, outbox: &MessageOutbox, cx: &mut Context<Self>) {
        if self.selected_thread.as_ref() != Some(outbox.thread_id()) {
            return;
        }
        let Ok(arrivals) = self.composer_queue.state.apply_outbox(outbox) else {
            return;
        };
        if let (Some(arrival), Some(host)) = (arrivals.last(), self.conversation_host.clone()) {
            let surface = host.read(cx).surface().clone();
            let message_id = arrival.as_str().to_owned();
            surface.update(cx, |surface, cx| {
                surface.begin_send_entrance(message_id, cx);
            });
        }
        self.sync_composer_controls(cx);
        cx.notify();
    }

    /// Renders the selected thread's Forge rows at the transcript tail.
    pub(super) fn sync_pending_rows(&mut self, cx: &mut Context<Self>) {
        let Some(host) = self.conversation_host.clone() else {
            return;
        };
        let rows = if self.composer_queue.state.current_thread() == self.selected_thread.as_ref() {
            pending_rows(self.composer_queue.state.entries())
        } else {
            Vec::new()
        };
        let surface = host.read(cx).surface().clone();
        let delivered = surface
            .read(cx)
            .send_entrance_message()
            .filter(|message_id| !rows.iter().any(|row| row.message_id == *message_id))
            .map(str::to_owned);
        if let Some(message_id) = delivered
            && let Some(item_id) = delivered_item(&host, &message_id, cx)
        {
            surface.update(cx, |surface, _| {
                surface.bind_send_entrance(&message_id, &item_id);
            });
        }
        if surface.update(cx, |surface, cx| surface.set_pending_messages(rows, cx)) {
            host.update(cx, |_, cx| cx.notify());
        }
    }

    /// Asks the transcript to follow its tail, where a sent message's row
    /// appears.
    pub(super) fn follow_transcript_tail(&mut self, cx: &mut Context<Self>) {
        let Some(host) = self.conversation_host.clone() else {
            return;
        };
        let _ = host.update(cx, |host, cx| {
            host.dispatch(
                ConversationStateEvent::Viewport(
                    crate::conversation_view_machine::ViewportEvent::JumpToBottomRequested,
                ),
                cx,
            )
        });
        self.pump_host_boundary(&host, cx);
    }

    /// Names the engine of a delivered user message's turn for the Waiting
    /// narration, from the engine the Forge's outbox row reported for that
    /// message (its accepted configuration snapshot).
    pub(super) fn label_delivered_turn(
        &mut self,
        item: &ConversationItem,
        host: &Entity<ConversationHost>,
        cx: &mut Context<Self>,
    ) {
        let (source_id, turn_id) = match item {
            ConversationItem::UserMessage(message) => {
                (message.source_message_id.as_ref(), &message.turn_id)
            }
            ConversationItem::MultimodalUserMessage(message) => {
                (message.source_message_id.as_ref(), &message.turn_id)
            }
            ConversationItem::AssistantMessage(_) => return,
        };
        let Some(engine) = source_id.and_then(|source_id| {
            self.composer_queue
                .state
                .entries()
                .iter()
                .find(|entry| entry.message_id() == source_id)
                .and_then(crate::composer_queue_state::ComposerQueueEntry::engine)
        }) else {
            return;
        };
        let dispatch = host.update(cx, |host, host_cx| {
            host.dispatch(
                ConversationStateEvent::SetTurnEngineLabel {
                    turn_id: turn_id.clone(),
                    engine_label: Some(profile_usage_display_name(engine.as_str()).to_owned()),
                },
                host_cx,
            )
        });
        if dispatch.is_ok() {
            self.pump_host_boundary(host, cx);
        }
    }
}

/// Projects Forge outbox rows into transcript-tail rows.
fn pending_rows(
    entries: &[crate::composer_queue_state::ComposerQueueEntry],
) -> Vec<PendingMessageRow> {
    entries
        .iter()
        .map(|entry| PendingMessageRow {
            message_id: entry.message_id().as_str().to_owned(),
            text: entry.lip_text().to_owned(),
            status: match (entry.state(), entry.dispatch_error()) {
                (QueuedMessageState::Dispatching, _) => "Starting…".to_owned(),
                (QueuedMessageState::Queued, Some(reason)) => format!("Waiting: {reason}"),
                (QueuedMessageState::Queued, None) => "Queued".to_owned(),
            },
            attachments: entry.attachments().to_vec(),
        })
        .collect()
}

/// The transcript item the Forge delivered for `message_id`, if projected.
fn delivered_item(host: &Entity<ConversationHost>, message_id: &str, cx: &App) -> Option<String> {
    host.read(cx)
        .canonical_snapshot()?
        .items()
        .iter()
        .find_map(|item| match item {
            ConversationItem::UserMessage(message)
                if message
                    .source_message_id
                    .as_ref()
                    .is_some_and(|source| source.as_str() == message_id) =>
            {
                Some(message.item_id.as_str().to_owned())
            }
            ConversationItem::MultimodalUserMessage(message)
                if message
                    .source_message_id
                    .as_ref()
                    .is_some_and(|source| source.as_str() == message_id) =>
            {
                Some(message.item_id.as_str().to_owned())
            }
            _ => None,
        })
}
