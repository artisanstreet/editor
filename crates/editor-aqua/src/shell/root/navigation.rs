use super::RootViews;
use crate::shell::host::Shell;
use aqua::prelude::AquaWindowId;
use aqua::{AppCtx, UiApp};

impl RootViews {
    pub(super) fn sync_navigation(
        &mut self,
        ui: &mut UiApp,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
        now: std::time::Instant,
    ) -> bool {
        let _tm = crate::shell::application::Tm::new("v.navigation");
        let frame = self.navigation.sync(ui, now);
        if frame.changed || frame.animating {
            ctx.request_frame(window, aqua::prelude::FrameReason::ANIMATION);
        }
        frame.animating
    }

    pub(super) fn apply_navigation(&mut self, shell: &mut Shell, reduced: bool) -> bool {
        use crate::frame::sidebar::Command;
        let now = shell.core.clock().now();
        let mut outlined = false;
        let Some(workspace) = shell.entry.workspace_mut() else {
            return outlined;
        };
        let core = &mut shell.core;
        for command in self.sidebar.take_commands() {
            match command {
                Command::Open(thread) => workspace.sidebar.open_thread(core, thread),
                Command::New => workspace.sidebar.new_thread(core, None),
                Command::Project(project) => workspace.sidebar.new_thread(core, Some(project)),
                Command::Agent(agent) => workspace.conversation.open_agent(core, agent),
                Command::Outline => {
                    self.navigation.toggle(now, reduced);
                    outlined = true;
                }
                Command::OpenUrl(url) => {
                    if let Err(error) = crate::shell::open_url(&url) {
                        shell.failures.link(url, &error);
                    }
                }
                Command::Settle {
                    host,
                    instance,
                    thread,
                    observed_seq,
                } => {
                    let unchanged = workspace.sidebar.host.as_ref() == Some(&host)
                        && core
                            .host_if_instance(&host, instance)
                            .and_then(|host| host.threads.get(&thread))
                            .is_some_and(|thread| thread.activity_seq == observed_seq);
                    if unchanged
                        && workspace
                            .sidebar
                            .lists
                            .inbox
                            .iter()
                            .any(|row| row.id == thread && row.settleable)
                    {
                        workspace.sidebar.settle(core, thread);
                    }
                }
            }
        }
        if !self.sidebar.refresh_due(now) {
            return outlined;
        }
        let open_project = workspace.sidebar.open.as_ref().and_then(|thread| {
            let host = core.host(workspace.sidebar.host.as_ref()?)?;
            Some(host.threads.get(thread)?.project_id.clone())
        });
        let Some(host) = workspace
            .sidebar
            .host
            .as_ref()
            .and_then(|id| core.host_mut(id))
        else {
            return outlined;
        };
        let projects = host.projects.keys().cloned().collect::<Vec<_>>();
        for project in &projects {
            host.load_project_icon(project);
            if !host.repository_checked(project) || open_project.as_ref() == Some(project) {
                host.load_repository(project, false);
            }
        }
        outlined
    }
}
