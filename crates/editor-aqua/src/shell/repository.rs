use crate::core::{Core, HostId, HostInstance, Screen};
use artisan_api::{ProjectId, Seq, ThreadId};

#[derive(PartialEq, Eq)]
struct Selection {
    host: HostId,
    instance: HostInstance,
    project: ProjectId,
    project_revision: Seq,
}

#[derive(Default)]
pub struct RepositoryContext {
    selected: Option<Selection>,
    thread: Option<ThreadId>,
    working: bool,
}

impl RepositoryContext {
    pub fn reset(&mut self) {
        self.selected = None;
    }

    pub fn sync(&mut self, core: &mut Core) {
        let (host_id, thread, project) = match core.screen() {
            Screen::Thread { host, thread } => (Some(host.clone()), Some(thread.clone()), None),
            Screen::NewThread { host, project } => (Some(host.clone()), None, project.clone()),
            _ => (None, None, None),
        };
        let Some(host) = host_id.as_ref().and_then(|id| core.host(id)) else {
            self.selected = None;
            self.thread = None;
            self.working = false;
            return;
        };
        let project = project.or_else(|| {
            thread.as_ref().and_then(|thread| {
                host.threads
                    .get(thread)
                    .map(|thread| thread.project_id.clone())
            })
        });
        let selected = project.and_then(|id| {
            host.projects.get(&id).map(|project| Selection {
                host: host.id.clone(),
                instance: host.instance(),
                project: id,
                project_revision: project.seq,
            })
        });
        let working = thread
            .as_ref()
            .is_some_and(|thread| host.is_working(thread));
        let completed = self.thread == thread && self.working && !working;
        let refresh = self.selected != selected || completed;
        self.thread = thread;
        self.working = working;
        self.selected = selected;
        if refresh {
            if let Some(selected) = &self.selected {
                if let Some(host) = core.host_mut_if_instance(&selected.host, selected.instance) {
                    host.load_repository(&selected.project, true);
                }
            }
        }
    }
}
