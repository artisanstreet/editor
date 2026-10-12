//! The sidebar against a real in-process Forge: what the inbox and History
//! list, and what Forge records when a thread is opened.

use crate::Sidebar;
use artisan_api::tls::Identity;
use artisan_api::{
    CreateProject, CreateThread, EngineConfig, EngineId, ItemId, ItemKind, Phase, ProjectId, Run,
    RunId, RunStatus, Status, ThreadId, Usage,
};
use editor_core::runtime::runtime;
use editor_core::{Core, EditorFiles, HostKind, Screen, hosts};
use gpui::{AppContext as _, Entity, Modifiers, TestAppContext, VisualTestContext, WindowHandle};
use std::net::SocketAddr;
use std::time::{Duration, Instant};

struct Setup {
    store: forge_store::Store,
    forge: tokio::runtime::Runtime,
    core: Entity<Core>,
    window: WindowHandle<Sidebar>,
    project: ProjectId,
    _dir: tempfile::TempDir,
}

impl Setup {
    fn new(cx: &mut TestAppContext) -> Self {
        cx.executor().allow_parking();
        let dir = tempfile::tempdir().unwrap();
        let forge = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let listen: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let server = forge
            .block_on(forge_http::start(forge_http::ServeConfig {
                data_dir: dir.path().join("forge"),
                listen,
            }))
            .unwrap();
        let address = server.local_addr().unwrap().to_string();
        let store = server.store().clone();
        forge.spawn(async move {
            let _ = server.run().await;
        });

        let files = EditorFiles {
            dir: dir.path().join("editor"),
        };
        let identity: Identity = files.identity().unwrap();
        let invite = forge.block_on(forge_http::invite(&store)).unwrap();
        let fingerprint = artisan_api::tls::fingerprint_of(&invite.forge_certificate).unwrap();
        let host = runtime()
            .block_on(hosts::pair(
                &address,
                &invite.code,
                &fingerprint,
                &identity,
                "test",
                "Forge".into(),
                HostKind::Local,
            ))
            .unwrap();

        let core = cx.new(|cx| Core::new(files, cx).unwrap());
        core.update(cx, |core, cx| core.add_host(host, cx)).unwrap();
        let window = cx.add_window(|window, cx| Sidebar::new(core.clone(), window, cx));

        let path = dir.path().join("project");
        std::fs::create_dir_all(&path).unwrap();
        let project = ProjectId::new();
        forge
            .block_on(store.create_project(CreateProject {
                id: project.clone(),
                path: path.display().to_string(),
                name: None,
            }))
            .unwrap();
        Self {
            store,
            forge,
            core,
            window,
            project,
            _dir: dir,
        }
    }

    fn thread(&self, parent: Option<&ThreadId>) -> ThreadId {
        let id = ThreadId::new();
        let request = CreateThread {
            id: id.clone(),
            project_id: self.project.clone(),
            config: EngineConfig::new(EngineId::Claude),
            title: Some("A thread".into()),
            parent_thread_id: parent.cloned(),
        };
        self.forge
            .block_on(self.store.create_thread(request))
            .unwrap();
        id
    }

    /// The engine replies in `thread`: new activity the user hasn't seen.
    fn reply(&self, thread: &ThreadId) {
        let item = forge_store::NewItem {
            id: ItemId::new(),
            thread_id: thread.clone(),
            run_id: None,
            parent_id: None,
            native_id: None,
            status: Status::Done,
            kind: ItemKind::AssistantMessage {
                text: "done".into(),
                phase: Phase::Final,
            },
        };
        self.forge.block_on(self.store.upsert_item(item)).unwrap();
    }

    /// A run of `thread` that ended `hours` ago.
    fn ran(&self, thread: &ThreadId, hours: i64) {
        let ended = artisan_api::now() - hours * 3_600_000;
        let run = Run {
            id: RunId::new(),
            thread_id: thread.clone(),
            engine: EngineId::Claude,
            model: None,
            status: RunStatus::Done,
            usage: Usage::default(),
            session_id: None,
            error: None,
            seq: 0,
            started_at: ended - 60_000,
            ended_at: Some(ended),
        };
        self.forge.block_on(self.store.upsert_run(run)).unwrap();
    }

    fn lists(&self, cx: &mut TestAppContext) -> (Vec<ThreadId>, Vec<ThreadId>) {
        self.window
            .read_with(cx, |sidebar, _| (sidebar.inbox(), sidebar.history()))
            .unwrap()
    }

    /// Lets the Editor apply what Forge sends until `check` holds.
    fn wait(
        &self,
        cx: &mut TestAppContext,
        what: &str,
        check: impl Fn(&Self, &mut TestAppContext) -> bool,
    ) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            cx.run_until_parked();
            if check(self, cx) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting until {what}; lists: {:?}",
                self.lists(cx)
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[gpui::test]
fn threads_stay_in_the_inbox_until_settled(cx: &mut TestAppContext) {
    let setup = Setup::new(cx);
    let quiet = setup.thread(None);
    let answered = setup.thread(None);
    setup.reply(&answered);
    setup.wait(cx, "both threads are in the inbox", |setup, cx| {
        setup.lists(cx) == (vec![answered.clone(), quiet.clone()], vec![])
    });

    // Opening it tells Forge it was read; reading never moves it.
    setup
        .window
        .update(cx, |sidebar, _, cx| {
            sidebar.open_thread(answered.clone(), cx);
        })
        .unwrap();
    setup.wait(cx, "Forge records the thread as read", |setup, _| {
        let thread = setup
            .forge
            .block_on(setup.store.thread(&answered))
            .unwrap()
            .unwrap();
        thread.read_seq == thread.activity_seq
    });
    let host = setup.core.read_with(cx, |core, cx| {
        core.active_host().unwrap().read(cx).id.clone()
    });
    setup.core.update(cx, |core, cx| {
        core.navigate(
            Screen::NewThread {
                host,
                project: None,
            },
            cx,
        );
    });
    setup.wait(cx, "the read mark arrives back", |setup, cx| {
        setup.core.read_with(cx, |core, cx| {
            core.active_host().unwrap().read(cx).threads[&answered].read_seq > 0
        })
    });
    assert_eq!(
        setup.lists(cx).0,
        vec![answered.clone(), quiet.clone()],
        "a read thread keeps its place until it is settled"
    );

    // Settling files it into History; new activity brings it back.
    setup
        .window
        .update(cx, |sidebar, _, cx| sidebar.settle(&answered, cx))
        .unwrap();
    setup.wait(cx, "the settled thread moves to History", |setup, cx| {
        setup.lists(cx) == (vec![quiet.clone()], vec![answered.clone()])
    });
    setup.reply(&answered);
    setup.wait(cx, "new activity returns it to the inbox", |setup, cx| {
        setup.lists(cx).0 == vec![answered.clone(), quiet.clone()]
    });
}

#[gpui::test]
fn subagent_threads_are_not_listed_at_the_top_level(cx: &mut TestAppContext) {
    let setup = Setup::new(cx);
    let parent = setup.thread(None);
    let child = setup.thread(Some(&parent));
    setup.reply(&child);
    setup.reply(&parent);
    setup.wait(cx, "the parent shows", |setup, cx| {
        setup.lists(cx).0 == vec![parent.clone()]
    });
    let (inbox, history) = setup.lists(cx);
    assert!(
        !inbox.contains(&child) && !history.contains(&child),
        "the subagent's thread is listed: {inbox:?} {history:?}"
    );
}

#[gpui::test]
fn history_opens_below_its_row_without_a_heading_for_the_last_day(cx: &mut TestAppContext) {
    let setup = Setup::new(cx);
    let settled = setup.thread(None);
    setup.ran(&settled, 2);
    setup.wait(cx, "the thread is listed", |setup, cx| {
        setup.lists(cx).0 == vec![settled.clone()]
    });
    setup
        .window
        .update(cx, |sidebar, _, cx| sidebar.settle(&settled, cx))
        .unwrap();
    setup.wait(cx, "the thread settles into History", |setup, cx| {
        setup.lists(cx).1 == vec![settled.clone()]
    });

    let mut cx = VisualTestContext::from_window(setup.window.into(), cx);
    cx.run_until_parked();
    let row: &'static str = format!("sidebar-thread-{settled}").leak();
    assert!(cx.debug_bounds(row).is_none(), "History is collapsed");
    let history = cx.debug_bounds("sidebar-history").expect("the History row");
    cx.simulate_click(history.center(), Modifiers::none());
    cx.run_until_parked();
    // The hover pill's measuring repaints the sidebar after the click, and
    // GPUI's cached rows don't carry their debug bounds into a reused
    // frame; repaint the rows so they report where they are.
    setup
        .window
        .update(&mut cx, |sidebar, _, cx| {
            for row in sidebar.rows.values() {
                row.update(cx, |_, cx| cx.notify());
            }
        })
        .unwrap();
    cx.run_until_parked();

    let shown = cx.debug_bounds(row).expect("the settled row shows");
    for heading in [
        "sidebar-history-last-24-hours",
        "sidebar-history-last-3-days",
        "sidebar-history-last-30-days",
        "sidebar-history-older",
    ] {
        assert!(
            cx.debug_bounds(heading).is_none(),
            "{heading}: the last day has no heading, the rest no threads"
        );
    }
    let history = cx.debug_bounds("sidebar-history").unwrap();
    assert!(
        shown.top() >= history.bottom(),
        "the row sits under the History row: {shown:?} vs {history:?}"
    );
}
