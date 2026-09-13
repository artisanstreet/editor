//! Local send presentation stays separate from Forge's canonical transcript.
use super::*;

pub(super) struct LocalSend {
    pub thread: ThreadId,
    pub request: RequestId,
    pub message: Option<MessageId>,
    pub text: String,
}

impl NativeApplication {
    pub(super) fn stage_local_send(&mut self) {
        if let Some(flight) = &self.message_flight {
            let text = flight.payload.text().map_or("", |text| text.as_str());
            let text = if flight.payload.attachments().is_empty() {
                text.to_owned()
            } else {
                format!(
                    "{text}\n{} image attachment(s)",
                    flight.payload.attachments().len()
                )
            };
            self.optimistic_messages
                .retain(|row| row.request != flight.request_id);
            self.optimistic_messages.push(LocalSend {
                thread: flight.thread_id.clone(),
                request: flight.request_id.clone(),
                message: None,
                text,
            });
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
        let canonical: Vec<MessageId> = host
            .read(cx)
            .canonical_snapshot()
            .map(|snapshot| {
                snapshot
                    .items()
                    .iter()
                    .filter_map(|item| match item {
                        ConversationItem::UserMessage(message) => message.source_message_id.clone(),
                        ConversationItem::MultimodalUserMessage(message) => {
                            message.source_message_id.clone()
                        }
                        ConversationItem::AssistantMessage(_) => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
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
                        "Starting agent…"
                    } else {
                        "Sending…"
                    }
                    .to_owned()
                },
                |error| format!("Could not start agent: {error}. Forge is retrying."),
            );
            rows.push((row.text.clone(), status));
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
                || "Starting agent…".to_owned(),
                |error| format!("Could not start agent: {error}. Forge is retrying."),
            );
            rows.push((
                if entry.lip_text().is_empty() {
                    "Image attachment".to_owned()
                } else {
                    entry.lip_text().to_owned()
                },
                status,
            ));
        }
        Self::set_local_send_rows(host, rows, cx);
    }

    fn set_local_send_rows(
        host: &Entity<ConversationHost>,
        rows: Vec<(String, String)>,
        cx: &mut Context<Self>,
    ) {
        let surface = host.read(cx).surface().clone();
        if surface.update(cx, |surface, cx| surface.set_pending_messages(rows, cx)) {
            host.update(cx, |_, cx| cx.notify());
        }
    }
}
