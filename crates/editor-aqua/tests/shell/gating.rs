use aqua::prelude::{
    AquaWindowId, InputEvent, ObservedWindowState, Point, PointerEvent, WindowEvent, WindowOptions,
};
use aqua::{AppCtx, Aqua, AquaConfig, UiApp};
use aqua_core::PointerKind;
use aqua_ui::element::{Box as UiBox, Element, Text};
use editor_aqua::core::{Core, EditorFiles};
use editor_aqua::shell::{
    application::{SectionViews, ShellApplication},
    host::{Shell, ShellResources},
};
use std::{cell::Cell, rc::Rc};

#[derive(Default)]
struct Fixture {
    builds: Rc<Cell<usize>>,
    change: Rc<Cell<bool>>,
    focused: Rc<Cell<bool>>,
}

#[test]
fn update_reasons_are_collected_without_profiling_and_drained_once() {
    use editor_aqua::shell::{rebuild_stats, refresh::Refresh};
    rebuild_stats::take_notes();
    assert!(!rebuild_stats::note("ignored", false));
    rebuild_stats::note("anim_clock", true);
    rebuild_stats::note("anim_clock", true);
    let causes = rebuild_stats::take_notes();
    assert_eq!(causes, ["anim_clock"]);
    assert_eq!(Refresh::from_causes(&causes), Refresh::Transcript);
    assert!(rebuild_stats::take_notes().is_empty());
}

#[derive(Default)]
struct RefreshFixture {
    builds: usize,
    sidebar: Cell<usize>,
    sidebar_frame: Cell<usize>,
    transcript: usize,
    causes: Option<Vec<&'static str>>,
    allow_transcript: bool,
}

impl SectionViews for RefreshFixture {
    fn input(
        &mut self,
        _: &mut Shell,
        _: &mut UiApp,
        _: &mut AppCtx<'_>,
        _: AquaWindowId,
        _: &InputEvent,
        _: bool,
    ) -> bool {
        false
    }

    fn wake(&mut self, _: &mut Shell, _: &mut UiApp, _: &mut AppCtx<'_>, _: AquaWindowId) -> bool {
        false
    }

    fn build(
        &mut self,
        _: &mut Shell,
        _: &mut UiApp,
        _: &ObservedWindowState,
    ) -> anyhow::Result<Element> {
        self.builds += 1;
        Ok(UiBox::new().build())
    }

    fn refresh_sidebar(&self, _: &Shell, _: &mut UiApp, _: &ObservedWindowState) -> bool {
        self.sidebar.set(self.sidebar.get() + 1);
        true
    }

    fn refresh_sidebar_frame(&self, _: &Shell, _: &mut UiApp, _: &ObservedWindowState) -> bool {
        self.sidebar_frame.set(self.sidebar_frame.get() + 1);
        true
    }

    fn refresh_transcript(
        &mut self,
        _: &mut Shell,
        _: &mut UiApp,
        _: &ObservedWindowState,
    ) -> bool {
        self.transcript += 1;
        self.allow_transcript
    }

    fn frame_completed(
        &mut self,
        _: &mut Shell,
        _: &mut UiApp,
        _: &mut AppCtx<'_>,
        _: AquaWindowId,
        _: u64,
    ) -> bool {
        match self.causes.take() {
            Some(causes) => {
                for cause in causes {
                    editor_aqua::shell::rebuild_stats::note(cause, true);
                }
                true
            }
            None => false,
        }
    }
}

#[test]
fn independent_section_updates_avoid_root_rebuilds_and_failed_refreshes_fall_back() {
    let directory = tempfile::tempdir().unwrap();
    let files = EditorFiles {
        dir: directory.path().to_owned(),
    };
    let resources = ShellResources::new(1.0).unwrap();
    let mut app = ShellApplication::new(
        files.clone(),
        resources,
        RefreshFixture {
            allow_transcript: true,
            ..RefreshFixture::default()
        },
    );
    let mut core = Core::for_tests(files).unwrap();
    core.set_image_assets(app.shell.images().clone());
    app.shell.core = core;
    app.shell.entry.sync(&app.shell.core);
    let mut aqua = Aqua::new(AquaConfig {
        headless: true,
        ..AquaConfig::default()
    })
    .unwrap();
    let id = AquaWindowId::new(1).unwrap();
    aqua.add_headless_window(ObservedWindowState::new(
        id,
        "refresh".into(),
        WindowOptions::default().inner_size,
    ));
    aqua.dispatch_start(&mut app, id);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while app.shell.core.test_startup_pending() {
        assert!(std::time::Instant::now() < deadline);
        aqua.dispatch_wake(&mut app, id);
        std::thread::yield_now();
    }
    editor_aqua::shell::rebuild_stats::take_notes();
    let base = app.views.builds;
    app.views.causes = Some(vec!["sidebar_tick", "after_layout", "al_motion"]);
    aqua.dispatch_frame_completed(&mut app, id, 1);
    assert_eq!(app.views.builds, base);
    assert_eq!(app.views.sidebar.get(), 1);
    assert_eq!(app.views.transcript, 1);
    app.views.causes = Some(vec!["anim_sidebar_resize"]);
    aqua.dispatch_frame_completed(&mut app, id, 2);
    assert_eq!(app.views.builds, base);
    assert_eq!(app.views.sidebar_frame.get(), 1);
    app.views.causes = Some(vec!["anim_edges", "anim_clock"]);
    aqua.dispatch_frame_completed(&mut app, id, 3);
    assert_eq!(app.views.builds, base);
    assert_eq!(app.views.sidebar_frame.get(), 2);
    assert_eq!(app.views.transcript, 2);
    app.views.causes = Some(vec!["theme"]);
    aqua.dispatch_frame_completed(&mut app, id, 2);
    assert_eq!(app.views.builds, base + 1);
    app.views.allow_transcript = false;
    app.views.causes = Some(vec!["after_layout"]);
    aqua.dispatch_frame_completed(&mut app, id, 3);
    assert_eq!(app.views.transcript, 3);
    assert_eq!(app.views.builds, base + 2);
}

impl SectionViews for Fixture {
    fn build(
        &mut self,
        _: &mut Shell,
        _: &mut UiApp,
        _: &ObservedWindowState,
    ) -> anyhow::Result<Element> {
        self.builds.set(self.builds.get() + 1);
        Ok(UiBox::new()
            .child(
                Text::new("Search")
                    .build()
                    .with_key("marketplace-search")
                    .debug_selector("marketplace-search")
                    .focusable(true),
            )
            .build())
    }

    fn input(
        &mut self,
        _: &mut Shell,
        _: &mut UiApp,
        _: &mut AppCtx<'_>,
        _: AquaWindowId,
        _: &InputEvent,
        _: bool,
    ) -> bool {
        self.change.replace(false)
    }

    fn wake(&mut self, _: &mut Shell, ui: &mut UiApp, _: &mut AppCtx<'_>, _: AquaWindowId) -> bool {
        self.focused.set(
            ui.find_debug_selector("marketplace-search")
                .is_some_and(|node| ui.tree().focused() == Some(node)),
        );
        false
    }
}

#[test]
fn rebuilds_follow_state_changes_and_effect_only_focus_is_applied() {
    let directory = tempfile::tempdir().unwrap();
    let files = EditorFiles {
        dir: directory.path().to_owned(),
    };
    let builds = Rc::new(Cell::new(0));
    let change = Rc::new(Cell::new(false));
    let focused = Rc::new(Cell::new(false));
    let views = Fixture {
        builds: builds.clone(),
        change: change.clone(),
        focused: focused.clone(),
    };
    let resources = ShellResources::new(1.0).unwrap();
    let mut app = ShellApplication::new(files.clone(), resources, views);
    let mut core = Core::for_tests(files).unwrap();
    core.set_image_assets(app.shell.images().clone());
    app.shell.core = core;
    app.shell.entry.sync(&app.shell.core);
    let mut aqua = Aqua::new(AquaConfig {
        headless: true,
        ..AquaConfig::default()
    })
    .unwrap();
    let id = AquaWindowId::new(1).unwrap();
    let mut size = WindowOptions::default().inner_size;
    size.width = 800.0;
    size.height = 600.0;
    aqua.add_headless_window(ObservedWindowState::new(id, "test".into(), size));
    aqua.dispatch_start(&mut app, id);
    assert_eq!(builds.get(), 1);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while app.shell.core.test_startup_pending() {
        assert!(
            std::time::Instant::now() < deadline,
            "Core startup did not settle"
        );
        aqua.dispatch_wake(&mut app, id);
        std::thread::yield_now();
    }
    builds.set(1);
    let pointer = InputEvent::PointerMove(PointerEvent {
        contact: 0,
        kind: PointerKind::Mouse,
        position: Point::new(700.0, 500.0),
        buttons: 0,
    });
    for iteration in 0..4 {
        aqua.dispatch_input(&mut app, id, &pointer);
        assert_eq!(builds.get(), 1, "pointer input {iteration} rebuilt");
        aqua.dispatch_wake(&mut app, id);
        assert_eq!(builds.get(), 1, "empty wake {iteration} rebuilt");
    }
    assert_eq!(builds.get(), 1);
    app.shell.entry.workspace().unwrap().marketplace_emit()(
        editor_aqua::marketplace::view::ViewAction::FocusSearch,
    );
    aqua.dispatch_wake(&mut app, id);
    assert_eq!(builds.get(), 1);
    aqua.dispatch_wake(&mut app, id);
    assert!(focused.get());
    assert_eq!(builds.get(), 1);
    change.set(true);
    aqua.dispatch_input(&mut app, id, &pointer);
    assert_eq!(builds.get(), 2);
    app.shell
        .core
        .set_setting("shell-gating-test", serde_json::json!(true));
    aqua.dispatch_wake(&mut app, id);
    assert_eq!(builds.get(), 3);
    app.shell
        .core
        .set_setting("shell-gating-test", serde_json::json!(true));
    aqua.dispatch_wake(&mut app, id);
    assert_eq!(builds.get(), 3);
    size.width = 900.0;
    aqua.dispatch_window_event(&mut app, id, &WindowEvent::Resized(id, size));
    assert_eq!(builds.get(), 4);
}

struct ShortcutFixture {
    root: editor_aqua::shell::root::RootViews,
    editor: editor_aqua::ui::text::mount::EditorController,
    clear_focus: Rc<Cell<bool>>,
}

impl SectionViews for ShortcutFixture {
    fn build(
        &mut self,
        shell: &mut Shell,
        _: &mut UiApp,
        _: &ObservedWindowState,
    ) -> anyhow::Result<Element> {
        Ok(self.editor.element_with_notice_keyed(
            "shortcut-editor",
            editor_aqua::ui::text::Mode::SingleLine,
            &shell.preferences.theme(),
            "",
            false,
            false,
        ))
    }

    fn mounted(&mut self, _: &mut Shell, ui: &mut UiApp, _: &mut AppCtx<'_>, _: AquaWindowId) {
        ui.focus_key("shortcut-editor");
    }

    fn input(
        &mut self,
        shell: &mut Shell,
        ui: &mut UiApp,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
        input: &InputEvent,
        consumed: bool,
    ) -> bool {
        self.root.input(shell, ui, ctx, window, input, consumed)
    }

    fn wake(
        &mut self,
        shell: &mut Shell,
        ui: &mut UiApp,
        ctx: &mut AppCtx<'_>,
        window: AquaWindowId,
    ) -> bool {
        if self.clear_focus.replace(false) {
            ui.tree_mut().set_focused(None);
        }
        self.root.wake(shell, ui, ctx, window)
    }
}

#[test]
fn editor_word_left_consumes_alt_left_before_workspace_back() {
    let directory = tempfile::tempdir().unwrap();
    let files = EditorFiles {
        dir: directory.path().to_owned(),
    };
    let clear_focus = Rc::new(Cell::new(false));
    let editor = editor_aqua::ui::text::mount::EditorController::new(
        editor_aqua::ui::text::Editor::new("", editor_aqua::ui::text::Mode::SingleLine),
        |_| {},
    );
    let views = ShortcutFixture {
        root: Default::default(),
        editor,
        clear_focus: clear_focus.clone(),
    };
    let mut app = ShellApplication::new(files.clone(), ShellResources::new(1.0).unwrap(), views);
    let mut core = Core::for_tests(files).unwrap();
    core.set_image_assets(app.shell.images().clone());
    core.navigate(editor_aqua::core::Screen::Usage);
    app.shell.core = core;
    app.shell.entry.sync(&app.shell.core);
    let mut aqua = Aqua::new(AquaConfig {
        headless: true,
        ..AquaConfig::default()
    })
    .unwrap();
    let id = AquaWindowId::new(2).unwrap();
    let size = WindowOptions::default().inner_size;
    aqua.add_headless_window(ObservedWindowState::new(id, "test".into(), size));
    aqua.dispatch_start(&mut app, id);
    let key = InputEvent::KeyDown(aqua_core::KeyEvent {
        key: aqua_core::Key::Named(aqua_core::NamedKey::Left),
        modifiers: aqua_core::Modifiers::ALT,
        repeat: false,
    });
    aqua.dispatch_input(&mut app, id, &key);
    assert_eq!(app.shell.core.screen(), &editor_aqua::core::Screen::Usage);
    clear_focus.set(true);
    aqua.dispatch_wake(&mut app, id);
    aqua.dispatch_input(&mut app, id, &key);
    assert_eq!(app.shell.core.screen(), &editor_aqua::core::Screen::Home);
}

#[derive(Default)]
struct HoverFixture {
    visible: bool,
}

fn hover_row(theme: &editor_aqua::ui::theme::ArtisanTheme, key: &str) -> Element {
    use editor_aqua::ui::kit::{Hover, live};
    live::track(
        UiBox::new()
            .width(200.0)
            .height(40.0)
            .child(live::layer(Hover::Neutral, theme, 8.0, false))
            .build()
            .with_key(key.to_owned()),
    )
}

impl SectionViews for HoverFixture {
    fn build(
        &mut self,
        shell: &mut Shell,
        _: &mut UiApp,
        _: &ObservedWindowState,
    ) -> anyhow::Result<Element> {
        let theme = shell.preferences.theme();
        Ok(UiBox::new()
            .width(200.0)
            .height(200.0)
            .layout("column")
            .child(hover_row(&theme, "hover-first"))
            .child(hover_row(&theme, "hover-second"))
            .build())
    }

    fn mounted(&mut self, _: &mut Shell, ui: &mut UiApp, _: &mut AppCtx<'_>, _: AquaWindowId) {
        ui.set_viewport(800.0, 600.0, 1.0);
        ui.build_frame().unwrap();
    }

    fn input(
        &mut self,
        _: &mut Shell,
        ui: &mut UiApp,
        ctx: &mut AppCtx<'_>,
        _: AquaWindowId,
        _: &InputEvent,
        _: bool,
    ) -> bool {
        ui.host_mut().motion_mut().set_reduced_motion(
            aqua_ui::motion::ReducedMotion::Reduce,
            ctx.now().as_secs_f64(),
        );
        false
    }

    fn wake(&mut self, _: &mut Shell, ui: &mut UiApp, _: &mut AppCtx<'_>, _: AquaWindowId) -> bool {
        let tree = ui.tree();
        let row = tree
            .sole_node_with_key(aqua_ui::tree::hash_key(aqua_ui::element::Key::from(
                "hover-first",
            )))
            .unwrap();
        let layer_key = aqua_ui::tree::hash_key(aqua_ui::element::Key::from("kit-hover-layer"));
        let layer = tree
            .get(row)
            .unwrap()
            .children
            .iter()
            .copied()
            .find(|child| {
                tree.get(*child)
                    .is_some_and(|record| record.key == Some(layer_key))
            })
            .unwrap();
        self.visible = tree.paint(layer).unwrap().opacity == Some(1.0);
        false
    }
}

#[test]
fn hover_input_updates_without_any_render_completion() {
    let directory = tempfile::tempdir().unwrap();
    let files = EditorFiles {
        dir: directory.path().to_owned(),
    };
    let resources = ShellResources::new(1.0).unwrap();
    let mut app = ShellApplication::new(files.clone(), resources, HoverFixture::default());
    let mut core = Core::for_tests(files).unwrap();
    core.set_image_assets(app.shell.images().clone());
    app.shell.core = core;
    app.shell.entry.sync(&app.shell.core);
    let mut aqua = Aqua::new(AquaConfig {
        headless: true,
        ..AquaConfig::default()
    })
    .unwrap();
    let id = AquaWindowId::new(1).unwrap();
    aqua.add_headless_window(ObservedWindowState::new(
        id,
        "hover".into(),
        WindowOptions::default().inner_size,
    ));
    aqua.dispatch_start(&mut app, id);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while app.shell.core.test_startup_pending() {
        assert!(
            std::time::Instant::now() < deadline,
            "Core startup did not settle"
        );
        aqua.dispatch_wake(&mut app, id);
        std::thread::yield_now();
    }
    aqua.dispatch_wake(&mut app, id);
    let pointer = |y| {
        InputEvent::PointerMove(PointerEvent {
            contact: 0,
            kind: PointerKind::Mouse,
            position: Point::new(20.0, y),
            buttons: 0,
        })
    };
    aqua.dispatch_input(&mut app, id, &pointer(20.0));
    aqua.dispatch_wake(&mut app, id);
    assert!(
        app.views.visible,
        "first hover must paint without a completion callback"
    );
    aqua.dispatch_input(&mut app, id, &pointer(60.0));
    aqua.dispatch_wake(&mut app, id);
    assert!(
        !app.views.visible,
        "moving rows must hide the old highlight"
    );
    aqua.dispatch_input(&mut app, id, &pointer(120.0));
    aqua.dispatch_wake(&mut app, id);
    assert!(
        !app.views.visible,
        "leaving must hide without a completion callback"
    );
    aqua.dispatch_input(&mut app, id, &pointer(20.0));
    aqua.dispatch_wake(&mut app, id);
    assert!(
        app.views.visible,
        "reentering must paint without a completion callback"
    );
}
