use artisan_api::{
    DirListing, EngineConfig, EngineId, Project, ProjectId, RunStatus, Thread, ThreadId,
};
use editor_aqua::sidebar::geometry::{PageDirection, PagePosition, page_direction};
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
    };
    let factory = sidebar.thread_visual_factory(ArtisanTheme::resolved(), false);
    let visuals = factory(&row, None, None);
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
        .find_map(|(_, item)| match item {
            DisplayItem::Text { run, .. } => Some(run.clone()),
            _ => None,
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
    assert!(scene.iter().any(|(_, item)| matches!(item, DisplayItem::PushGpuiQuadPulse(pulse) if pulse.period_ms == 1_600 && pulse.min_opacity == 0.4)));
    assert!(
        scene
            .iter()
            .any(|(_, item)| matches!(item, DisplayItem::PopGpuiQuadPulse))
    );

    let reduced = sidebar.thread_visual_factory(ArtisanTheme::resolved(), true)(&row, None, None);
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
    use editor_aqua::core::{Core, EditorFiles};
    use editor_aqua::sidebar::{
        controller::Sidebar,
        view::{ViewAction, dispatch},
    };

    let directory = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: directory.path().join("editor"),
    })
    .unwrap();
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
    use editor_aqua::core::{Core, EditorFiles};
    use editor_aqua::sidebar::controller::Sidebar;
    use std::time::{Duration, Instant};

    let directory = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: directory.path().join("editor"),
    })
    .unwrap();
    let mut sidebar = Sidebar::new(true);
    let start = Instant::now();
    assert!(!sidebar.advance_motion(start, false));
    sidebar.toggle_history(&mut core);
    assert!(sidebar.history_visible());
    assert!(sidebar.advance_motion(start + Duration::from_millis(100), false));
    assert!(sidebar.history_fraction > 0.0 && sidebar.history_fraction < 1.0);
    assert!(sidebar.advance_motion(start + Duration::from_secs(2), false));
    assert_eq!(sidebar.history_fraction, 1.0);
    assert!(sidebar.advance_motion(start + Duration::from_secs(3), false));
    assert!(!sidebar.advance_motion(start + Duration::from_secs(4), false));
    sidebar.toggle_history(&mut core);
    assert!(sidebar.advance_motion(start + Duration::from_secs(10), true));
    assert_eq!(sidebar.history_fraction, 0.0);
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
    let host = Host {
        waiting: vec![waiting.id.clone()],
        working: vec![waiting.id.clone()],
        project,
        threads: vec![older, settled, waiting],
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
    picker.finish_create(Some("connection lost".into()));
    assert_eq!(picker.begin_create().unwrap(), first);
    picker.finish_create(Some("connection lost".into()));
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
    let mut page = PagePosition {
        viewport_height: 400.0,
        above: 700.0,
        below: 0.0,
        loading: false,
        loaded: true,
        failed: true,
        scrolled: false,
        at_latest: true,
        has_older: true,
        history_open: true,
        history_empty: false,
    };
    assert_eq!(page_direction(&page), None);
    page.scrolled = true;
    assert_eq!(page_direction(&page), Some(PageDirection::Older));
    page.history_open = false;
    assert_eq!(page_direction(&page), None);
    page.at_latest = false;
    page.above = 0.0;
    assert_eq!(page_direction(&page), Some(PageDirection::Newer));
}

#[test]
fn removed_and_recreated_host_rejects_delayed_sidebar_completions() {
    use artisan_api::tls::{Identity, fingerprint};
    use editor_aqua::core::{Core, EditorFiles, ForgeClient, HostConfig, HostId, HostKind, Screen};
    use editor_aqua::sidebar::controller::Sidebar;

    let directory = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: directory.path().join("editor"),
    })
    .unwrap();
    let identity = Identity::generate("sidebar-test").unwrap();
    let fingerprint = fingerprint(identity.certificate_der().unwrap().as_ref());
    let client = ForgeClient::new("127.0.0.1:1", &fingerprint, &identity).unwrap();
    let id = HostId("same-host".into());
    let config = HostConfig {
        id: id.clone(),
        name: "Forge".into(),
        address: "127.0.0.1:1".into(),
        fingerprint,
        kind: HostKind::Local,
    };
    core.add_test_host(&config, client.clone());
    core.navigate(Screen::NewThread {
        host: id.clone(),
        project: None,
    });
    let first = core.host(&id).unwrap().instance();
    let mut sidebar = Sidebar::new(false);
    sidebar.sync(&core, 0);
    sidebar.add_project(&core);
    sidebar.picker.as_mut().unwrap().creating = true;
    let generation = sidebar.test_picker_generation();
    let browse_generation = sidebar.test_browse_generation();
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
    futures::executor::block_on(sidebar.test_action_failure(id.clone(), first));
    sidebar.drain(&mut core, 0);
    assert!(sidebar.error.is_none());
    assert!(sidebar.picker.is_none());
    core.add_test_host(&config, client.clone());
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
    sidebar.picker.as_mut().unwrap().listing = Some(DirListing {
        path: "/new".into(),
        parent: None,
        entries: vec![],
    });
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
    sidebar.error = Some("new host error".into());
    futures::executor::block_on(sidebar.test_action_failure(id.clone(), first));
    futures::executor::block_on(sidebar.test_browse_completion(
        id.clone(),
        first,
        browse_generation,
        DirListing {
            path: "/old".into(),
            parent: None,
            entries: vec![],
        },
    ));
    futures::executor::block_on(sidebar.test_create_completion(
        id.clone(),
        first,
        generation,
        old_project,
    ));
    sidebar.drain(&mut core, 0);
    assert_eq!(sidebar.error.as_deref(), Some("new host error"));
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
    core.add_test_host(&config, client);
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
        let spacer = history_spacer(&column(), 500.0, 0.0);
        assert!((spacer - (500.0 - 10.0 - 43.0)).abs() < 1e-4);
    }

    #[test]
    fn open_history_leaves_room_for_the_rows_below() {
        let spacer = history_spacer(&column(), 500.0, 1.0);
        assert!((spacer - (500.0 - 10.0 - 43.0 - 32.0)).abs() < 1e-4);
    }

    #[test]
    fn fraction_interpolates_between_closed_and_open() {
        let closed = history_spacer(&column(), 500.0, 0.0);
        let open = history_spacer(&column(), 500.0, 1.0);
        let half = history_spacer(&column(), 500.0, 0.5);
        assert!((half - (closed + open) / 2.0).abs() < 1e-4);
    }

    #[test]
    fn spacer_never_drops_below_twelve_pixels() {
        assert!((history_spacer(&column(), 20.0, 0.0) - 12.0).abs() < 1e-4);
        assert!((history_spacer(&column(), 20.0, 1.0) - 12.0).abs() < 1e-4);
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
