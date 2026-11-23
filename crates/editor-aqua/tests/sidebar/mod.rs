use artisan_api::{
    DirListing, EngineConfig, EngineId, Project, ProjectId, RunStatus, Thread, ThreadId,
};
use editor_aqua::sidebar::geometry::{
    HistoryPane, PageDirection, PageLoad, PageState, PaneDistances, page_direction,
};
use editor_aqua::sidebar::list::{AgeGroup, HostSnapshot, RowState, lists};
use editor_aqua::sidebar::picker::FolderPicker;

struct Host {
    project: Project,
    threads: Vec<Thread>,
    waiting: Vec<ThreadId>,
    working: Vec<ThreadId>,
}

#[test]
fn production_thread_visuals_ellipsize_and_scope_working_pulse() {
    use aqua::{UiApp, prelude::DisplayItem};
    use aqua_text::{RunId, SharedFonts, SharedRuns};
    use aqua_ui::element::Box as UiBox;
    use editor_aqua::sidebar::{controller::Sidebar, list::RowData};
    use editor_aqua::ui::theme::ArtisanTheme;

    let sidebar = Sidebar::new(true);
    let row = RowData {
        id: ThreadId::new(),
        title: "A long conversation title that should be shortened for the sidebar".into(),
        subtitle: "project".into(),
        project: ProjectId::new(),
        state: RowState::Working,
        selected: false,
        settleable: false,
        matched: Vec::new(),
    };
    let factory = sidebar.thread_visual_factory(&ArtisanTheme::resolved(), false);
    let visuals = factory(&row, None, editor_ui::project_mark::Forge::Folder);
    let runs = SharedRuns::new();
    let fonts = SharedFonts::new();
    editor_aqua::ui::fonts::register_bundled_fonts(&fonts);
    let mut app = UiApp::new(fonts, runs.clone());
    app.mount(|_| {
        UiBox::new()
            .width(90.0)
            .layout("column")
            .child(visuals.title)
            .child(visuals.dot.unwrap())
            .build()
    })
    .unwrap();
    app.set_viewport(90.0, 100.0, 1.0);
    app.build_frame().unwrap();
    let run = app
        .scene()
        .ordered()
        .find_map(|(_, item)| {
            if let DisplayItem::Text { run, .. } = item {
                Some(run.clone())
            } else {
                None
            }
        })
        .unwrap();
    let registry = runs.lock();
    let layout = registry
        .resolve_expected(RunId(run.id), run.revision)
        .unwrap()
        .layout;
    assert!(layout.display_text().ends_with('…'));
    assert_eq!(layout.source_len(), row.title.len());
    drop(registry);
    let scene = app.tree_mut().emit_scene();
    assert!(
        scene
            .iter()
            .any(|(_, item)| matches!(item, DisplayItem::PushGpuiQuadPulse(_)))
    );
    assert!(
        scene
            .iter()
            .any(|(_, item)| matches!(item, DisplayItem::PopGpuiQuadPulse))
    );

    let reduced = sidebar.thread_visual_factory(&ArtisanTheme::resolved(), true)(
        &row,
        None,
        editor_ui::project_mark::Forge::Folder,
    );
    let mut reduced_app = UiApp::new(SharedFonts::new(), SharedRuns::new());
    reduced_app
        .mount(|_| UiBox::new().child(reduced.dot.unwrap()).build())
        .unwrap();
    reduced_app.set_viewport(90.0, 100.0, 1.0);
    reduced_app.build_frame().unwrap();
    assert!(
        !reduced_app
            .tree_mut()
            .emit_scene()
            .iter()
            .any(|(_, item)| matches!(
                item,
                DisplayItem::PushGpuiQuadPulse(_) | DisplayItem::PopGpuiQuadPulse
            ))
    );
}

#[test]
fn dispatch_reports_visible_changes_and_ignores_idempotent_actions() {
    use editor_aqua::sidebar::{
        controller::Sidebar,
        view::{ViewAction, dispatch},
    };

    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(&directory.path().join("editor"));
    let mut sidebar = Sidebar::new(true);
    sidebar.sync(&core, 0);
    assert!(!dispatch(&mut sidebar, &mut core, ViewAction::DismissError));
    assert!(!dispatch(
        &mut sidebar,
        &mut core,
        ViewAction::NewThread(None)
    ));
    assert!(!sidebar.drain(&mut core, 0));
}

#[test]
fn history_motion_runs_to_its_end_and_reports_one_final_rebuild() {
    use editor_aqua::sidebar::controller::Sidebar;
    use std::time::{Duration, Instant};

    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(&directory.path().join("editor"));
    let mut sidebar = Sidebar::new(true);
    let start = Instant::now();
    assert!(!sidebar.advance_motion(start, false));
    sidebar.toggle_history(&mut core);
    assert!(sidebar.history_visible());
    assert!(sidebar.advance_motion(start + Duration::from_millis(100), false));
    assert!(sidebar.history_fraction > 0.0 && sidebar.history_fraction < 1.0);
    assert!(sidebar.advance_motion(start + Duration::from_secs(2), false));
    test_support::assert_near!(sidebar.history_fraction, 1.0);
    assert!(sidebar.advance_motion(start + Duration::from_secs(3), false));
    assert!(!sidebar.advance_motion(start + Duration::from_secs(4), false));
    sidebar.toggle_history(&mut core);
    assert!(sidebar.advance_motion(start + Duration::from_secs(10), true));
    test_support::assert_near!(sidebar.history_fraction, 0.0);
    assert!(sidebar.advance_motion(start + Duration::from_secs(11), true));
    assert!(!sidebar.advance_motion(start + Duration::from_secs(12), true));
    assert!(!sidebar.history_visible());
}

impl HostSnapshot for Host {
    fn threads(&self) -> Vec<&Thread> {
        self.threads.iter().collect()
    }
    fn project(&self, id: &ProjectId) -> Option<&Project> {
        (&self.project.id == id).then_some(&self.project)
    }
    fn repository_url(&self, _: &ProjectId) -> Option<&str> {
        Some("https://github.com/owner/repository/")
    }
    fn is_waiting(&self, id: &ThreadId) -> bool {
        self.waiting.contains(id)
    }
    fn is_working(&self, id: &ThreadId) -> bool {
        self.working.contains(id)
    }
    fn run_status(&self, _: &ThreadId) -> Option<RunStatus> {
        Some(RunStatus::Failed)
    }
}

fn thread(project: &ProjectId, seq: u64, age: i64) -> Thread {
    Thread {
        id: ThreadId::new(),
        project_id: project.clone(),
        title: Some(format!("thread {seq}")),
        config: EngineConfig::new(EngineId::Claude),
        activity_seq: seq,
        activity_at: 1_000_000_000 - age,
        read_seq: seq,
        marked_unread: false,
        settled_seq: 0,
        seq,
        created_at: 0,
        updated_at: 0,
        archived: false,
        parent_thread_id: None,
        spawned_by: None,
        parked_until: None,
    }
}

#[test]
fn projection_prioritizes_live_state_and_preserves_history_age() {
    let project = Project {
        id: ProjectId::new(),
        path: "/work".into(),
        name: "work".into(),
        seq: 0,
        created_at: 0,
        removed: false,
    };
    let day = 86_400_000;
    let mut waiting = thread(&project.id, 5, day * 2);
    waiting.parked_until = Some(1_000_000_001);
    waiting.marked_unread = true;
    let mut settled = thread(&project.id, 4, 0);
    settled.settled_seq = 4;
    let older = thread(&project.id, 3, day * 2);
    let live = waiting.id.clone();
    let host = Host {
        project,
        threads: vec![older, settled, waiting],
        waiting: vec![live.clone()],
        working: vec![live],
    };
    let result = lists(&host, None, 1_000_000_000);
    assert_eq!(result.inbox.len(), 1);
    assert_eq!(result.inbox[0].state, RowState::Waiting);
    assert_eq!(result.inbox[0].subtitle, "owner/repository");
    assert_eq!(result.history.len(), 2);
    assert_eq!(result.history[0].age, AgeGroup::LastDay);
    assert_eq!(result.history[1].age, AgeGroup::LastThreeDays);
    assert_eq!(result.next_tick, Some(1_000_000_000 + day));
}

#[test]
fn picker_retries_same_identity_until_successful_browse() {
    let mut picker = FolderPicker::default();
    assert!(picker.begin_browse());
    picker.finish_browse(Ok(DirListing {
        path: "/work".into(),
        parent: None,
        entries: vec![],
    }));
    let first = picker.begin_create().unwrap();
    assert!(!picker.begin_browse());
    picker.finish_create(Some(editor_aqua::core::Problem::new("connection lost")));
    assert_eq!(picker.begin_create().unwrap(), first);
    picker.finish_create(Some(editor_aqua::core::Problem::new("connection lost")));
    assert!(picker.begin_browse());
    picker.finish_browse(Ok(DirListing {
        path: "/other".into(),
        parent: None,
        entries: vec![],
    }));
    assert_ne!(picker.begin_create().unwrap().id, first.id);
}

#[test]
fn paging_retries_only_after_scroll_and_closed_history_stops_older_loading() {
    let mut distances = PaneDistances {
        viewport_height: 400.0,
        above: 700.0,
        below: 0.0,
    };
    let mut state = PageState {
        load: PageLoad::Failed { scrolled: false },
        at_latest: true,
        has_older: true,
        history: HistoryPane::Open,
    };
    assert_eq!(page_direction(distances, state), None);
    state.load = PageLoad::Failed { scrolled: true };
    assert_eq!(page_direction(distances, state), Some(PageDirection::Older));
    state.history = HistoryPane::Closed;
    assert_eq!(page_direction(distances, state), None);
    state.at_latest = false;
    distances.above = 0.0;
    assert_eq!(page_direction(distances, state), Some(PageDirection::Newer));
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one host-recreation scenario walked step by step"
)]
fn removed_and_recreated_host_rejects_delayed_sidebar_completions() {
    use editor_aqua::core::Screen;
    use editor_aqua::sidebar::controller::Sidebar;
    use test_support::editor::core::{Delivered, connect, stub_host};

    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(&directory.path().join("editor"));
    let forge = stub_forge();
    let config = stub_host("same-host", "Forge", &forge);
    let id = config.id.clone();
    connect(&mut core, &config);
    core.navigate(Screen::NewThread {
        host: id.clone(),
        project: None,
    });
    let first = core.host(&id).unwrap().instance();
    let mut sidebar = Sidebar::new(false);
    sidebar.sync(&core, 0);
    sidebar.add_project(&core);
    forge.take("GET", "/fs/list").respond(folder("/root"));
    drain_until(
        &mut sidebar,
        &mut core,
        "the first folder listing",
        |sidebar| {
            sidebar
                .picker
                .as_ref()
                .is_some_and(|picker| picker.listing.is_some())
        },
    );
    let create = add_this_folder(&sidebar, &core);
    editor_aqua::sidebar::view::dispatch(&mut sidebar, &mut core, create);
    assert!(sidebar.picker.as_ref().unwrap().creating);
    let late_create = forge.take("POST", "/projects");
    let old_project = Project {
        id: ProjectId::new(),
        path: "/old".into(),
        name: "old".into(),
        seq: 0,
        created_at: 0,
        removed: false,
    };
    core.remove_host(&id).unwrap();
    assert!(core.host_if_instance(&id, first).is_none());
    sidebar.drain(&mut core, 0);
    assert!(sidebar.error.is_none());
    assert!(sidebar.picker.is_none());
    connect(&mut core, &config);
    let replacement = core.host(&id).unwrap().instance();
    assert_ne!(first, replacement);
    core.navigate(Screen::NewThread {
        host: id.clone(),
        project: None,
    });
    sidebar.sync(&core, 0);
    assert!(sidebar.picker.is_none());
    let drag = editor_aqua::core::DraggedThread {
        host: id.clone(),
        thread: ThreadId::new(),
        title: "drag".into(),
    };
    assert!(!editor_aqua::sidebar::view::dispatch(
        &mut sidebar,
        &mut core,
        editor_aqua::sidebar::view::ViewAction::ThreadDragStarted {
            payload: drag.clone(),
            instance: first,
            position: aqua_core::Point::new(1.0, 2.0),
        }
    ));
    assert!(sidebar.drain_effects().is_empty());
    assert_eq!(sidebar.drag_instance(&core, &drag), None);
    assert!(!editor_aqua::sidebar::view::dispatch(
        &mut sidebar,
        &mut core,
        editor_aqua::sidebar::view::ViewAction::ThreadDragStarted {
            payload: drag.clone(),
            instance: replacement,
            position: aqua_core::Point::new(1.0, 2.0),
        }
    ));
    assert!(matches!(
        sidebar.drain_effects().as_slice(),
        [editor_aqua::sidebar::controller::SidebarEffect::ShowDragPreview { .. }]
    ));
    assert_eq!(sidebar.drag_instance(&core, &drag), Some(replacement));
    let mut other = drag.clone();
    other.thread = ThreadId::new();
    assert_eq!(sidebar.drag_instance(&core, &other), None);
    assert!(!editor_aqua::sidebar::view::dispatch(
        &mut sidebar,
        &mut core,
        editor_aqua::sidebar::view::ViewAction::ThreadDragEnded {
            host: id.clone(),
            instance: first,
            thread: drag.thread.clone(),
        }
    ));
    assert!(sidebar.drain_effects().is_empty());
    assert!(!editor_aqua::sidebar::view::dispatch(
        &mut sidebar,
        &mut core,
        editor_aqua::sidebar::view::ViewAction::ThreadDragEnded {
            host: id.clone(),
            instance: replacement,
            thread: drag.thread.clone(),
        }
    ));
    assert!(matches!(
        sidebar.drain_effects().as_slice(),
        [editor_aqua::sidebar::controller::SidebarEffect::HideDragPreview]
    ));
    assert_eq!(sidebar.drag_instance(&core, &drag), None);
    assert!(!editor_aqua::sidebar::view::dispatch(
        &mut sidebar,
        &mut core,
        editor_aqua::sidebar::view::ViewAction::OpenThread {
            host: id.clone(),
            instance: first,
            thread: ThreadId::new(),
        },
    ));
    sidebar.add_project(&core);
    forge.take("GET", "/fs/list").respond(folder("/new"));
    drain_until(
        &mut sidebar,
        &mut core,
        "the replacement's folder listing",
        |sidebar| {
            sidebar
                .picker
                .as_ref()
                .and_then(|picker| picker.listing.as_ref())
                .is_some_and(|listing| listing.path == "/new")
        },
    );
    let new_rename = ThreadId::new();
    sidebar.rename = Some(new_rename.clone());
    sidebar.rename_generation = 2;
    assert!(!editor_aqua::sidebar::view::dispatch(
        &mut sidebar,
        &mut core,
        editor_aqua::sidebar::view::ViewAction::DismissRenameFor {
            host: id.clone(),
            instance: replacement,
            thread: new_rename.clone(),
            generation: 1,
        },
    ));
    assert!(!editor_aqua::sidebar::view::dispatch(
        &mut sidebar,
        &mut core,
        editor_aqua::sidebar::view::ViewAction::DismissRenameFor {
            host: id.clone(),
            instance: first,
            thread: new_rename.clone(),
            generation: 2,
        },
    ));
    sidebar.error = Some(editor_aqua::core::Problem::new("new host error"));
    let delivered = Delivered::watch(sidebar.wake_mailbox().unwrap());
    late_create.respond(&old_project);
    delivered.wait("the removed host's project creation");
    sidebar.drain(&mut core, 0);
    assert_eq!(
        sidebar
            .error
            .as_ref()
            .map(editor_aqua::core::Problem::headline),
        Some("new host error")
    );
    assert_eq!(sidebar.rename.as_ref(), Some(&new_rename));
    assert!(sidebar.picker.is_some());
    assert_eq!(
        sidebar
            .picker
            .as_ref()
            .unwrap()
            .listing
            .as_ref()
            .unwrap()
            .path,
        "/new"
    );
    assert_eq!(
        core.screen(),
        &Screen::NewThread {
            host: id.clone(),
            project: None
        }
    );
    editor_aqua::sidebar::view::dispatch(
        &mut sidebar,
        &mut core,
        editor_aqua::sidebar::view::ViewAction::ThreadDragStarted {
            payload: drag.clone(),
            instance: replacement,
            position: aqua_core::Point::new(1.0, 2.0),
        },
    );
    assert_eq!(sidebar.drag_instance(&core, &drag), Some(replacement));
    core.remove_host(&id).unwrap();
    assert_eq!(sidebar.drag_instance(&core, &drag), None);
    connect(&mut core, &config);
    assert_ne!(core.host(&id).unwrap().instance(), replacement);
    assert_eq!(sidebar.drag_instance(&core, &drag), None);
}

#[test]
fn thread_menu_accepts_context_menu_key_and_shift_f10() {
    use aqua_core::{Key, KeyEvent, Modifiers, NamedKey};
    use editor_aqua::sidebar::row::opens_aqua_keyboard_menu;

    let mut event = KeyEvent {
        key: Key::Named(NamedKey::ContextMenu),
        modifiers: Modifiers::default(),
        repeat: false,
    };
    assert!(opens_aqua_keyboard_menu(&event));
    event.key = Key::Named(NamedKey::Function(10));
    assert!(!opens_aqua_keyboard_menu(&event));
    event.modifiers = Modifiers::SHIFT;
    assert!(opens_aqua_keyboard_menu(&event));
}

#[test]
fn avatar_hash_uses_utf16_and_dither_is_deterministic() {
    use editor_aqua::sidebar::avatar::{lit, seed_hash, svg};
    assert_eq!(seed_hash(""), 0x811c_9dc5);
    assert_eq!(seed_hash("a"), 0xe40c_292c);
    let utf16_hash = [0xd83d_u16, 0xde00]
        .into_iter()
        .fold(0x811c_9dc5_u32, |hash, unit| {
            (hash ^ u32::from(unit)).wrapping_mul(0x0100_0193)
        });
    assert_eq!(seed_hash("😀"), utf16_hash);
    assert_eq!(svg("😀"), svg("😀"));
    assert!(!lit(0, 15));
}

#[test]
fn rail_hover_does_not_restart_an_active_animation() {
    use editor_aqua::sidebar::rail::{Glyph, RailMotion};
    use std::time::{Duration, Instant};
    let start = Instant::now();
    let mut motion = RailMotion::default();
    assert!(motion.play(Glyph::Pen, start));
    assert!(!motion.play(Glyph::Pen, start + Duration::from_millis(500)));
    assert_eq!(
        motion.progress(Glyph::Pen, start + Duration::from_millis(500)),
        Some(0.5)
    );
    assert!(motion.animating(start + Duration::from_millis(500)));
    assert!(motion.play(Glyph::Pen, start + Duration::from_millis(1000)));
    assert_eq!(
        motion.progress(Glyph::Pen, start + Duration::from_millis(1000)),
        Some(0.0)
    );
}

mod spacer {
    use editor_aqua::sidebar::geometry::{Entry, history_spacer};
    use editor_aqua::ui::theme::ArtisanTheme;

    fn column() -> Vec<Entry> {
        vec![
            Entry::Gap(10.0),
            Entry::Spacer,
            Entry::History,
            Entry::Gap(2.0),
            Entry::Gap(30.0),
        ]
    }

    #[test]
    fn closed_history_fills_the_space_above_the_footer() {
        let spacer = history_spacer(&ArtisanTheme::resolved(), &column(), 500.0, 0.0);
        assert!(
            (spacer - (500.0 - 10.0 - Entry::History.height(&ArtisanTheme::resolved()))).abs()
                < 1e-4
        );
    }

    #[test]
    fn open_history_leaves_room_for_the_rows_below() {
        let spacer = history_spacer(&ArtisanTheme::resolved(), &column(), 500.0, 1.0);
        assert!(
            (spacer - (500.0 - 10.0 - Entry::History.height(&ArtisanTheme::resolved()) - 32.0))
                .abs()
                < 1e-4
        );
    }

    #[test]
    fn fraction_interpolates_between_closed_and_open() {
        let closed = history_spacer(&ArtisanTheme::resolved(), &column(), 500.0, 0.0);
        let open = history_spacer(&ArtisanTheme::resolved(), &column(), 500.0, 1.0);
        let half = history_spacer(&ArtisanTheme::resolved(), &column(), 500.0, 0.5);
        assert!((half - f32::midpoint(closed, open)).abs() < 1e-4);
    }
}

#[test]
fn settlement_moves_to_history_and_new_activity_returns_to_inbox() {
    let project = Project {
        id: ProjectId::new(),
        path: "/work".into(),
        name: "work".into(),
        seq: 0,
        created_at: 0,
        removed: false,
    };
    let record = thread(&project.id, 4, 0);
    let id = record.id.clone();
    let mut host = Host {
        project,
        threads: vec![record],
        waiting: vec![],
        working: vec![],
    };
    let initial = lists(&host, Some(&id), 1_000_000_000);
    assert!(initial.inbox[0].settleable);
    host.threads[0].settled_seq = 4;
    let settled = lists(&host, Some(&id), 1_000_000_000);
    assert!(settled.inbox.is_empty());
    assert!(settled.history[0].rows[0].selected);
    host.threads[0].activity_seq = 5;
    let resumed = lists(&host, Some(&id), 1_000_000_000);
    assert_eq!(resumed.inbox[0].id, id);
    assert!(resumed.history.is_empty());
    host.working.push(id);
    assert!(!lists(&host, None, 1_000_000_000).inbox[0].settleable);
}

#[test]
fn scrolling_history_to_its_end_loads_older_threads_after_the_inbox_reports_its_own_position() {
    use editor_aqua::sidebar::geometry::{PaneViewports, ThreadPane};

    let state = PageState {
        load: PageLoad::Ready,
        at_latest: true,
        has_older: true,
        history: HistoryPane::Open,
    };
    let mut viewports = PaneViewports::default();
    viewports.report(
        ThreadPane::History,
        PaneDistances {
            viewport_height: 300.0,
            above: 900.0,
            below: 0.0,
        },
    );
    viewports.report(
        ThreadPane::Inbox,
        PaneDistances {
            viewport_height: 400.0,
            above: 0.0,
            below: 2000.0,
        },
    );
    assert_eq!(
        viewports.direction(state),
        Some(PageDirection::Older),
        "the inbox's position must not replace where History was scrolled"
    );
    viewports.forget(ThreadPane::History);
    assert_eq!(viewports.direction(state), None);
}

#[test]
fn message_search_results_do_not_follow_a_switch_to_another_host() {
    use artisan_api::{ItemId, TextHit};
    use editor_aqua::core::Screen;
    use editor_aqua::sidebar::controller::Sidebar;
    use test_support::editor::core::{Delivered, stub_host};

    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(&directory.path().join("editor"));
    let first_forge = stub_forge();
    let second_forge = stub_forge();
    let first = stub_host("first", "first", &first_forge);
    let second = stub_host("second", "second", &second_forge);
    connect_live(&mut core, &first);
    connect_live(&mut core, &second);
    core.navigate(Screen::NewThread {
        host: first.id.clone(),
        project: None,
    });
    let mut sidebar = Sidebar::new(false);
    sidebar.sync(&core, 0);
    sidebar.search.borrow_mut().set_text("needle");
    sidebar.search_text(&core);
    let hit = |thread: &str| TextHit {
        thread_id: ThreadId(thread.into()),
        thread_title: Some("Found".into()),
        project_id: ProjectId::new(),
        item_id: ItemId(format!("{thread}-item")),
        position: 1,
        created_at: 0,
        snippet: "a needle here".into(),
        matched: (2, 8),
        offset: (0, 0),
        length: 6,
    };
    first_forge
        .take("GET", "/search/text")
        .respond(vec![hit("first-thread")]);
    drain_until(
        &mut sidebar,
        &mut core,
        "the first host's matches",
        |sidebar| !sidebar.lists.hits.is_empty(),
    );
    assert_eq!(sidebar.lists.hits.len(), 1);

    sidebar.search_text(&core);
    let delivered = Delivered::watch(sidebar.wake_mailbox().unwrap());
    first_forge
        .take("GET", "/search/text")
        .respond(vec![hit("late-first-thread")]);
    delivered.wait("the first host's late matches");

    core.navigate(Screen::NewThread {
        host: second.id.clone(),
        project: None,
    });
    sidebar.sync(&core, 0);
    assert!(
        sidebar.lists.hits.is_empty(),
        "the first host's matches must not be offered on the second host"
    );
    let on_second = second_forge.take("GET", "/search/text");
    assert!(
        on_second
            .query()
            .is_some_and(|query| query.contains("needle")),
        "the search restarts on the second host: {on_second:?}"
    );

    sidebar.drain(&mut core, 0);
    assert!(
        sidebar.lists.hits.is_empty(),
        "a late search from the first host must not land on the second"
    );
}

#[test]
fn a_rename_failure_from_an_earlier_visit_to_the_same_host_is_not_shown_after_returning() {
    use axum::http::StatusCode;
    use editor_aqua::core::Screen;
    use editor_aqua::sidebar::controller::Sidebar;
    use test_support::editor::core::{Delivered, stub_host};

    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(&directory.path().join("editor"));
    let first_forge = stub_forge();
    let second_forge = stub_forge();
    let first = stub_host("first", "first", &first_forge);
    let second = stub_host("second", "second", &second_forge);
    connect_live(&mut core, &first);
    connect_live(&mut core, &second);
    let visit = |core: &mut editor_aqua::core::Core, host: &editor_aqua::core::HostConfig| {
        core.navigate(Screen::NewThread {
            host: host.id.clone(),
            project: None,
        });
    };
    visit(&mut core, &first);
    let instance = core.host(&first.id).unwrap().instance();
    let mut sidebar = Sidebar::new(false);
    sidebar.sync(&core, 0);
    let thread = known_thread(&mut core, &first_forge, &first.id);
    sidebar.rename = Some(thread.clone());
    editor_aqua::sidebar::view::dispatch(
        &mut sidebar,
        &mut core,
        editor_aqua::sidebar::view::ViewAction::SubmitRename("Renamed".into()),
    );
    let late_rename = first_forge.take("PATCH", &format!("/threads/{thread}"));

    visit(&mut core, &second);
    sidebar.sync(&core, 0);
    visit(&mut core, &first);
    sidebar.sync(&core, 0);
    assert_eq!(core.host(&first.id).unwrap().instance(), instance);

    let delivered = Delivered::watch(sidebar.wake_mailbox().unwrap());
    late_rename.fail(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal",
        "rename refused",
    );
    test_support::wait::wait_until(
        "the host applies the failed rename",
        std::time::Duration::from_secs(10),
        || {
            core.drain_with(|_, _| {});
            core.host(&first.id)
                .is_some_and(|host| !host.has_requests_in_flight())
        },
    );
    delivered.wait("the first visit's rename failure");
    sidebar.drain(&mut core, 0);
    assert_eq!(
        sidebar.error, None,
        "a failure from before the host switch must not appear on the return visit"
    );
}

fn known_thread(
    core: &mut editor_aqua::core::Core,
    forge: &test_support::editor::forge::StubForge,
    host: &editor_aqua::core::HostId,
) -> ThreadId {
    let thread = thread(&ProjectId::new(), 1, 0);
    let id = thread.id.clone();
    forge.publish(artisan_api::Event::Thread { thread });
    test_support::wait::wait_until(
        "Forge's thread to reach the Editor",
        std::time::Duration::from_secs(10),
        || {
            core.drain_with(|_, _| {});
            core.host(host)
                .is_some_and(|data| data.threads.contains_key(&id))
        },
    );
    id
}

#[test]
fn a_rename_shows_in_the_sidebar_at_once_and_a_failure_restores_the_title() {
    use axum::http::StatusCode;
    use editor_aqua::core::Screen;
    use editor_aqua::sidebar::controller::Sidebar;
    use test_support::editor::core::stub_host;

    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(&directory.path().join("editor"));
    let forge = stub_forge();
    let config = stub_host("rename", "Forge", &forge);
    connect_live(&mut core, &config);
    let project = Project {
        id: ProjectId::new(),
        path: "/work".into(),
        name: "work".into(),
        seq: 1,
        created_at: 0,
        removed: false,
    };
    forge.publish(artisan_api::Event::Project {
        project: project.clone(),
    });
    let mut known = thread(&project.id, 2, 0);
    known.title = Some("Before".into());
    let id = known.id.clone();
    forge.publish(artisan_api::Event::Thread { thread: known });
    core.navigate(Screen::NewThread {
        host: config.id.clone(),
        project: None,
    });
    let mut sidebar = Sidebar::new(false);
    let row_title = |sidebar: &Sidebar| {
        sidebar
            .lists
            .inbox
            .iter()
            .find(|row| row.id == id)
            .map(|row| row.title.clone())
    };
    test_support::wait::wait_until(
        "the thread's row",
        std::time::Duration::from_secs(10),
        || {
            core.drain_with(|_, _| {});
            sidebar.drain(&mut core, 0);
            row_title(&sidebar).is_some()
        },
    );
    sidebar.rename = Some(id.clone());
    editor_aqua::sidebar::view::dispatch(
        &mut sidebar,
        &mut core,
        editor_aqua::sidebar::view::ViewAction::SubmitRename("After".into()),
    );
    sidebar.drain(&mut core, 0);
    assert_eq!(
        row_title(&sidebar).as_deref(),
        Some("After"),
        "the row shows the new title before Forge answers"
    );
    forge.take("PATCH", &format!("/threads/{id}")).fail(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal",
        "disk full",
    );
    test_support::wait::wait_until(
        "the rename failure",
        std::time::Duration::from_secs(10),
        || {
            core.drain_with(|_, _| {});
            sidebar.drain(&mut core, 0);
            sidebar.error.is_some()
        },
    );
    assert_eq!(
        row_title(&sidebar).as_deref(),
        Some("Before"),
        "the failed rename puts the old title back"
    );
    assert!(
        sidebar
            .error
            .as_ref()
            .is_some_and(|error| error.headline().starts_with("Couldn't rename this thread.")),
        "the sidebar says what failed: {:?}",
        sidebar.error
    );
}

fn stub_forge() -> test_support::editor::forge::StubForge {
    editor_aqua::core::runtime::runtime().block_on(test_support::editor::forge::StubForge::serve(
        test_support::editor::forge::empty_state(),
        axum::Router::new(),
    ))
}

fn connect_live(core: &mut editor_aqua::core::Core, config: &editor_aqua::core::HostConfig) {
    test_support::editor::core::connect(core, config);
    test_support::wait::wait_until(
        "the stub Forge to go live",
        std::time::Duration::from_secs(10),
        || {
            core.drain_with(|_, _| {});
            core.host(&config.id)
                .is_some_and(editor_aqua::core::HostData::is_live)
        },
    );
}

fn folder(path: &str) -> DirListing {
    DirListing {
        path: path.into(),
        parent: None,
        entries: vec![],
    }
}

fn drain_until(
    sidebar: &mut editor_aqua::sidebar::controller::Sidebar,
    core: &mut editor_aqua::core::Core,
    what: &str,
    done: impl Fn(&editor_aqua::sidebar::controller::Sidebar) -> bool,
) {
    test_support::wait::wait_until(what, std::time::Duration::from_secs(10), || {
        sidebar.drain(core, 0);
        done(sidebar)
    });
}

fn add_this_folder(
    sidebar: &editor_aqua::sidebar::controller::Sidebar,
    core: &editor_aqua::core::Core,
) -> editor_aqua::sidebar::view::ViewAction {
    use aqua_ui::element::Box as UiBox;
    use editor_aqua::sidebar::overlay::SidebarOverlays;
    use editor_aqua::sidebar::view::ViewAction;
    use editor_aqua::ui::theme::ArtisanTheme;
    use std::{cell::RefCell, rc::Rc};

    let emitted = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&emitted);
    let emit: Rc<dyn Fn(ViewAction)> = Rc::new(move |action| sink.borrow_mut().push(action));
    let fonts = aqua_text::SharedFonts::new();
    editor_aqua::ui::fonts::register_bundled_fonts(&fonts);
    let mut app = aqua::UiApp::new(fonts, aqua_text::SharedRuns::new());
    app.set_viewport(800.0, 600.0, 1.0);
    app.mount(|_| {
        UiBox::new()
            .width(800.0)
            .height(600.0)
            .build()
            .with_key("sidebar-owner")
    })
    .expect("mount the picker's owner");
    app.build_frame().expect("lay out the picker's owner");
    let owner = test_support::editor::ui::node_with_key(&app, "sidebar-owner");
    let mut overlays = SidebarOverlays::default();
    overlays
        .sync(
            &mut app,
            owner,
            sidebar,
            core,
            &ArtisanTheme::resolved(),
            0.0,
            &emit,
        )
        .expect("open the folder picker");
    app.build_frame().expect("lay out the folder picker");
    test_support::editor::ui::click(&mut app, "sidebar-folder-add");
    let action = emitted.borrow_mut().pop();
    match action {
        Some(action @ ViewAction::CreateProject(_)) => action,
        other => panic!("Add this folder emitted {other:?}"),
    }
}
