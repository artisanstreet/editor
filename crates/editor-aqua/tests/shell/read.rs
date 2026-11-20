use artisan_api::{
    EngineConfig, EngineId, Item, ItemsPage, ProjectId, Run, RunId, RunOutcome, RunState, Thread,
    ThreadId, UpdateThread, Usage,
};
use editor_aqua::conversation::fold::{Row, Slot};
use editor_aqua::shell::application::SectionViews;
use editor_aqua::shell::root::RootViews;
use test_support::editor::forge::{StubForge, empty_state};
use test_support::editor::shell::{Motion, RootFrames, ThreadShell, headless, window_id};
use test_support::items::{assistant_reply, item};

const REPLY_SEQ: u64 = 4;
const RUN_SEQ: u64 = 5;

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one focus-and-read scenario walked step by step"
)]
fn a_reply_painted_while_unfocused_is_read_once_the_window_gains_focus() {
    let thread = ThreadId("read-thread".into());
    let run = RunId("read-run".into());
    let mut state = empty_state();
    state.threads.push(Thread {
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
    });
    state.runs.push(Run {
        config: None,
        id: run.clone(),
        thread_id: thread.clone(),
        engine: EngineId::Claude,
        model: None,
        input_cutoff: 0,
        state: RunState::Finished {
            outcome: RunOutcome::Done,
            error: None,
            ended_at: 0,
        },
        usage: Usage::default(),
        session_id: None,
        seq: RUN_SEQ,
        started_at: 0,
        continuation: None,
    });
    state.head = RUN_SEQ;
    let forge = editor_aqua::core::runtime::runtime()
        .block_on(StubForge::serve(state, axum::Router::new()));
    let mut opened = ThreadShell::open_stubbed(&forge, "read-host", "read-thread", Motion::Full);
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
    let host = opened.host.id.clone();
    let ThreadShell {
        mut shell,
        mut resources,
        ..
    } = opened;
    let workspace = shell.entry.workspace_mut().unwrap();
    workspace
        .conversation_mut()
        .view
        .as_mut()
        .unwrap()
        .apply_page(
            editor_aqua::conversation::viewport::Fetch::Tail,
            ItemsPage {
                items: vec![reply.clone()],
                runs: Vec::new(),
                more: false,
                work: Vec::new(),
            },
        );
    workspace.conversation_mut().rows.make_mut().push((
        Row::Text {
            id: reply.id,
            slot: Slot::Prose,
            block: 0,
            run: None,
        },
        1,
    ));
    workspace.conversation_mut().landing = None;
    assert!(!window.focused);
    let mut root = RootViews::default();
    let element = root.build(&mut shell, &mut resources.ui, &window).unwrap();
    resources.ui.mount(|_| element).unwrap();
    resources.ui.build_frame().unwrap();
    let mut runtime = headless(window);
    let id = window_id();
    let acknowledgement = format!("/threads/{thread}");
    let mut frame = RootFrames {
        root: &mut root,
        shell: &mut shell,
        ui: &mut resources.ui,
    };
    runtime.dispatch_frame_completed(&mut frame, id, 1);
    assert!(
        !frame
            .shell
            .core
            .host(&host)
            .unwrap()
            .has_requests_in_flight()
            && !forge.asked("PATCH", &acknowledgement),
        "a reply painted behind an unfocused window has not been read"
    );
    runtime.dispatch_window_event(
        &mut frame,
        id,
        &aqua::prelude::WindowEvent::Focused(id, true),
    );
    runtime.dispatch_frame_completed(&mut frame, id, 2);
    let read = forge.take("PATCH", &acknowledgement).json::<UpdateThread>();
    assert_eq!(
        read.read_seq,
        Some(RUN_SEQ),
        "focusing the window over an unchanged reply must mark the finished run read"
    );
}
