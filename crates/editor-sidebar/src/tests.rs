//! The sidebar against a real in-process Forge: what the inbox and History
//! list, and what Forge records when a thread is opened.

#[path = "../../editor-ui/test_support/forge.rs"]
pub(crate) mod forge_fixture;
#[path = "../../editor-ui/test_support/server.rs"]
mod server_fixture;
use crate::Sidebar;
use artisan_api::{
    CreateProject, CreateThread, EngineConfig, EngineId, ItemId, ItemKind, Phase, ProjectId, Run,
    RunId, RunStatus, Status, ThreadId, Usage,
};
use editor_core::{Core, Screen};
use gpui::{AppContext as _, Entity, Modifiers, TestAppContext, VisualTestContext, WindowHandle};
use std::time::{Duration, Instant};

pub(crate) struct Setup {
    pub(crate) store: forge_store::Store,
    pub(crate) forge: tokio::runtime::Runtime,
    pub(crate) core: Entity<Core>,
    pub(crate) window: WindowHandle<Sidebar>,
    project: ProjectId,
    pub(crate) _dir: tempfile::TempDir,
}

impl Setup {
    pub(crate) fn new(cx: &mut TestAppContext) -> Self {
        cx.executor().allow_parking();
        let server_fixture::Server {
            store,
            runtime: forge,
            address,
            directory: dir,
        } = server_fixture::start();

        let (files, _) = forge_fixture::pair(&store, &forge, &address, &dir.path().join("editor"));
        cx.update(editor_ui::text_editor::TextEditor::bind_actions);
        let core = cx.new(|cx| Core::new(files, cx).unwrap());
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
        self.ran(thread, 0);
    }

    /// A run of `thread` that ended `hours` ago.
    fn ran(&self, thread: &ThreadId, hours: i64) {
        let ended = artisan_api::now() - hours * 3_600_000;
        let run = Run {
            id: RunId::new(),
            thread_id: thread.clone(),
            engine: EngineId::Claude,
            model: None,
            input_cutoff: 0,
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
    pub(crate) fn wait(
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
        let activity = setup
            .forge
            .block_on(setup.store.thread(&answered))
            .unwrap()
            .unwrap()
            .activity_seq;
        setup.lists(cx) == (vec![answered.clone(), quiet.clone()], vec![])
            && setup.core.read_with(cx, |core, cx| {
                core.active_host()
                    .unwrap()
                    .read(cx)
                    .threads
                    .get(&answered)
                    .is_some_and(|thread| thread.activity_seq == activity)
            })
    });

    // Sidebar navigation does not acknowledge unseen content.
    setup
        .window
        .update(cx, |sidebar, _, cx| {
            sidebar.open_thread(answered.clone(), cx)
        })
        .unwrap();
    let unread = setup
        .forge
        .block_on(setup.store.thread(&answered))
        .unwrap()
        .unwrap();
    assert!(unread.read_seq < unread.activity_seq);
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
    assert_eq!(
        setup.lists(cx).0,
        vec![answered.clone(), quiet.clone()],
        "navigation keeps its place until it is settled"
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

#[gpui::test]
fn collapsed_history_has_no_retained_rows_and_expanding_renders_a_window(cx: &mut TestAppContext) {
    let setup = Setup::new(cx);
    setup
        .window
        .update(cx, |sidebar, _, cx| {
            sidebar._host = None;
            sidebar.host = None;
            let rows = (0..10_000)
                .map(|index| crate::list::RowData {
                    id: ThreadId::from(format!("history-{index}")),
                    title: format!("Thread {index}").into(),
                    subtitle: "Project".into(),
                    project: setup.project.clone(),
                    state: crate::list::RowState::Idle,
                    selected: false,
                    settleable: false,
                })
                .collect();
            sidebar.lists = crate::list::Lists {
                inbox: Vec::new(),
                history: vec![crate::list::HistoryGroup {
                    age: crate::list::AgeGroup::LastDay,
                    rows,
                }],
                next_tick: None,
            };
            sidebar.rebuild_entries();
            assert_eq!(
                sidebar
                    .entries
                    .iter()
                    .filter(|entry| matches!(entry, crate::Entry::Thread(_)))
                    .count(),
                0,
                "collapsed history retains no thread rows"
            );
            cx.notify();
        })
        .unwrap();
    let mut cx = VisualTestContext::from_window(setup.window.into(), cx);
    cx.run_until_parked();
    let history = cx.debug_bounds("sidebar-history").unwrap();
    cx.simulate_click(history.center(), Modifiers::none());
    cx.run_until_parked();
    assert!(cx.debug_bounds("sidebar-thread-history-0").is_some());
    assert!(
        cx.debug_bounds("sidebar-thread-history-9999").is_none(),
        "offscreen rows are not constructed"
    );
    setup
        .window
        .update(&mut cx, |sidebar, _, _| {
            assert!(sidebar.icon_tasks.len() <= 4);
            sidebar.threads_list.scroll_to(gpui::ListOffset {
                item_ix: 100,
                offset_in_item: gpui::px(5.0),
            });
            let anchor = sidebar.threads_list.logical_scroll_top();
            sidebar.lists.history[0].rows[100].selected = true;
            sidebar.rebuild_entries();
            assert_eq!(
                (
                    sidebar.threads_list.logical_scroll_top().item_ix,
                    sidebar.threads_list.logical_scroll_top().offset_in_item
                ),
                (anchor.item_ix, anchor.offset_in_item),
                "changing selection must preserve the native list and its History anchor"
            );
            assert_eq!(
                sidebar
                    .entries
                    .iter()
                    .filter(|entry| matches!(entry, crate::Entry::History))
                    .count(),
                1
            );
        })
        .unwrap();
}

/// A working thread's dot breathes on the GPU, so the sidebar stops asking
/// for frames: a running agent must not keep a CPU core busy redrawing it.
#[gpui::test]
fn a_working_thread_breathes_without_redrawing_the_sidebar(cx: &mut TestAppContext) {
    let setup = Setup::new(cx);
    setup
        .window
        .update(cx, |sidebar, _, cx| {
            sidebar._host = None;
            sidebar.host = None;
            sidebar.lists = crate::list::Lists {
                inbox: vec![crate::list::RowData {
                    id: ThreadId::from("working".to_owned()),
                    title: "Working thread".into(),
                    subtitle: "Project".into(),
                    project: setup.project.clone(),
                    state: crate::list::RowState::Working,
                    selected: false,
                    settleable: false,
                }],
                history: Vec::new(),
                next_tick: None,
            };
            sidebar.rebuild_entries();
            cx.notify();
        })
        .unwrap();
    let mut cx = VisualTestContext::from_window(setup.window.into(), cx);
    cx.run_until_parked();
    assert!(cx.debug_bounds("sidebar-thread-working").is_some());
    // Outlast the sidebar's opening transitions, which run on the wall clock,
    // then let a few frames run: an endless animation would keep asking.
    std::thread::sleep(Duration::from_millis(400));
    for _ in 0..3 {
        cx.simulate_frame(setup.window.into());
        cx.run_until_parked();
    }
    cx.update(|window, _| {
        assert!(
            !window.awaits_next_frame(),
            "the working dot must animate without asking for new frames"
        );
    });
}

#[gpui::test]
fn summary_pages_evict_history_and_direct_navigation_keeps_its_metadata(cx: &mut TestAppContext) {
    let setup = Setup::new(cx);
    let ids: Vec<_> = (0..650).map(|_| setup.thread(None)).collect();
    let oldest = ids.first().unwrap().clone();
    let newest = ids.last().unwrap().clone();
    setup.wait(cx, "latest metadata is applied", |setup, cx| {
        setup.core.read_with(cx, |core, cx| {
            core.active_host()
                .unwrap()
                .read(cx)
                .threads
                .contains_key(&newest)
        })
    });
    let host = setup
        .core
        .read_with(cx, |core, _| core.active_host().unwrap().clone());
    host.update(cx, |host, cx| {
        host.load_threads(false, editor_core::ThreadPageDirection::Latest, cx)
    });
    setup.wait(cx, "latest page completes", |_, cx| {
        host.read_with(cx, |host, _| !host.thread_page(false).loading)
    });
    host.read_with(cx, |host, _| {
        assert_eq!(host.thread_page(false).ids.len(), 200);
        assert_eq!(host.threads.len(), 200);
        assert!(!host.threads.contains_key(&oldest));
    });
    for expected in [400, 600, 600] {
        host.update(cx, |host, cx| {
            host.load_threads(false, editor_core::ThreadPageDirection::Older, cx)
        });
        setup.wait(cx, "older page completes", |_, cx| {
            host.read_with(cx, |host, _| !host.thread_page(false).loading)
        });
        host.read_with(cx, |host, _| {
            assert_eq!(host.thread_page(false).ids.len(), expected)
        });
    }
    host.read_with(cx, |host, _| {
        assert!(!host.thread_page(false).at_latest);
        assert!(host.thread_page(false).next.is_none());
        assert_eq!(host.threads.len(), 600);
        assert!(!host.threads.contains_key(&newest));
    });
    let host_id = host.read_with(cx, |host, _| host.id.clone());
    setup.core.update(cx, |core, cx| {
        core.navigate(
            Screen::Thread {
                host: host_id,
                thread: newest.clone(),
            },
            cx,
        )
    });
    setup.wait(cx, "direct navigation loads evicted metadata", |_, cx| {
        host.read_with(cx, |host, _| host.threads.contains_key(&newest))
    });
    host.update(cx, |host, cx| {
        host.load_threads(false, editor_core::ThreadPageDirection::Older, cx)
    });
    host.read_with(cx, |host, _| assert!(host.threads.contains_key(&newest)));
    setup
        .forge
        .block_on(setup.store.update_thread(
            &oldest,
            artisan_api::UpdateThread {
                archived: Some(true),
                ..Default::default()
            },
        ))
        .unwrap();
    host.update(cx, |host, cx| {
        host.load_threads(true, editor_core::ThreadPageDirection::Latest, cx)
    });
    setup.wait(cx, "archive is loaded separately", |_, cx| {
        host.read_with(cx, |host, _| {
            !host.thread_page(true).loading && host.thread_page(true).ids.contains(&oldest)
        })
    });
    host.read_with(cx, |host, _| {
        assert_eq!(host.thread_page(true).ids, vec![oldest.clone()]);
        assert!(!host.thread_page(false).ids.contains(&oldest));
        assert!(
            host.threads.contains_key(&newest),
            "direct navigation remains pinned across archive loads"
        );
    });
}

#[test]
fn avatar_cache_expires_success_and_missing_results() {
    let now = Instant::now();
    let mut entry = crate::IconEntry {
        project_seq: 1,
        hash: Some(artisan_api::BlobHash("image".into())),
        failed: false,
        checked: now - Duration::from_secs(299),
    };
    assert!(entry.fresh(now));
    assert!(!entry.fresh(now + Duration::from_secs(1)));
    entry.hash = None;
    entry.checked = now - Duration::from_secs(4);
    assert!(entry.fresh(now));
    assert!(!entry.fresh(now + Duration::from_secs(1)));
    entry.hash = Some(artisan_api::BlobHash("last-good".into()));
    entry.failed = true;
    assert!(
        !entry.fresh(now + Duration::from_secs(1)),
        "failed refresh uses failure TTL even with retained pixels"
    );
}

#[gpui::test]
fn project_metadata_replacement_releases_icons_and_the_old_request(cx: &mut TestAppContext) {
    let setup = Setup::new(cx);
    setup.wait(cx, "project is observed", |setup, cx| {
        setup.core.read_with(cx, |core, cx| {
            core.active_host()
                .unwrap()
                .read(cx)
                .projects
                .contains_key(&setup.project)
        })
    });
    setup
        .window
        .update(cx, |sidebar, _, cx| {
            let host = sidebar.host.clone().unwrap();
            let old = host.read(cx).projects[&setup.project].seq;
            sidebar.icons.insert(
                setup.project.clone(),
                crate::IconEntry {
                    project_seq: old,
                    hash: Some(artisan_api::BlobHash("old-icon".into())),
                    failed: false,
                    checked: Instant::now(),
                },
            );
            sidebar.icon_order.push_back(setup.project.clone());
            sidebar.icon_tasks.insert(
                setup.project.clone(),
                crate::IconRequest {
                    project_seq: old,
                    _task: gpui::Task::ready(()),
                },
            );
            host.update(cx, |host, _| {
                host.projects.get_mut(&setup.project).unwrap().seq = old + 1
            });
            sidebar.reconcile_icons(cx);
            assert!(!sidebar.icons.contains_key(&setup.project));
            assert!(!sidebar.icon_tasks.contains_key(&setup.project));
            assert!(sidebar.icon_order.is_empty());
            // An expired missing avatar is retried once when this project is visible.
            sidebar.icons.insert(
                setup.project.clone(),
                crate::IconEntry {
                    project_seq: old + 1,
                    hash: None,
                    failed: false,
                    checked: Instant::now() - Duration::from_secs(6),
                },
            );
            sidebar.load_visible_projects(&[setup.project.clone(), setup.project.clone()], cx);
            assert_eq!(sidebar.icon_tasks.len(), 1);
            assert_eq!(sidebar.icon_tasks[&setup.project].project_seq, old + 1);
            assert_eq!(sidebar.icon_order.len(), 1);
            sidebar.clear_icons();
            assert!(
                sidebar.icons.is_empty()
                    && sidebar.icon_tasks.is_empty()
                    && sidebar.icon_order.is_empty()
            );
        })
        .unwrap();
}

#[gpui::test]
fn metadata_and_read_updates_do_not_reset_history_age(cx: &mut TestAppContext) {
    let setup = Setup::new(cx);
    let id = setup.thread(None);
    setup.wait(cx, "the idle thread to arrive", |setup, cx| {
        setup.lists(cx).0.contains(&id)
    });
    let host = setup
        .core
        .read_with(cx, |core, _| core.active_host().unwrap().clone());
    host.update(cx, |host, _| {
        let now = artisan_api::now();
        let thread = host.threads.get_mut(&id).unwrap();
        thread.activity_at = now - 2 * 24 * 60 * 60 * 1000;
        thread.updated_at = thread.activity_at;
        thread.read_seq = thread.activity_seq;
        thread.marked_unread = false;
        let before = crate::list::lists(host, None, now);
        assert!(before.inbox.is_empty());
        assert_eq!(before.history[0].age, crate::list::AgeGroup::LastThreeDays);
        let thread = host.threads.get_mut(&id).unwrap();
        thread.title = Some("Renamed after reading".into());
        thread.updated_at = now;
        let after = crate::list::lists(host, Some(&id), now);
        assert!(after.inbox.is_empty());
        assert_eq!(after.history[0].age, before.history[0].age);
        assert_eq!(after.next_tick, before.next_tick);
        assert_eq!(
            after.history[0].rows[0].title.as_ref(),
            "Renamed after reading"
        );
        assert!(after.history[0].rows[0].selected);
    });
}

#[gpui::test]
fn delayed_settle_failure_preserves_new_activity_and_independent_dialog(cx: &mut TestAppContext) {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let setup = Setup::new(cx);
    let first = setup.thread(None);
    let second = setup.thread(None);
    setup.reply(&first);
    setup.reply(&second);
    setup.wait(cx, "both authoritative activities", |setup, cx| {
        setup.core.read_with(cx, |core, cx| {
            let host = core.active_host().unwrap().read(cx);
            [&first, &second].into_iter().all(|id| {
                let stored = setup
                    .forge
                    .block_on(setup.store.thread(id))
                    .unwrap()
                    .unwrap();
                host.threads
                    .get(id)
                    .is_some_and(|thread| thread.activity_seq == stored.activity_seq)
            })
        })
    });
    let arrived = Arc::new(AtomicBool::new(false));
    let (release, receive) = tokio::sync::oneshot::channel();
    setup
        .window
        .update(cx, |sidebar, _, cx| {
            sidebar.call_completion_gate = Some((arrived.clone(), receive));
            sidebar.settle(&first, cx);
        })
        .unwrap();
    setup.wait(
        cx,
        "first settle committed before its acknowledgement",
        |_, _| arrived.load(Ordering::Acquire),
    );
    setup.reply(&first);
    setup.wait(
        cx,
        "new first-thread activity is authoritative",
        |setup, cx| {
            let current = setup
                .forge
                .block_on(setup.store.thread(&first))
                .unwrap()
                .unwrap();
            current.activity_seq > current.settled_seq
                && setup.core.read_with(cx, |core, cx| {
                    core.active_host()
                        .unwrap()
                        .read(cx)
                        .threads
                        .get(&first)
                        .is_some_and(|thread| thread.activity_seq == current.activity_seq)
                })
        },
    );
    setup
        .window
        .update(cx, |sidebar, window, cx| {
            sidebar.settle(&second, cx);
            sidebar.start_rename(second.clone(), window, cx);
            sidebar
                .rename
                .as_ref()
                .unwrap()
                .editor
                .update(cx, |editor, cx| editor.set_text("independent draft", cx));
        })
        .unwrap();
    setup.wait(cx, "independent settle completes", |setup, cx| {
        let lists = setup.lists(cx);
        lists.0.contains(&first)
            && lists.1.contains(&second)
            && setup
                .window
                .read_with(cx, |sidebar, _| sidebar.call_completions == 1)
                .unwrap()
    });
    let editor = setup
        .window
        .read_with(cx, |sidebar, _| {
            sidebar.rename.as_ref().unwrap().editor.clone()
        })
        .unwrap();
    release.send(true).unwrap();
    setup.wait(cx, "delayed failure is delivered", |setup, cx| {
        setup
            .window
            .read_with(cx, |sidebar, _| {
                sidebar.call_completions == 2 && sidebar.error.is_some()
            })
            .unwrap()
    });
    assert!(
        setup.lists(cx).0.contains(&first),
        "old acknowledgement cannot settle newer activity"
    );
    assert!(
        setup.lists(cx).1.contains(&second),
        "independent settlement remains authoritative"
    );
    assert_eq!(
        setup
            .window
            .read_with(cx, |sidebar, _| sidebar
                .rename
                .as_ref()
                .unwrap()
                .editor
                .entity_id())
            .unwrap(),
        editor.entity_id()
    );
    assert_eq!(
        editor.read_with(cx, |editor, _| editor.text().to_owned()),
        "independent draft"
    );
    setup
        .window
        .update(cx, |sidebar, _, cx| sidebar.settle(&first, cx))
        .unwrap();
    setup.wait(
        cx,
        "new activity can be settled after the failed acknowledgement",
        |setup, cx| {
            let lists = setup.lists(cx);
            !lists.0.contains(&first) && lists.1.contains(&first) && lists.1.contains(&second)
        },
    );
}

#[gpui::test]
fn host_switch_preserves_new_rename_dialog_after_old_rename_failure(cx: &mut TestAppContext) {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let setup = Setup::new(cx);
    let thread = setup.thread(None);
    setup.wait(cx, "first thread", |setup, cx| {
        setup.lists(cx).0.contains(&thread)
    });
    let second_dir = setup._dir.path().join("second-forge");
    let server = setup
        .forge
        .block_on(forge_http::start(forge_http::ServeConfig {
            data_dir: second_dir,
            listen: "127.0.0.1:0".parse().unwrap(),
        }))
        .unwrap();
    let address = server.local_addr().unwrap().to_string();
    let other_store = server.store().clone();
    setup.forge.spawn(server.run());
    let project = setup
        .forge
        .block_on(other_store.create_project(CreateProject {
            id: ProjectId::new(),
            path: setup._dir.path().display().to_string(),
            name: None,
        }))
        .unwrap()
        .id;
    setup
        .forge
        .block_on(other_store.create_thread(CreateThread {
            id: thread.clone(),
            project_id: project,
            config: EngineConfig::new(EngineId::Claude),
            title: Some("other host title".into()),
            parent_thread_id: None,
        }))
        .unwrap();
    let (_, other_host) = forge_fixture::pair(
        &other_store,
        &setup.forge,
        &address,
        &setup._dir.path().join("editor"),
    );
    let arrived = Arc::new(AtomicBool::new(false));
    let (release, receive) = tokio::sync::oneshot::channel();
    setup
        .window
        .update(cx, |sidebar, window, cx| {
            sidebar.call_completion_gate = Some((arrived.clone(), receive));
            sidebar.start_rename(thread.clone(), window, cx);
        })
        .unwrap();
    {
        let mut visual = VisualTestContext::from_window(setup.window.into(), cx);
        visual.simulate_input("accepted old host title");
        visual.simulate_keystrokes("enter");
    }
    setup.wait(cx, "old rename completed remotely", |_, _| {
        arrived.load(Ordering::Acquire)
    });
    assert_eq!(
        setup
            .forge
            .block_on(setup.store.thread(&thread))
            .unwrap()
            .unwrap()
            .title
            .as_deref(),
        Some("accepted old host title")
    );
    setup.core.update(cx, |core, cx| {
        core.add_host(other_host.clone(), cx).unwrap();
        core.navigate(
            Screen::Thread {
                host: other_host.id.clone(),
                thread: thread.clone(),
            },
            cx,
        );
    });
    setup.wait(cx, "second host metadata", |setup, cx| {
        setup
            .window
            .read_with(cx, |sidebar, cx| {
                sidebar.host.as_ref().is_some_and(|host| {
                    host.read(cx).id == other_host.id && host.read(cx).threads.contains_key(&thread)
                })
            })
            .unwrap()
    });
    setup
        .window
        .update(cx, |sidebar, window, cx| {
            sidebar.start_rename(thread.clone(), window, cx);
            sidebar
                .rename
                .as_ref()
                .unwrap()
                .editor
                .update(cx, |editor, cx| editor.set_text("new host draft", cx));
            sidebar.error = Some("new host error".into());
        })
        .unwrap();
    let new_editor = setup
        .window
        .read_with(cx, |sidebar, _| {
            sidebar.rename.as_ref().unwrap().editor.clone()
        })
        .unwrap();
    release.send(true).unwrap();
    setup.wait(cx, "old callback rejected", |setup, cx| {
        setup
            .window
            .read_with(cx, |sidebar, _| sidebar.call_completions == 1)
            .unwrap()
    });
    assert_eq!(
        setup
            .window
            .read_with(cx, |sidebar, _| sidebar.error.clone())
            .unwrap(),
        Some("new host error".into())
    );
    assert_eq!(
        setup
            .window
            .read_with(cx, |sidebar, _| sidebar
                .rename
                .as_ref()
                .unwrap()
                .editor
                .entity_id())
            .unwrap(),
        new_editor.entity_id()
    );
    assert_eq!(
        new_editor.read_with(cx, |editor, _| editor.text().to_owned()),
        "new host draft"
    );
    assert_eq!(
        setup
            .forge
            .block_on(other_store.thread(&thread))
            .unwrap()
            .unwrap()
            .title
            .as_deref(),
        Some("other host title")
    );
}

#[gpui::test]
fn thread_rows_open_and_offer_actions_from_the_keyboard(cx: &mut TestAppContext) {
    let setup = Setup::new(cx);
    let thread = setup.thread(None);
    setup.wait(cx, "thread in the inbox", |setup, cx| {
        setup.lists(cx).0.contains(&thread)
    });
    let mut visual = VisualTestContext::from_window(setup.window.into(), cx);
    visual.run_until_parked();
    let selector: &'static str = format!("sidebar-thread-{thread}").leak();
    let row = visual.debug_bounds(selector).expect("mounted thread row");
    visual.simulate_click(row.center(), Modifiers::none());
    visual.run_until_parked();
    let focus = visual.update(|window, cx| {
        window
            .focused(cx)
            .expect("thread row receives native focus")
    });
    setup.core.update(&mut visual, |core, cx| {
        core.navigate(Screen::Marketplace, cx)
    });
    visual.run_until_parked();
    visual.update(|window, _| {
        assert!(
            focus.is_focused(window),
            "the actual thread-row focus survives navigation"
        )
    });
    let press = |visual: &mut VisualTestContext, key: &str| {
        visual.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        let keystroke = gpui::Keystroke::parse(key).expect("valid test key");
        visual.update(|window, cx| {
            window.dispatch_event(
                gpui::PlatformInput::KeyDown(gpui::KeyDownEvent {
                    keystroke: keystroke.clone(),
                    is_held: false,
                    prefer_character_input: false,
                }),
                cx,
            );
            window.dispatch_event(
                gpui::PlatformInput::KeyUp(gpui::KeyUpEvent { keystroke }),
                cx,
            );
        });
        visual.run_until_parked();
    };
    press(&mut visual, "enter");
    assert!(setup.core.read_with(&visual, |core, _| matches!(core.screen(), Screen::Thread { thread: open, .. } if open == &thread)), "Enter opens the focused thread row");
    press(&mut visual, "shift-f10");
    assert!(
        visual.debug_bounds("sidebar-menu-rename").is_some(),
        "Shift+F10 opens the focused thread's actions"
    );
}

/// An empty pagination state must not reserve a flex gap below the account.
/// The profile menu floats over thread rows; a click on the menu must stay
/// on the menu instead of opening the thread beneath it.
#[gpui::test]
fn clicks_on_the_profile_menu_never_open_the_thread_beneath(cx: &mut TestAppContext) {
    let setup = Setup::new(cx);
    let threads: Vec<_> = (0..12).map(|_| setup.thread(None)).collect();
    setup.wait(cx, "threads in the inbox", |setup, cx| {
        threads.iter().all(|thread| setup.lists(cx).0.contains(thread))
    });
    let mut visual = VisualTestContext::from_window(setup.window.into(), cx);
    visual.simulate_resize(gpui::size(gpui::px(360.0), gpui::px(600.0)));
    visual.run_until_parked();
    let trigger = visual
        .debug_bounds("artisan-desktop-profile-trigger")
        .expect("profile footer");
    visual.simulate_click(trigger.center(), Modifiers::none());
    visual.run_until_parked();
    // Let the opening scale finish so the menu has its full bounds.
    std::thread::sleep(Duration::from_millis(400));
    for _ in 0..3 {
        visual.simulate_frame(setup.window.into());
        visual.run_until_parked();
    }
    let menu = visual
        .debug_bounds("artisan-desktop-profile-menu")
        .expect("profile menu is open");
    let (thread, point) = threads
        .iter()
        .find_map(|thread| {
            let selector: &'static str = format!("sidebar-thread-{thread}").leak();
            let row = visual.debug_bounds(selector)?;
            let overlap = row.intersect(&menu);
            (overlap.size.width > gpui::px(0.0) && overlap.size.height > gpui::px(0.0))
                .then(|| (thread.clone(), overlap.center()))
        })
        .expect("a thread row lies beneath the profile menu");
    visual.simulate_click(point, Modifiers::none());
    visual.run_until_parked();
    let opened = setup.core.read_with(&visual, |core, _| {
        matches!(core.screen(), Screen::Thread { thread: open, .. } if *open == thread)
    });
    assert!(!opened, "the click went through the profile menu to {thread}");
}

#[gpui::test]
fn profile_footer_keeps_only_the_sidebar_bottom_inset(cx: &mut TestAppContext) {
    let setup = Setup::new(cx);
    let thread = setup.thread(None);
    setup.wait(cx, "thread in the inbox", |setup, cx| {
        setup.lists(cx).0.contains(&thread)
    });
    let mut visual = VisualTestContext::from_window(setup.window.into(), cx);
    visual.simulate_resize(gpui::size(gpui::px(360.0), gpui::px(600.0)));
    visual.run_until_parked();
    let footer = visual
        .debug_bounds("artisan-desktop-profile-trigger")
        .expect("profile footer");
    assert_eq!(footer.bottom(), gpui::px(590.0));
}
