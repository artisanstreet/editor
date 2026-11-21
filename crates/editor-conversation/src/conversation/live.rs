use crate::conversation::controller::Conversation;
use crate::conversation::presentation::PresentationScope;
use artisan_api::{ItemId, LiveCoverage, LiveTarget, Status};
use editor_core::Core;
use std::sync::Arc;

#[derive(Debug, Clone)]
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
        if model.epoch != snapshot.epoch || !Arc::ptr_eq(&model.text, &snapshot.text) {
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
            "Live preview shows {} of {}. Complete content is available after the reply is saved.",
            editor_ui::locale::integer(self.text.len_bytes()),
            editor_ui::locale::count(self.coverage.total, "byte", "bytes"),
        ))
    }
}
