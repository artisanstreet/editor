use super::core::{connect, remote_host, test_core_with_clock};
use artisan_api::ThreadId;
use editor_aqua::core::{Clock, Core, EditorFiles, HostConfig, Screen};
use editor_aqua::shell::application::SectionViews as _;
use editor_aqua::shell::host::{Shell, ShellResources};
use editor_aqua::shell::root::RootViews;
use editor_aqua::shell::workspace::Workspace;
use tempfile::TempDir;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Full,
    Reduced,
}

pub struct ThreadShell {
    pub shell: Shell,
    pub resources: ShellResources,
    pub host: HostConfig,
    pub thread: ThreadId,
    pub directory: TempDir,
}

impl ThreadShell {
    pub fn open(host: &str, thread: &str, motion: Motion) -> Self {
        Self::open_with(host, thread, motion, |_, _| {})
    }

    pub fn open_at(host: &str, thread: &str, motion: Motion, clock: Clock) -> Self {
        Self::open_with_clock(host, thread, motion, clock, |_, _| {})
    }

    pub fn open_with(
        host: &str,
        thread: &str,
        motion: Motion,
        prepare: impl FnOnce(&mut Core, &HostConfig),
    ) -> Self {
        Self::open_with_clock(host, thread, motion, Clock::system(), prepare)
    }

    fn open_with_clock(
        host: &str,
        thread: &str,
        motion: Motion,
        clock: Clock,
        prepare: impl FnOnce(&mut Core, &HostConfig),
    ) -> Self {
        let directory = tempfile::tempdir().expect("create a test data folder");
        let files = EditorFiles {
            dir: directory.path().to_owned(),
        };
        let mut resources = ShellResources::new(1.0).expect("create headless shell resources");
        if motion == Motion::Reduced {
            resources
                .ui
                .host_mut()
                .motion_mut()
                .set_reduced_motion(aqua_ui::motion::ReducedMotion::Reduce, 0.0);
        }
        let (mut shell, _) = Shell::new(files, resources.images.clone());
        shell.core = test_core_with_clock(directory.path(), clock);
        shell.core.set_image_assets(resources.images.clone());
        let host = remote_host(host);
        connect(&mut shell.core, &host);
        prepare(&mut shell.core, &host);
        let thread = ThreadId::from(thread);
        shell.core.navigate(Screen::Thread {
            host: host.id.clone(),
            thread: thread.clone(),
        });
        shell.entry.sync(&shell.core);
        let workspace = shell
            .entry
            .workspace_mut()
            .expect("a signed-in shell on a thread shows a workspace");
        workspace
            .conversation_mut()
            .open_thread(&mut shell.core, host.id.clone(), thread.clone());
        workspace.sync_composer(&shell.core, &mut resources.ui);
        Self {
            shell,
            resources,
            host,
            thread,
            directory,
        }
    }

    pub fn workspace(&mut self) -> &mut Workspace {
        self.shell
            .entry
            .workspace_mut()
            .expect("a signed-in shell on a thread shows a workspace")
    }

    pub fn window(&mut self, title: &str) -> aqua::prelude::ObservedWindowState {
        let mut size = aqua::prelude::WindowOptions::default().inner_size;
        size.width = 1000.0;
        size.height = 700.0;
        self.resources.ui.set_viewport(1000.0, 700.0, 1.0);
        aqua::prelude::ObservedWindowState::new(window_id(), title.into(), size)
    }
}

impl std::fmt::Debug for ThreadShell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThreadShell")
            .field("host", &self.host.id)
            .field("thread", &self.thread)
            .finish_non_exhaustive()
    }
}

pub fn window_id() -> aqua::prelude::AquaWindowId {
    aqua::prelude::AquaWindowId::new(1).expect("invariant: window id 1 is non-zero")
}

pub fn headless(window: aqua::prelude::ObservedWindowState) -> aqua::Aqua {
    let mut runtime = aqua::Aqua::new(aqua::AquaConfig {
        headless: true,
        ..aqua::AquaConfig::default()
    })
    .expect("start a headless Aqua runtime");
    runtime.add_headless_window(window);
    runtime
}

pub struct RootFrames<'a> {
    pub root: &'a mut RootViews,
    pub shell: &'a mut Shell,
    pub ui: &'a mut aqua::UiApp,
}

impl std::fmt::Debug for RootFrames<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RootFrames").finish_non_exhaustive()
    }
}

impl aqua::Application for RootFrames<'_> {
    fn on_window_event(
        &mut self,
        _: &mut aqua::AppCtx<'_>,
        _: aqua::prelude::AquaWindowId,
        _: &aqua::prelude::WindowEvent,
    ) {
    }

    fn on_input(
        &mut self,
        _: &mut aqua::AppCtx<'_>,
        _: aqua::prelude::AquaWindowId,
        _: &aqua::prelude::InputEvent,
    ) {
    }

    fn build_scene(
        &mut self,
        _: &aqua::prelude::ObservedWindowState,
        _: aqua::Instant,
    ) -> Option<aqua::Instant> {
        None
    }

    fn on_frame_completed(
        &mut self,
        ctx: &mut aqua::AppCtx<'_>,
        id: aqua::prelude::AquaWindowId,
        sequence: u64,
    ) {
        self.root
            .frame_completed(self.shell, self.ui, ctx, id, sequence);
    }
}
