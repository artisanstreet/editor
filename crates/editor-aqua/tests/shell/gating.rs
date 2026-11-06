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
