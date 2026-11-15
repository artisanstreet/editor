use artisan_api::{
    EngineConfig, EngineId, Item, ItemId, ItemKind, ItemsPage, Phase, ProjectId, Run, RunId,
    RunStatus, Status, TextContent, Thread, ThreadId, Usage,
};
use editor_aqua::conversation::fold::{Row, Slot};
use editor_aqua::core::{Core, EditorFiles, ForgeClient, HostConfig, HostId, HostKind, Screen};
use editor_aqua::shell::application::SectionViews;
use editor_aqua::shell::host::{Shell, ShellResources};
use editor_aqua::shell::root::RootViews;

const REPLY_SEQ: u64 = 4;
const RUN_SEQ: u64 = 5;

struct Frame<'a> {
    root: &'a mut RootViews,
    shell: &'a mut Shell,
    ui: &'a mut aqua::UiApp,
}

impl aqua::Application for Frame<'_> {
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
        self.root.frame_completed(self.shell, self.ui, ctx, id, sequence);
    }
}

#[test]
fn a_reply_painted_while_unfocused_is_read_once_the_window_gains_focus() {
    let directory = tempfile::tempdir().unwrap();
    let files = EditorFiles {
        dir: directory.path().to_owned(),
    };
    let mut resources = ShellResources::new(1.0).unwrap();
    let (mut shell, _) = Shell::new(files.clone(), resources.images.clone());
    shell.core = Core::for_tests(files).unwrap();
    shell.core.set_image_assets(resources.images.clone());
    let config = HostConfig {
        id: HostId("read-host".into()),
        name: "Forge".into(),
        address: "127.0.0.1:1".into(),
        fingerprint: "0".repeat(64),
        kind: HostKind::Remote,
    };
    let client = ForgeClient::new(
        &config.address,
        &config.fingerprint,
        shell.core.identity.as_ref().unwrap(),
    )
    .unwrap();
    shell.core.add_test_host(&config, client);
    let thread = ThreadId("read-thread".into());
    let run = RunId("read-run".into());
    let host = shell.core.host_mut(&config.id).unwrap();
    host.threads.insert(
        thread.clone(),
        Thread {
            id: thread.clone(),
            project_id: ProjectId::scratch(),
            title: Some("Unread reply".into()),
            config: EngineConfig::new(EngineId::Claude),
            activity_seq: RUN_SEQ,
            activity_at: 0,
            read_seq: 1,
            marked_unread: false,
            settled_seq: 0,
            seq: RUN_SEQ,
            created_at: 0,
            updated_at: 0,
            archived: false,
            parent_thread_id: None,
            spawned_by: None,
            parked_until: None,
        },
    );
    host.runs.insert(
        thread.clone(),
        Run {
            config: None,
            id: run.clone(),
            thread_id: thread.clone(),
            engine: EngineId::Claude,
            model: None,
            input_cutoff: 0,
            status: RunStatus::Done,
            usage: Usage::default(),
            session_id: None,
            error: None,
            seq: RUN_SEQ,
            started_at: 0,
            ended_at: Some(0),
            continuation: None,
        },
    );
    shell.core.navigate(Screen::Thread {
        host: config.id.clone(),
        thread: thread.clone(),
    });
    shell.entry.sync(&shell.core);
    let workspace = shell.entry.workspace_mut().unwrap();
    workspace.show_sidebar = false;
    workspace
        .conversation
        .open_thread(&mut shell.core, config.id.clone(), thread.clone());
    workspace.sync_composer(&shell.core, &mut resources.ui);
    let reply = Item {
        id: ItemId("read-reply".into()),
        thread_id: thread.clone(),
        position: 1,
        seq: REPLY_SEQ,
        run_id: Some(run),
        parent_id: None,
        status: Status::Done,
        created_at: 0,
        updated_at: 0,
        kind: ItemKind::AssistantMessage {
            text: TextContent::Inline("The videos are both under 20 MB now.".into()),
            phase: Phase::Final,
        },
    };
    workspace.conversation.view.as_mut().unwrap().apply_page(
        editor_aqua::conversation::viewport::Fetch::Tail,
        ItemsPage {
            items: vec![reply.clone()],
            runs: Vec::new(),
            more: false,
            work: Vec::new(),
        },
    );
    workspace.conversation.rows.push((
        Row::Text {
            id: reply.id.clone(),
            slot: Slot::Prose,
            block: 0,
            run: None,
        },
        1,
    ));
    workspace.conversation.landing = None;
    let id = aqua::prelude::AquaWindowId::new(1).unwrap();
    let mut size = aqua::prelude::WindowOptions::default().inner_size;
    size.width = 1000.0;
    size.height = 700.0;
    let window = aqua::prelude::ObservedWindowState::new(id, "read".into(), size);
    assert!(!window.focused);
    resources.ui.set_viewport(1000.0, 700.0, 1.0);
    let mut root = RootViews::default();
    let element = root.build(&mut shell, &mut resources.ui, &window).unwrap();
    resources.ui.mount(|_| element).unwrap();
    resources.ui.build_frame().unwrap();
    let mut runtime = aqua::Aqua::new(aqua::AquaConfig {
        headless: true,
        ..aqua::AquaConfig::default()
    })
    .unwrap();
    runtime.add_headless_window(window);
    let pending = |shell: &Shell| shell.core.host(&config.id).unwrap().pending_read(&thread);
    let mut frame = Frame {
        root: &mut root,
        shell: &mut shell,
        ui: &mut resources.ui,
    };
    runtime.dispatch_frame_completed(&mut frame, id, 1);
    assert_eq!(
        pending(frame.shell),
        None,
        "a reply painted behind an unfocused window has not been read"
    );
    runtime.dispatch_window_event(
        &mut frame,
        id,
        &aqua::prelude::WindowEvent::Focused(id, true),
    );
    runtime.dispatch_frame_completed(&mut frame, id, 2);
    assert_eq!(
        pending(frame.shell),
        Some(RUN_SEQ),
        "focusing the window over an unchanged reply must mark the finished run read"
    );
}
