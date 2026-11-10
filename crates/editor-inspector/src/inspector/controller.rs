use super::{Entry, entries};
use editor_conversation::controller::Conversation;
use editor_core::{Core, CoreEvent, HostEvent, HostId, HostInstance, Screen};
use editor_sources::{CollectionBrowser, ContentText};
use artisan_api::{InspectorSummary, ItemId, PlanStep, ThreadId};
use std::{cell::RefCell, rc::Rc, sync::Arc};

pub struct Inspector {
    pub open: Option<(HostId, ThreadId)>,
    instance: Option<HostInstance>,
    pub summary: Arc<InspectorSummary>,
    pub entries: Vec<Entry>,
    pub error: Option<String>,
    pub source: Option<ContentText>,
    pub plan: Option<Rc<RefCell<CollectionBrowser<PlanStep>>>>,
}

impl Default for Inspector {
    fn default() -> Self {
        Self {
            open: None,
            instance: None,
            summary: Arc::default(),
            entries: Vec::new(),
            error: None,
            source: None,
            plan: None,
        }
    }
}

impl Inspector {
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
            .map(|host| host.instance());
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

    pub fn on_core_event(
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
                _ => changed,
            },
            _ => changed,
        }
    }

    pub fn sync_summary(&mut self, core: &Core) {
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
            _ => {}
        }
        self.entries = entries(&self.summary);
    }

    pub fn retry_summary(&self, core: &mut Core) {
        if let Some((host, thread)) = &self.open
            && let Some(instance) = self.instance
            && let Some(host) = core.host_mut_if_instance(host, instance)
        {
            host.load_thread_summary(thread, true);
        }
    }

    pub fn open_agent(&self, core: &mut Core, conversation: &mut Conversation, index: usize) {
        let Some(agent) = self.summary.agents.get(index) else {
            return;
        };
        let Some((host, _)) = &self.open else {
            return;
        };
        let Some(instance) = self.instance else {
            return;
        };
        if core.host_if_instance(host, instance).is_none() {
            return;
        }
        if let Some(thread) = &agent.thread {
            core.navigate(Screen::Thread {
                host: host.clone(),
                thread: thread.clone(),
            });
        } else {
            conversation.open_agent(core, Some(agent.id.clone()));
        }
    }

    pub fn open_agent_target(
        &self,
        core: &mut Core,
        conversation: &mut Conversation,
        id: artisan_api::ItemId,
        thread: Option<artisan_api::ThreadId>,
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

    pub fn open_source(&mut self, core: &Core, source: artisan_api::ContentRef) {
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

    pub fn open_plan(&mut self, core: &Core) {
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

    pub fn open_step(&mut self, core: &Core, index: usize) {
        let Some(step) = self
            .summary
            .plan
            .inline()
            .and_then(|steps| steps.get(index))
        else {
            return;
        };
        let artisan_api::TextContent::Stored(source) = &step.text else {
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
        self.source = Some(ContentText::new(host, source.clone()));
    }

    pub fn terminal_id(&self, index: usize) -> Option<&ItemId> {
        self.summary
            .terminals
            .get(index)
            .map(|terminal| &terminal.id)
    }
}
