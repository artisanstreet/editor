//! Echo-watch correlation for [`NativeApplication`]: retiring staged send
//! watches when the canonical user item projects, and dispatching each
//! turn's send-time engine metadata (typed engine plus display label).
//!
//! Extracted from `impl_service_events.rs`; behavior is unchanged.

use super::*;

impl NativeApplication {
    /// Resolves the Waiting-narration engine label at echo time.
    ///
    /// The send-time captured label stands UNLESS exact correlation proves
    /// the observed run is ours: the watch named this run as its steer
    /// target, or the canonical snapshot already holds an assistant item of
    /// the echoed turn produced by the observed run. A merely same-thread
    /// observed run proves nothing â€” an old poll can describe the previous
    /// engine while a new cross-engine send just launched â€” so without
    /// proof the captured label stands, and without any label the generic
    /// fallback renders. The picker is never consulted after the send.
    pub(super) fn resolve_dispatch_engine_label(
        &self,
        thread_id: &ThreadId,
        echo_turn_id: &TurnId,
        send_label: Option<TurnEngineLabel>,
        steer_run_id: Option<&artisan_domain::RunId>,
        host: &Entity<ConversationHost>,
        cx: &App,
    ) -> Option<TurnEngineLabel> {
        if let Some((observed_id, observed_engine)) =
            self.run_controls.observed_run(Some(thread_id))
        {
            let exact = steer_run_id == Some(&observed_id)
                || host.read(cx).canonical_snapshot().is_some_and(|snapshot| {
                    snapshot.items().iter().any(|item| match item {
                        ConversationItem::AssistantMessage(message) => {
                            message.turn_id == *echo_turn_id && message.run_id == observed_id
                        }
                        _ => false,
                    })
                });
            if exact {
                return Some(TurnEngineLabel::for_engine(observed_engine));
            }
        }
        send_label
    }

    /// Retires the staged echo watch for one pre-matched source id.
    ///
    /// Take-up marks immediately (the echo was observed, so the lip retires
    /// even while still listed), but the watch is kept until the label
    /// dispatch succeeds: backpressure must not permanently lose the label.
    /// A later echo re-attempts the dispatch; the lip marking and the
    /// watch take stay idempotent.
    pub(super) fn retire_echo_matched(
        &mut self,
        thread_id: &ThreadId,
        message_id: &MessageId,
        turn_id: TurnId,
        host: &Entity<ConversationHost>,
        cx: &mut Context<Self>,
    ) {
        if self.selected_thread.as_ref() != Some(thread_id) {
            return;
        }
        let (send_label, steer_run_id) = match self.composer_queue.state.echo_watch_for(message_id)
        {
            Some(watch) => (
                watch.turn_engine_label().cloned(),
                watch.steer_run_id().cloned(),
            ),
            None => return,
        };
        self.composer_queue.state.mark_taken_up(message_id);
        let engine_label = self.resolve_dispatch_engine_label(
            thread_id,
            &turn_id,
            send_label,
            steer_run_id.as_ref(),
            host,
            cx,
        );
        let dispatch = host.update(cx, |host, host_cx| {
            host.dispatch(
                ConversationStateEvent::SetTurnEngineLabel {
                    turn_id,
                    engine_label,
                },
                host_cx,
            )
        });
        if dispatch.is_ok() {
            self.composer_queue.state.clear_echo_watch_for(message_id);
            // Drain the invalidation the label dispatch raised (plus any
            // sibling boundary work) so one effect does not accumulate per
            // send.
            self.pump_host_boundary(host, cx);
        }
        self.sync_composer_controls(cx);
    }

    /// Retires the staged echo watch when the canonical user item projects.
    ///
    /// Frozen native correlation contract: matches ONLY
    /// `item.source_message_id == receipt.message_id`, never item-id
    /// equality. An absent legacy source id matches nothing â€” no guessed
    /// take-up or label; the forced queue refresh stays the fallback.
    /// Take-up marks on first match so the lip cannot duplicate; the watch
    /// clears only once the label dispatch succeeds, so a later duplicate
    /// re-attempts a lost label idempotently.
    pub(super) fn retire_echo_for_item(
        &mut self,
        thread_id: &ThreadId,
        item: &ConversationItem,
        host: &Entity<ConversationHost>,
        cx: &mut Context<Self>,
    ) {
        if self.selected_thread.as_ref() != Some(thread_id) {
            return;
        }
        let (source_id, turn_id) = match item {
            ConversationItem::UserMessage(message) => {
                (message.source_message_id.as_ref(), message.turn_id.clone())
            }
            ConversationItem::MultimodalUserMessage(message) => {
                (message.source_message_id.as_ref(), message.turn_id.clone())
            }
            // Assistant messages are the only other item kind and
            // never echo a send.
            ConversationItem::AssistantMessage(_) => return,
        };
        let Some(source_id) = source_id else {
            return;
        };
        if self
            .composer_queue
            .state
            .echo_watch_for(source_id)
            .is_none()
        {
            return;
        }
        self.retire_echo_matched(thread_id, source_id, turn_id, host, cx);
    }

    /// Re-scans the canonical snapshot for staged echo watches.
    ///
    /// A watch retained across a failed label dispatch (backpressure) must
    /// not wait for another duplicate `ItemUpsert` that may never arrive:
    /// every authoritative refresh re-resolves retained watches against
    /// what is already projected. No-ops when no watch is staged.
    pub(super) fn rescan_retained_echo_watches(&mut self, cx: &mut Context<Self>) {
        if self.composer_queue.state.echo_watch_count() == 0 {
            return;
        }
        let Some(thread_id) = self.selected_thread.clone() else {
            return;
        };
        let Some(host) = self.conversation_host.clone() else {
            return;
        };
        if host.read(cx).controller_view().delivery.thread_id != thread_id {
            return;
        }
        let Some(snapshot) = host.read(cx).canonical_snapshot() else {
            return;
        };
        let mut matches = Vec::new();
        for item in snapshot.items() {
            let (source_id, turn_id) = match item {
                ConversationItem::UserMessage(message) => {
                    (message.source_message_id.as_ref(), message.turn_id.clone())
                }
                ConversationItem::MultimodalUserMessage(message) => {
                    (message.source_message_id.as_ref(), message.turn_id.clone())
                }
                // Assistant messages are the only other item kind and
                // never echo a send.
                ConversationItem::AssistantMessage(_) => continue,
            };
            if let Some(source_id) = source_id
                && self
                    .composer_queue
                    .state
                    .echo_watch_for(source_id)
                    .is_some()
            {
                matches.push((source_id.clone(), turn_id));
            }
        }
        for (message_id, turn_id) in matches {
            self.retire_echo_matched(&thread_id, &message_id, turn_id, &host, cx);
        }
    }
}
