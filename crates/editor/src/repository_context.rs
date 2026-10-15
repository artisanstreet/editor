//! Repository reads follow the selected Forge project and run lifecycle.

use artisan_api::{ProjectId, Seq, ThreadId};
use editor_core::{Core, CoreEvent, HostEvent, HostId, Screen};
use gpui::{Context, Entity, Subscription};

#[derive(PartialEq, Eq)]
struct Selection {
    host: HostId,
    project: ProjectId,
    project_revision: Seq,
}

pub struct RepositoryContext {
    core: Entity<Core>,
    selected: Option<Selection>,
    thread: Option<ThreadId>,
    working: bool,
    host: Option<HostId>,
    _host: Option<Subscription>,
    _core: Subscription,
}
impl RepositoryContext {
    pub fn new(core: Entity<Core>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&core, |this, _, event, cx| {
            if matches!(event, CoreEvent::ScreenChanged | CoreEvent::HostsChanged) {
                this.sync(cx);
            }
        });
        let mut this = Self {
            core,
            selected: None,
            thread: None,
            working: false,
            host: None,
            _host: None,
            _core: subscription,
        };
        this.sync(cx);
        this
    }
    fn sync(&mut self, cx: &mut Context<Self>) {
        let core = self.core.read(cx);
        let (host, thread, project) = match core.screen() {
            Screen::Thread { host, thread } => {
                (core.host(host).cloned(), Some(thread.clone()), None)
            }
            Screen::NewThread { host, project } => {
                (core.host(host).cloned(), None, project.clone())
            }
            _ => (None, None, None),
        };
        let host_id = host.as_ref().map(|host| host.read(cx).id.clone());
        if self.host != host_id {
            self.host = host_id;
            self._host = host.as_ref().map(|host| {
                cx.subscribe(host, |this, _, event, cx| match event {
                    HostEvent::Reset => {
                        this.selected = None;
                        this.sync(cx);
                    }
                    HostEvent::Changed => this.sync(cx),
                    HostEvent::ThreadChanged { thread_id }
                        if this.thread.as_ref() == Some(thread_id) =>
                    {
                        this.sync(cx)
                    }
                    _ => {}
                })
            });
        }
        let Some(host) = host else {
            self.selected = None;
            self.thread = None;
            self.working = false;
            return;
        };
        let data = host.read(cx);
        let project = project.or_else(|| {
            thread.as_ref().and_then(|thread| {
                data.threads
                    .get(thread)
                    .map(|thread| thread.project_id.clone())
            })
        });
        let selected = project.and_then(|id| {
            data.projects.get(&id).map(|project| Selection {
                host: data.id.clone(),
                project: id,
                project_revision: project.seq,
            })
        });
        let working = thread
            .as_ref()
            .is_some_and(|thread| data.is_working(thread));
        let completed = self.thread == thread && self.working && !working;
        let refresh = self.selected != selected || completed;
        self.thread = thread;
        self.working = working;
        self.selected = selected;
        if refresh && let Some(selected) = &self.selected {
            let project = selected.project.clone();
            host.update(cx, |host, cx| host.load_repository(&project, true, cx));
        }
    }
}
