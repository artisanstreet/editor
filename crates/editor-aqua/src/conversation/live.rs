use crate::conversation::controller::Conversation;
use crate::conversation::presentation::PresentationScope;
use crate::core::Core;
use artisan_api::{ItemId, LiveCoverage, LiveTarget, Status};
use std::sync::Arc;

#[derive(Clone)]
pub struct LiveBinding {
    pub scope: PresentationScope,
    pub item: ItemId,
    pub epoch: u64,
    pub text: Arc<ropey::Rope>,
    pub coverage: LiveCoverage,
}

impl LiveBinding {
    pub fn capture(conversation: &Conversation, core: &Core, item: &ItemId) -> Option<Self> {
        let scope = PresentationScope::capture(conversation, core)?;
        let view = conversation.view.as_ref()?;
        if view.item(item)?.status != Status::Running {
            return None;
        }
        let target = LiveTarget::Text {
            item_id: item.clone(),
        };
        let model = view.live.get(&target)?;
        let snapshot = core
            .host_if_instance(&scope.host, scope.instance)?
            .live_snapshot(&scope.thread, &target)?;
        if !Arc::ptr_eq(model, &snapshot.text) {
            return None;
        }
        Some(Self {
            scope,
            item: item.clone(),
            epoch: snapshot.epoch,
            text: snapshot.text,
            coverage: snapshot.coverage,
        })
    }

    pub fn current(&self, conversation: &Conversation, core: &Core) -> bool {
        Self::capture(conversation, core, &self.item).is_some_and(|current| {
            current.scope == self.scope
                && current.epoch == self.epoch
                && current.coverage == self.coverage
                && Arc::ptr_eq(&current.text, &self.text)
        })
    }

    pub fn complete(&self, conversation: &Conversation, core: &Core) -> bool {
        self.current(conversation, core) && self.coverage.is_complete(self.text.len_bytes())
    }

    pub fn preview_notice(&self) -> Option<String> {
        (!self.coverage.is_complete(self.text.len_bytes())).then(|| format!(
            "Live preview shows {} of {} bytes. Complete content is available after the reply is saved.",
            self.text.len_bytes(), self.coverage.total,
        ))
    }
}
