//! Local send presentation stays separate from Forge's canonical transcript.
use super::*;

pub(super) struct LocalSend {
    pub thread: ThreadId,
    pub request: RequestId,
    pub message: Option<MessageId>,
    pub text: String,
}

impl NativeApplication {
    pub(super) fn stage_local_send(&mut self, cx: &mut Context<Self>) {
        if let Some(flight) = &self.message_flight {
            let text = flight.payload.text().map_or("", |text| text.as_str());
            let text = text.to_owned();
            self.optimistic_messages
                .retain(|row| row.request != flight.request_id);
            self.optimistic_messages.push(LocalSend {
                thread: flight.thread_id.clone(),
                request: flight.request_id.clone(),
                message: None,
                text,
            });
        }
        if let Some(host) = self.conversation_host.clone() {
            let surface = host.read(cx).surface().clone();
            surface.update(cx, |surface, cx| {
                surface.begin_send_entrance(
                    self.message_flight
                        .as_ref()
                        .expect("staged flight")
                        .request_id
                        .as_str()
                        .to_owned(),
                    cx,
                )
            });
            self.sync_local_sends(cx);
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
    }

    pub(super) fn sync_local_sends(&mut self, cx: &mut Context<Self>) {
        let Some(host) = self.conversation_host.as_ref() else {
            return;
        };
        let entries = if self.composer_queue.state.current_thread() == self.selected_thread.as_ref()
        {
            self.composer_queue.state.entries()
        } else {
            &[]
        };
        if entries.is_empty()
            && !self
                .optimistic_messages
                .iter()
                .any(|row| Some(&row.thread) == self.selected_thread.as_ref())
        {
            Self::set_local_send_rows(host, Vec::new(), cx);
            return;
        }
        let canonical_items: Vec<(MessageId, String)> = host
            .read(cx)
            .canonical_snapshot()
            .map(|snapshot| {
                snapshot
                    .items()
                    .iter()
                    .filter_map(|item| match item {
                        ConversationItem::UserMessage(message) => message
                            .source_message_id
                            .clone()
                            .map(|id| (id, message.item_id.as_str().to_owned())),
                        ConversationItem::MultimodalUserMessage(message) => message
                            .source_message_id
                            .clone()
                            .map(|id| (id, message.item_id.as_str().to_owned())),
                        ConversationItem::AssistantMessage(_) => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let surface = host.read(cx).surface().clone();
        for row in &self.optimistic_messages {
            if let Some((_, item_id)) = canonical_items
                .iter()
                .find(|(id, _)| row.message.as_ref() == Some(id))
            {
                surface.update(cx, |surface, _| {
                    surface.bind_send_entrance(row.request.as_str(), item_id)
                });
            }
        }
        let canonical: Vec<MessageId> = canonical_items.into_iter().map(|(id, _)| id).collect();
        let failed: Vec<_> = self
            .composer_queue
            .state
            .failed_entries()
            .iter()
            .map(|entry| entry.message_id().clone())
            .collect();
        self.optimistic_messages.retain(|row| {
            !row.message
                .as_ref()
                .is_some_and(|id| canonical.contains(id) || failed.contains(id))
        });
        let mut rows = Vec::new();
        for row in &self.optimistic_messages {
            if Some(&row.thread) != self.selected_thread.as_ref() {
                continue;
            }
            let error = entries
                .iter()
                .find(|entry| entry.identity().command_id() == row.request.as_str())
                .and_then(|entry| entry.dispatch_error());
            let status = error.map_or_else(
                || {
                    if row.message.is_some() {
                        "Queued — waiting to start…"
                    } else {
                        "Sending…"
                    }
                    .to_owned()
                },
                |error| format!("Could not start agent: {error}. Forge is retrying."),
            );
            let attachments = entries
                .iter()
                .find(|entry| entry.identity().command_id() == row.request.as_str())
                .map(|entry| entry.attachments().to_vec())
                .unwrap_or_default();
            rows.push((row.text.clone(), status, attachments));
        }
        // Include messages recovered from Forge after reopening a thread.
        for entry in entries {
            if canonical.contains(entry.message_id())
                || self
                    .optimistic_messages
                    .iter()
                    .any(|row| row.request.as_str() == entry.identity().command_id())
            {
                continue;
            }
            let status = entry.dispatch_error().map_or_else(
                || "Queued — waiting to start…".to_owned(),
                |error| format!("Could not start agent: {error}. Forge is retrying."),
            );
            rows.push((
                entry.lip_text().to_owned(),
                status,
                entry.attachments().to_vec(),
            ));
        }
        Self::set_local_send_rows(host, rows, cx);
    }

    fn set_local_send_rows(
        host: &Entity<ConversationHost>,
        rows: Vec<(String, String, Vec<artisan_domain::ImageAttachmentRef>)>,
        cx: &mut Context<Self>,
    ) {
        let surface = host.read(cx).surface().clone();
        if surface.update(cx, |surface, cx| surface.set_pending_messages(rows, cx)) {
            host.update(cx, |_, cx| cx.notify());
        }
    }
}
