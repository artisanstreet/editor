use super::{Entry, entries};
use artisan_api::{InspectorSummary, PlanStep, ThreadId};
use editor_conversation::controller::Conversation;
use editor_core::{Core, CoreEvent, HostEvent, HostId, HostInstance, Screen};
use editor_sources::{CollectionBrowser, ContentText};
use std::{cell::RefCell, rc::Rc, sync::Arc};

#[derive(Default)]
pub struct Inspector {
    pub(crate) open: Option<(HostId, ThreadId)>,
    instance: Option<HostInstance>,
    pub(crate) summary: Arc<InspectorSummary>,
    pub(crate) entries: Vec<Entry>,
    pub(crate) error: Option<String>,
    pub(crate) source: Option<ContentText>,
    pub(crate) plan: Option<Rc<RefCell<CollectionBrowser<PlanStep>>>>,
}

impl std::fmt::Debug for Inspector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Inspector").finish_non_exhaustive()
    }
}

impl Inspector {
    pub const fn open(&self) -> Option<&(HostId, ThreadId)> {
        self.open.as_ref()
    }

    pub fn summary(&self) -> &InspectorSummary {
        &self.summary
    }

    pub fn is_current(&self, core: &Core) -> bool {
        self.open.as_ref().is_some_and(|(host, _)| {
            self.instance
                .is_some_and(|instance| core.host_if_instance(host, instance).is_some())
        })
    }

    pub fn follow(&mut self, core: &Core, conversation: &Conversation) -> bool {
        let instance = conversation
            .open
            .as_ref()
            .and_then(|(host, _)| core.host(host))
            .map(editor_core::HostData::instance);
        if self.open == conversation.open && self.instance == instance {
            return false;
        }
        self.open.clone_from(&conversation.open);
        self.instance = instance;
        self.summary = Arc::default();
        self.entries.clear();
        self.error = None;
        self.source = None;
        self.plan = None;
        self.sync_summary(core);
        true
    }

    pub(crate) fn on_core_event(
        &mut self,
        core: &Core,
        conversation: &Conversation,
        event: &CoreEvent,
    ) -> bool {
        let changed = self.follow(core, conversation);
        let Some((host, thread)) = &self.open else {
            return changed;
        };
        match event {
            CoreEvent::Host {
                host: event_host,
                event,
            } if event_host == host => match event {
                HostEvent::Reset => {
                    self.sync_summary(core);
                    true
                }
                HostEvent::ThreadSummaryChanged { thread_id } if thread_id == thread => {
                    self.sync_summary(core);
                    true
                }
                HostEvent::Changed | HostEvent::ThreadChanged { .. } => true,
                HostEvent::ImagesChanged
                | HostEvent::EngineInstall { .. }
                | HostEvent::ThreadSummaryChanged { .. }
                | HostEvent::WslDistro { .. }
                | HostEvent::Item(_)
                | HostEvent::Live { .. }
                | HostEvent::EditorCall { .. } => changed,
            },
            CoreEvent::ScreenChanged
            | CoreEvent::HostsChanged
            | CoreEvent::SettingsChanged
            | CoreEvent::AccessChanged
            | CoreEvent::McpSignInChanged
            | CoreEvent::OpenUrl(_)
            | CoreEvent::Account(_)
            | CoreEvent::Host { .. } => changed,
        }
    }

    fn sync_summary(&mut self, core: &Core) {
        let state = self.open.as_ref().and_then(|(host, thread)| {
            core.host_if_instance(host, self.instance?)?
                .thread_summary(thread)
        });
        self.error = state.and_then(|state| state.error.clone());
        let summary = state
            .and_then(|state| state.value.clone())
            .unwrap_or_default();
        if Arc::ptr_eq(&summary, &self.summary) {
            return;
        }
        self.summary = summary;
        match (&mut self.plan, self.summary.plan.stored()) {
            (Some(plan), Some(source)) => plan.borrow_mut().set_source(source.clone()),
            (_, None) => self.plan = None,
            (None, Some(_)) => {}
        }
        self.entries = entries(&self.summary);
    }

    pub(crate) fn retry_summary(&self, core: &mut Core) {
        if let Some((host, thread)) = &self.open
            && let Some(instance) = self.instance
            && let Some(host) = core.host_mut_if_instance(host, instance)
        {
            host.load_thread_summary(thread, true);
        }
    }

    pub(crate) fn open_agent_target(
        &self,
        core: &mut Core,
        conversation: &mut Conversation,
        id: artisan_api::ItemId,
        thread: Option<ThreadId>,
    ) {
        let Some((host, _)) = &self.open else {
            return;
        };
        let Some(instance) = self.instance else {
            return;
        };
        if core.host_if_instance(host, instance).is_none() {
            return;
        }
        if let Some(thread) = thread {
            core.navigate(Screen::Thread {
                host: host.clone(),
                thread,
            });
        } else {
            conversation.open_agent(core, Some(id));
        }
    }

    pub(crate) fn open_source(&mut self, core: &Core, source: artisan_api::ContentRef) {
        let Some((host, _)) = &self.open else {
            return;
        };
        let Some(instance) = self.instance else {
            return;
        };
        let Some(host) = core.host_if_instance(host, instance) else {
            return;
        };
        self.source = Some(ContentText::new(host, source));
    }

    pub(crate) fn open_plan(&mut self, core: &Core) {
        let Some(source) = self.summary.plan.stored().cloned() else {
            return;
        };
        let Some((host, _)) = &self.open else {
            return;
        };
        let Some(instance) = self.instance else {
            return;
        };
        let Some(host) = core.host_if_instance(host, instance) else {
            return;
        };
        self.plan = Some(Rc::new(RefCell::new(CollectionBrowser::new(host, source))));
    }
}
