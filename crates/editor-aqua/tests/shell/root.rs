use artisan_api::ItemId;
use editor_aqua::conversation::fold::{Row, Slot};
use editor_aqua::conversation::presentation::BodyKey;
use editor_aqua::core::Screen;
use editor_aqua::shell::application::SectionViews;
use editor_aqua::shell::root::RootViews;
use std::sync::Arc;
use test_support::clock::ManualClock;
use test_support::editor::shell::{Motion, RootFrames, ThreadShell, headless, window_id};
use test_support::editor::ui::text_opacity;
use test_support::items::{assistant_reply, item, user_message};

fn replies(opened: &mut ThreadShell, count: u64) {
    let thread = opened.thread.clone();
    let conversation = opened.workspace().conversation_mut();
    for index in 0..count {
        let reply = Arc::new(item(
            format!("reply-{index}"),
            thread.clone(),
            index + 1,
            assistant_reply(format!("Reply {index}.")),
        ));
        conversation.rows.make_mut().push((
            Row::Text {
                id: reply.id.clone(),
                slot: Slot::Prose,
                block: 0,
                run: None,
            },
            1,
        ));
        conversation
            .view
            .as_mut()
            .unwrap()
            .context
            .make_mut()
            .insert(reply.id.clone(), reply);
    }
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one landing scenario walked step by step"
)]
fn landing_completion_rebuilds_hidden_transcript_before_subtree_refresh() {
    for expire in [false, true] {
        let clock = ManualClock::starting_at(1_700_000_000_000);
        let mut opened = ThreadShell::open_at(
            "landing-host",
            "landing-thread",
            Motion::Reduced,
            clock.clock(),
        );
        let window = opened.window("landing");
        let item = Arc::new(item(
            "landing-message",
            opened.thread.clone(),
            1,
            user_message("Loaded transcript message"),
        ));
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
                artisan_api::ItemsPage {
                    items: vec![item.as_ref().clone()],
                    runs: Vec::new(),
                    more: false,
                    work: Vec::new(),
                },
            );
        workspace.conversation_mut().rows.make_mut().push((
            Row::Text {
                id: item.id.clone(),
                slot: Slot::Text(editor_aqua::conversation::fold::TextSection::User),
                block: 0,
                run: None,
            },
            1,
        ));
        let settle = std::time::Duration::from_secs(60);
        let deadline = shell.core.clock().now() + settle;
        workspace.conversation_mut().landing =
            Some(editor_aqua::conversation::controller::Landing::Settling { until: deadline });
        let mut root = RootViews::default();
        let element = root.build(&mut shell, &mut resources.ui, &window).unwrap();
        resources.ui.mount(|_| element).unwrap();
        resources.ui.build_frame().unwrap();
        let transcript_opacity =
            |ui: &mut aqua::UiApp| text_opacity(ui, "Loaded transcript message");
        test_support::assert_near!(
            transcript_opacity(&mut resources.ui),
            0.0,
            "landing must initially hide loaded rows"
        );
        let conversation = shell.entry.workspace_mut().unwrap().conversation_mut();
        if expire {
            clock.advance(settle);
            assert!(
                conversation.expire_landing(),
                "the scheduled deadline must end landing"
            );
        } else {
            use aqua_ui::collection_runtime::{CollectionViewport, FollowState};
            use aqua_ui::virtualization::RowRange;
            editor_aqua::conversation::view::transcript::observer(
                conversation,
                &shell.core,
                CollectionViewport {
                    visible: RowRange { start: 0, end: 1 },
                    mounted: RowRange { start: 0, end: 1 },
                    scroll_y: 0.0,
                    viewport_height: 600.0,
                    distance_from_start: 0.0,
                    distance_from_end: 0.0,
                    follow_state: FollowState::Tail { is_following: true },
                },
                true,
                true,
                false,
                |_, _| true,
                |_| false,
            );
            assert!(
                conversation.landing.is_none(),
                "loaded viewport observation must end landing"
            );
        }
        assert!(
            !root.refresh_transcript(&mut shell, &mut resources.ui, &window),
            "a landing transition must rebuild the outer frame, not only the inner list"
        );
        let element = root.build(&mut shell, &mut resources.ui, &window).unwrap();
        resources.ui.refresh(vec![element]).unwrap();
        resources.ui.build_frame().unwrap();
        test_support::assert_near!(
            transcript_opacity(&mut resources.ui),
            1.0,
            "the next rebuilt scene must expose loaded rows"
        );
        assert!(
            root.refresh_transcript(&mut shell, &mut resources.ui, &window),
            "visible transcripts must still allow ordinary subtree refreshes"
        );
        resources.ui.build_frame().unwrap();
        test_support::assert_near!(
            transcript_opacity(&mut resources.ui),
            1.0,
            "later subtree refreshes must preserve visible scene text"
        );
        let mut runtime = headless(window.clone());
        runtime.dispatch_frame_completed(
            &mut RootFrames {
                root: &mut root,
                shell: &mut shell,
                ui: &mut resources.ui,
            },
            window_id(),
            1,
        );
        let mut resized = window.clone();
        assert!(!root.resize_needs_rebuild(&shell, &resized));
        for (width, height) in [
            (980.0, 700.0),
            (960.0, 700.0),
            (960.0, 600.0),
            (1000.0, 700.0),
        ] {
            resized.inner_size.width = width;
            resized.inner_size.height = height;
            assert!(root.resize_needs_rebuild(&shell, &resized));
            resources.ui.set_viewport(
                test_support::float::narrow(width),
                test_support::float::narrow(height),
                1.0,
            );
            let element = root.build(&mut shell, &mut resources.ui, &resized).unwrap();
            resources.ui.refresh(vec![element]).unwrap();
            resources.ui.build_frame().unwrap();
            assert!(!root.resize_needs_rebuild(&shell, &resized));
            test_support::assert_near!(transcript_opacity(&mut resources.ui), 1.0);
        }
    }
}

#[test]
fn mounted_root_prepares_only_realized_bodies_and_releases_sources_on_navigation() {
    let mut opened = ThreadShell::open("root-host", "root-thread", Motion::Full);
    let workspace = opened.workspace();
    let document = workspace.selections().conversation();
    let markdown = std::rc::Rc::clone(workspace.markdown());
    replies(&mut opened, 1000);
    let window = opened.window("root");
    let ThreadShell {
        mut shell,
        mut resources,
        ..
    } = opened;
    let mut root = RootViews::default();
    let element = root.build(&mut shell, &mut resources.ui, &window).unwrap();
    resources.ui.mount(|_| element).unwrap();
    resources.ui.layout().unwrap();
    let key = |index| BodyKey {
        item: ItemId(format!("reply-{index}")),
        slot: Slot::Prose,
    };
    assert!(markdown.borrow().projected_range(&key(0), 0).is_some());
    assert!(markdown.borrow().projected_range(&key(999), 0).is_none());
    let sources: Vec<_> = resources
        .ui
        .tree()
        .traverse()
        .into_iter()
        .filter_map(|node| {
            let aqua_ui::tree::NodeKind::Text(text) = &resources.ui.tree().get(node)?.kind else {
                return None;
            };
            text.selection_source
                .as_ref()
                .filter(|binding| binding.document == document)
                .map(|binding| binding.slice.source)
        })
        .collect();
    assert!(!sources.is_empty());
    assert!(sources.len() < 100);
    let removed = ItemId("reply-0".into());
    let workspace = shell.entry.workspace_mut().unwrap();
    workspace
        .conversation_mut()
        .view
        .as_mut()
        .unwrap()
        .context
        .remove(&removed);
    workspace.conversation_mut().rows.make_mut().remove(0);
    let element = root.build(&mut shell, &mut resources.ui, &window).unwrap();
    resources.ui.refresh(vec![element]).unwrap();
    resources.ui.layout().unwrap();
    assert!(markdown.borrow().projected_range(&key(0), 0).is_none());
    assert!(markdown.borrow().projected_range(&key(1), 0).is_some());
    shell.core.navigate(Screen::Home);
    let element = root.build(&mut shell, &mut resources.ui, &window).unwrap();
    resources.ui.refresh(vec![element]).unwrap();
    assert!(markdown.borrow().projected_range(&key(1), 0).is_none());
    for source in sources {
        assert!(
            !resources
                .ui
                .unregister_selection_source_in(document, source)
        );
    }
}

#[test]
fn tail_controls_force_full_refresh_only_when_visibility_changes() {
    let mut opened = ThreadShell::open("root-host", "root-thread", Motion::Reduced);
    opened.workspace().conversation_mut().landing = None;
    replies(&mut opened, 40);
    let window = opened.window("root");
    let ThreadShell {
        mut shell,
        mut resources,
        ..
    } = opened;
    let mut root = RootViews::default();
    let element = root.build(&mut shell, &mut resources.ui, &window).unwrap();
    resources.ui.mount(|_| element).unwrap();
    resources.ui.layout().unwrap();
    let key = aqua_ui::tree::hash_key(aqua_ui::element::Key::from("jump-latest"));
    assert!(resources.ui.tree().sole_node_with_key(key).is_none());
    assert!(root.refresh_transcript(&mut shell, &mut resources.ui, &window));
    let workspace = shell.entry.workspace_mut().unwrap();
    workspace.conversation_mut().viewport(
        &shell.core,
        editor_aqua::conversation::viewport::Input::ScrolledAway {
            anchor: 1,
            through: 20,
        },
    );
    assert!(!root.refresh_transcript(&mut shell, &mut resources.ui, &window));
    let element = root.build(&mut shell, &mut resources.ui, &window).unwrap();
    resources.ui.refresh(vec![element]).unwrap();
    resources.ui.layout().unwrap();
    assert!(resources.ui.tree().sole_node_with_key(key).is_some());
    let workspace = shell.entry.workspace_mut().unwrap();
    workspace.conversation_mut().viewport(
        &shell.core,
        editor_aqua::conversation::viewport::Input::ScrolledAway {
            anchor: 2,
            through: 21,
        },
    );
    assert!(root.refresh_transcript(&mut shell, &mut resources.ui, &window));
    let workspace = shell.entry.workspace_mut().unwrap();
    workspace.conversation_mut().viewport(
        &shell.core,
        editor_aqua::conversation::viewport::Input::BottomReached,
    );
    assert!(!root.refresh_transcript(&mut shell, &mut resources.ui, &window));
    let element = root.build(&mut shell, &mut resources.ui, &window).unwrap();
    resources.ui.refresh(vec![element]).unwrap();
    resources.ui.layout().unwrap();
    assert!(resources.ui.tree().sole_node_with_key(key).is_none());
}

struct Keys<'a> {
    root: &'a mut RootViews,
    shell: &'a mut editor_aqua::shell::host::Shell,
    ui: &'a mut aqua::UiApp,
}

impl aqua::Application for Keys<'_> {
    fn on_window_event(
        &mut self,
        _: &mut aqua::AppCtx<'_>,
        _: aqua::prelude::AquaWindowId,
        _: &aqua::prelude::WindowEvent,
    ) {
    }

    fn on_input(
        &mut self,
        ctx: &mut aqua::AppCtx<'_>,
        id: aqua::prelude::AquaWindowId,
        event: &aqua::prelude::InputEvent,
    ) {
        self.root.input(self.shell, self.ui, ctx, id, event, false);
        self.root.animate(self.shell, self.ui, ctx, id);
    }

    fn build_scene(
        &mut self,
        _: &aqua::prelude::ObservedWindowState,
        _: aqua::Instant,
    ) -> Option<aqua::Instant> {
        None
    }
}

fn wide(opened: &mut ThreadShell, width: f32) -> aqua::prelude::ObservedWindowState {
    let mut window = opened.window("wide");
    window.inner_size.width = width.into();
    opened.resources.ui.set_viewport(width, 700.0, 1.0);
    window
}

fn press(
    root: &mut RootViews,
    shell: &mut editor_aqua::shell::host::Shell,
    ui: &mut aqua::UiApp,
    window: &aqua::prelude::ObservedWindowState,
    key: char,
    modifiers: aqua_core::Modifiers,
) {
    let mut runtime = headless(window.clone());
    runtime.dispatch_input(
        &mut Keys { root, shell, ui },
        window_id(),
        &aqua::prelude::InputEvent::KeyDown(aqua_core::KeyEvent {
            key: aqua_core::Key::Char(key),
            modifiers,
            repeat: false,
        }),
    );
}

fn rebuild(
    root: &mut RootViews,
    shell: &mut editor_aqua::shell::host::Shell,
    ui: &mut aqua::UiApp,
    window: &aqua::prelude::ObservedWindowState,
) {
    let element = root.build(shell, ui, window).unwrap();
    ui.refresh(vec![element]).unwrap();
    ui.layout().unwrap();
}

fn keyed(ui: &aqua::UiApp, key: &str) -> Option<aqua_core::Rect> {
    let node = ui
        .tree()
        .sole_node_with_key(aqua_ui::tree::hash_key(aqua_ui::element::Key::from(key)))?;
    ui.tree().visual_rect(node)
}

#[test]
fn every_workspace_route_mounts_its_screen() {
    let mut opened = ThreadShell::open("routes-host", "routes-thread", Motion::Reduced);
    let window = opened.window("routes");
    let ThreadShell {
        mut shell,
        mut resources,
        host,
        ..
    } = opened;
    let mut root = RootViews::default();
    let element = root.build(&mut shell, &mut resources.ui, &window).unwrap();
    resources.ui.mount(|_| element).unwrap();
    for (query, screen, mounted) in [
        ("market", Screen::Marketplace, "marketplace-list"),
        ("usage", Screen::Usage, "usage-screen"),
    ] {
        let mut palette = editor_aqua::shell::palette::Palette::default();
        palette.set_query(query.into());
        palette.refresh(Some(&host.id), &[]);
        assert_eq!(
            palette.selected(),
            Some(&screen),
            "the palette offers {query}"
        );
        shell.navigate(screen.clone());
        rebuild(&mut root, &mut shell, &mut resources.ui, &window);
        assert!(
            keyed(&resources.ui, mounted).is_some(),
            "{screen:?} mounts its content"
        );
    }
    shell.core.remove_host(&host.id).unwrap();
    shell.core.navigate(Screen::Home);
    assert_eq!(shell.core.workspace_screen(), &Screen::Home);
    rebuild(&mut root, &mut shell, &mut resources.ui, &window);
    assert!(
        test_support::editor::ui::painted_texts(&resources.ui)
            .iter()
            .any(|text| text == "Choose a Forge"),
        "Home mounts its Forge picker"
    );
}

#[test]
fn ctrl_b_collapses_the_rendered_sidebar() {
    let mut opened = ThreadShell::open("sidebar-host", "sidebar-thread", Motion::Reduced);
    let window = wide(&mut opened, 1400.0);
    let ThreadShell {
        mut shell,
        mut resources,
        ..
    } = opened;
    let mut root = RootViews::default();
    let element = root.build(&mut shell, &mut resources.ui, &window).unwrap();
    resources.ui.mount(|_| element).unwrap();
    resources.ui.layout().unwrap();
    let sidebar = editor_aqua::frame::view::SIDEBAR;
    let open = keyed(&resources.ui, sidebar).unwrap().w;
    press(
        &mut root,
        &mut shell,
        &mut resources.ui,
        &window,
        'b',
        aqua_core::Modifiers::CONTROL,
    );
    rebuild(&mut root, &mut shell, &mut resources.ui, &window);
    let collapsed = keyed(&resources.ui, sidebar).unwrap().w;
    assert!(
        collapsed < open,
        "Ctrl+B narrows the sidebar from {open} to {collapsed}"
    );
    press(
        &mut root,
        &mut shell,
        &mut resources.ui,
        &window,
        'b',
        aqua_core::Modifiers::CONTROL,
    );
    rebuild(&mut root, &mut shell, &mut resources.ui, &window);
    test_support::assert_near!(keyed(&resources.ui, sidebar).unwrap().w, open);
}

#[test]
fn ctrl_alt_b_swaps_the_overview_for_the_full_inspector() {
    let mut opened = ThreadShell::open("inspector-host", "inspector-thread", Motion::Reduced);
    let window = wide(&mut opened, 1800.0);
    let ThreadShell {
        mut shell,
        mut resources,
        ..
    } = opened;
    let mut root = RootViews::default();
    let element = root.build(&mut shell, &mut resources.ui, &window).unwrap();
    resources.ui.mount(|_| element).unwrap();
    resources.ui.layout().unwrap();
    assert!(keyed(&resources.ui, "artisan-thread-overview").is_some());
    assert!(keyed(&resources.ui, "artisan-thread-inspector").is_none());
    let both = aqua_core::Modifiers::CONTROL.union(aqua_core::Modifiers::ALT);
    press(&mut root, &mut shell, &mut resources.ui, &window, 'b', both);
    rebuild(&mut root, &mut shell, &mut resources.ui, &window);
    assert!(keyed(&resources.ui, "artisan-thread-overview").is_none());
    assert!(keyed(&resources.ui, "artisan-thread-inspector").is_some());
    assert!(
        test_support::editor::ui::painted_texts(&resources.ui)
            .iter()
            .any(|text| text == "Worktree"),
        "the full inspector lists the worktree"
    );
    press(&mut root, &mut shell, &mut resources.ui, &window, 'b', both);
    rebuild(&mut root, &mut shell, &mut resources.ui, &window);
    assert!(keyed(&resources.ui, "artisan-thread-overview").is_some());
}

#[test]
fn the_overview_only_shows_when_its_full_width_fits() {
    let mut opened = ThreadShell::open("fit-host", "fit-thread", Motion::Reduced);
    let window = wide(&mut opened, 1800.0);
    let ThreadShell {
        mut shell,
        mut resources,
        ..
    } = opened;
    let mut root = RootViews::default();
    let element = root.build(&mut shell, &mut resources.ui, &window).unwrap();
    resources.ui.mount(|_| element).unwrap();
    for step in 0..50_u16 {
        let width = 10.0_f32.mul_add(f32::from(step), 1300.0);
        let mut resized = window.clone();
        resized.inner_size.width = f64::from(width);
        resources.ui.set_viewport(width, 700.0, 1.0);
        rebuild(&mut root, &mut shell, &mut resources.ui, &resized);
        let Some(overview) = keyed(&resources.ui, "artisan-thread-overview") else {
            continue;
        };
        let thread = keyed(&resources.ui, "thread-under-sky").unwrap();
        assert!(
            thread.w >= editor_aqua::shell::layout::CHAT_MIN_WIDTH,
            "at {width}px the overview ({}px) squeezes the thread to {}px",
            overview.w,
            thread.w
        );
    }
}

#[test]
fn a_reveal_whose_row_disappeared_does_not_hold_back_jumping_to_the_latest_message() {
    use editor_aqua::conversation::adapter::{ScrollResolution, resolve_scroll};
    use editor_aqua::conversation::controller::ScrollAction;
    use editor_aqua::conversation::scroll::{following_tail, leave_tail};

    let mut opened = ThreadShell::open("scroll-host", "scroll-thread", Motion::Reduced);
    opened.workspace().conversation_mut().landing = None;
    replies(&mut opened, 40);
    let window = opened.window("scroll");
    let ThreadShell {
        mut shell,
        mut resources,
        ..
    } = opened;
    let mut root = RootViews::default();
    let element = root.build(&mut shell, &mut resources.ui, &window).unwrap();
    resources.ui.mount(|_| element).unwrap();
    resources.ui.layout().unwrap();
    leave_tail(&mut resources.ui);
    assert!(!following_tail(&resources.ui));

    let conversation = shell.entry.workspace_mut().unwrap().conversation_mut();
    let last = conversation.rows[39].0.key();
    conversation.reveal_row(39);
    conversation.rows.make_mut().truncate(30);
    assert_eq!(
        resolve_scroll(conversation, &ScrollAction::Row(last), true),
        ScrollResolution::Cancelled
    );
    rebuild(&mut root, &mut shell, &mut resources.ui, &window);
    root.advance_glides(&mut shell, &mut resources.ui, aqua::Instant::ZERO);

    let conversation = shell.entry.workspace_mut().unwrap().conversation_mut();
    conversation.jump(
        &shell.core,
        editor_aqua::conversation::viewport::Jump::Latest,
    );
    root.advance_glides(&mut shell, &mut resources.ui, aqua::Instant::ZERO);
    rebuild(&mut root, &mut shell, &mut resources.ui, &window);
    root.advance_glides(&mut shell, &mut resources.ui, aqua::Instant::ZERO);
    assert!(
        following_tail(&resources.ui),
        "jumping to the latest message follows the tail"
    );
}

#[test]
fn a_wheel_notch_on_the_transcript_glides_by_the_frame_clock() {
    use aqua::prelude::InputEvent;
    use aqua_core::{Point, ScrollDelta};
    use std::time::Duration;

    let mut opened = ThreadShell::open("wheel-host", "wheel-thread", Motion::Full);
    opened.workspace().conversation_mut().landing = None;
    replies(&mut opened, 80);
    let window = opened.window("wheel");
    let ThreadShell {
        mut shell,
        mut resources,
        ..
    } = opened;
    let mut root = RootViews::default();
    let element = root.build(&mut shell, &mut resources.ui, &window).unwrap();
    resources.ui.mount(|_| element).unwrap();
    resources.ui.layout().unwrap();
    let transcript = keyed(&resources.ui, "conversation-transcript").unwrap();
    let notch = InputEvent::Scroll {
        at: Point::new(
            transcript.x + transcript.w / 2.0,
            transcript.y + transcript.h / 2.0,
        ),
        delta: ScrollDelta { x: 0.0, y: 16.0 },
    };
    let frame = aqua::Instant::from_nanos(5_000_000_000);
    assert_eq!(
        root.pre_input(&shell, &mut resources.ui, &notch, frame),
        Some(true)
    );
    assert!(root.glide_active());
    root.advance_glides(&mut shell, &mut resources.ui, frame);
    assert!(
        root.glide_active(),
        "no frame time has passed, so the glide has not moved"
    );
    let later = frame + Duration::from_secs(1);
    root.advance_glides(&mut shell, &mut resources.ui, later);
    root.advance_glides(
        &mut shell,
        &mut resources.ui,
        later + Duration::from_millis(16),
    );
    assert!(
        !root.glide_active(),
        "a second of frames after the notch the glide has come to rest"
    );
}
