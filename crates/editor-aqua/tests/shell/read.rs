use artisan_api::{
    EngineConfig, EngineId, Item, ItemsPage, ProjectId, Run, RunId, RunStatus, Thread, ThreadId,
    Usage,
};
use editor_aqua::conversation::fold::{Row, Slot};
use editor_aqua::shell::application::SectionViews;
use editor_aqua::shell::host::Shell;
use editor_aqua::shell::root::RootViews;
use test_support::editor::shell::{Motion, RootFrames, ThreadShell, headless, window_id};
use test_support::items::{assistant_reply, item};

const REPLY_SEQ: u64 = 4;
const RUN_SEQ: u64 = 5;

#[test]
fn a_reply_painted_while_unfocused_is_read_once_the_window_gains_focus() {
    let thread = ThreadId("read-thread".into());
    let run = RunId("read-run".into());
    let mut opened =
        ThreadShell::open_with("read-host", "read-thread", Motion::Full, |core, config| {
            let host = core.host_mut(&config.id).unwrap();
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
        });
    let reply = Item {
        position: 1,
        run_id: Some(run),
        ..item(
            "read-reply",
            thread.clone(),
            REPLY_SEQ,
            assistant_reply("The videos are both under 20 MB now."),
        )
    };
    let window = opened.window("read");
    let config = opened.host.clone();
    let ThreadShell {
        mut shell,
        mut resources,
        ..
    } = opened;
    let workspace = shell.entry.workspace_mut().unwrap();
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
    assert!(!window.focused);
    let mut root = RootViews::default();
    let element = root.build(&mut shell, &mut resources.ui, &window).unwrap();
    resources.ui.mount(|_| element).unwrap();
    resources.ui.build_frame().unwrap();
    let mut runtime = headless(window);
    let id = window_id();
    let pending = |shell: &Shell| shell.core.host(&config.id).unwrap().pending_read(&thread);
    let mut frame = RootFrames {
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
