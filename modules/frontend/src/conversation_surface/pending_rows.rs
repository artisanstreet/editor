//! The Forge outbox rows painted at the transcript tail and the entrance
//! animation of a row's first appearance.
//!
//! The rows are exactly what the Forge's outbox reports; the surface keeps
//! no copy of a sent message. The entrance is presentation state only.

use super::*;

/// Presentation-only entrance of one newly arrived Forge outbox row: the
/// row rises in when it first appears, and the same clock carries over to
/// its transcript item if the Forge delivers it within the animation.
pub(super) struct SendEntrance {
    pub(super) started: Instant,
    pub(super) message_id: String,
    pub(super) target: Option<SceneId>,
}

/// One accepted message the Forge has not delivered to the transcript yet,
/// rendered at the transcript tail exactly as its outbox row describes it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PendingMessageRow {
    /// Forge-minted message identity.
    pub(crate) message_id: String,
    /// Authored text, empty for image-only messages.
    pub(crate) text: String,
    /// Forge delivery status as data (queued, starting, or the reason the
    /// Forge is holding it). Not painted beneath the bubble.
    pub(crate) status: String,
    /// Byte-free image references of the message.
    pub(crate) attachments: Vec<artisan_domain::ImageAttachmentRef>,
}

impl ConversationSurface {
    pub(crate) fn has_pending_messages(&self) -> bool {
        !self.pending_messages.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn pending_message_rows(&self) -> &[PendingMessageRow] {
        &self.pending_messages
    }

    /// Shows one queued text-only row, for surface tests.
    #[cfg(test)]
    pub(crate) fn show_queued_text(&mut self, text: String, cx: &mut Context<Self>) {
        let row = PendingMessageRow {
            message_id: "pending".to_owned(),
            text,
            status: "Queued".to_owned(),
            attachments: Vec::new(),
        };
        self.set_pending_messages(vec![row], cx);
    }

    pub(crate) fn set_pending_messages(
        &mut self,
        rows: Vec<PendingMessageRow>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.pending_messages != rows {
            self.pending_messages = rows;
            cx.notify();
            return true;
        }
        false
    }

    /// Starts the rise-in of one Forge outbox row on its first appearance.
    pub(crate) fn begin_send_entrance(&mut self, message_id: String, cx: &mut Context<Self>) {
        self.send_entrance = (!cx.reduce_motion()).then(|| SendEntrance {
            started: Instant::now(),
            message_id,
            target: None,
        });
        cx.notify();
    }

    /// The message whose entrance may still be running, if any.
    pub(crate) fn send_entrance_message(&self) -> Option<&str> {
        self.send_entrance
            .as_ref()
            .filter(|entrance| entrance.target.is_none())
            .map(|entrance| entrance.message_id.as_str())
    }

    /// Carries a running entrance over to the transcript item the Forge
    /// delivered for its message.
    pub(crate) fn bind_send_entrance(&mut self, message_id: &str, item_id: &str) {
        if let Some(entrance) = &mut self.send_entrance
            && entrance.message_id == message_id
        {
            entrance.target = SceneId::parse(item_id.to_owned()).ok();
        }
    }
}
