use aqua_core::{InputEvent, Point, ScrollDelta, Size};
use aqua_ui::element::Box as UiBox;
use editor_aqua::core::{Screen, SettingsPage};
use editor_aqua::settings::{about::BuildInfo, controller::SettingsController, view};
use editor_aqua::shell::{host::ShellResources, wheel::keyed_node};
use editor_aqua::ui::{dialog::DialogHost, theme::ArtisanTheme};
use std::rc::Rc;

fn settings_scroll(viewport: Size, scale: f32) {
    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(directory.path());
    core.navigate(Screen::Settings(SettingsPage::General));
    let controller = SettingsController::default();
    let build = BuildInfo::default();
    let mut resources = ShellResources::new(scale).unwrap();
    let app = &mut resources.ui;
    let dimensions = editor_aqua::shell::layout::Size {
        width: viewport.w,
        height: viewport.h,
    };
    app.set_viewport(viewport.w, viewport.h, scale);
    app.mount(|_| UiBox::new().width(viewport.w).height(viewport.h).build())
        .unwrap();
    app.build_frame().unwrap();
    let owner = app.tree().root().unwrap();
    let page = view::build(
        &controller,
        &core,
        &ArtisanTheme::resolved(),
        &build,
        viewport,
        true,
        Rc::new(|_| {}),
    );
    let dialog = editor_aqua::shell::settings_frame::settings_dialog(
        &ArtisanTheme::resolved(),
        dimensions,
        UiBox::new().build(),
        page,
        Rc::new(|| {}),
    );
    let host = DialogHost::open(app, owner, dialog).unwrap();
    app.build_frame().unwrap();
    let scroller = keyed_node(app, "settings-main-scroll").unwrap();
    let bounds = app.tree().visual_rect(scroller).unwrap();
    let content = app.tree().content_size(scroller).unwrap();
    assert!(
        content.h > bounds.h,
        "settings content must exceed its viewport: {content:?}, {bounds:?}"
    );
    let close = keyed_node(app, "settings-close").unwrap();
    let header = app.tree().visual_rect(close).unwrap();
    app.handle_input(&InputEvent::Scroll {
        at: Point::new(bounds.x + bounds.w / 2.0, bounds.y + bounds.h / 2.0),
        delta: ScrollDelta { x: 0.0, y: 120.0 },
    });
    app.build_frame().unwrap();
    assert!(
        app.tree().scroll_offset(scroller).unwrap().y > 0.0,
        "wheel input must move the settings page"
    );
    assert_eq!(app.tree().visual_rect(close).unwrap(), header);
    let offset = app.tree().scroll_offset(scroller).unwrap();
    let page = view::build(
        &controller,
        &core,
        &ArtisanTheme::resolved(),
        &build,
        viewport,
        true,
        Rc::new(|_| {}),
    );
    let dismiss: Rc<dyn Fn()> = Rc::new(|| {});
    host.refresh_content(
        app,
        editor_aqua::shell::settings_frame::settings_panel(
            &ArtisanTheme::resolved(),
            dimensions,
            UiBox::new().build(),
            page,
            &dismiss,
        ),
    )
    .unwrap();
    app.build_frame().unwrap();
    let scroller = keyed_node(app, "settings-main-scroll").unwrap();
    assert_eq!(
        app.tree().scroll_offset(scroller).unwrap(),
        offset,
        "background updates must not reset settings scroll"
    );
}

#[test]
fn general_settings_scroll_inside_the_dialog_without_moving_its_header() {
    for viewport in [
        Size::new(1100.0, 760.0),
        Size::new(900.0, 600.0),
        Size::new(720.0, 480.0),
    ] {
        for scale in [1.0, 2.0] {
            settings_scroll(viewport, scale);
        }
    }
}

#[test]
fn one_wheel_notch_glides_the_settings_page_as_far_as_a_thread() {
    use editor_aqua::shell::wheel::{Surface, WheelGlides};
    let viewport = Size::new(1100.0, 760.0);
    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(directory.path());
    core.navigate(Screen::Settings(SettingsPage::General));
    let controller = SettingsController::default();
    let mut resources = ShellResources::new(1.0).unwrap();
    let app = &mut resources.ui;
    app.set_viewport(viewport.w, viewport.h, 1.0);
    app.mount(|_| UiBox::new().width(viewport.w).height(viewport.h).build())
        .unwrap();
    app.build_frame().unwrap();
    let owner = app.tree().root().unwrap();
    let page = view::build(
        &controller,
        &core,
        &ArtisanTheme::resolved(),
        &BuildInfo::default(),
        viewport,
        true,
        Rc::new(|_| {}),
    );
    let dialog = editor_aqua::shell::settings_frame::settings_dialog(
        &ArtisanTheme::resolved(),
        editor_aqua::shell::layout::Size {
            width: viewport.w,
            height: viewport.h,
        },
        UiBox::new().build(),
        page,
        Rc::new(|| {}),
    );
    let _host = DialogHost::open(app, owner, dialog).unwrap();
    app.build_frame().unwrap();
    let scroller = keyed_node(app, "settings-main-scroll").unwrap();
    let bounds = app.tree().visual_rect(scroller).unwrap();
    let mut glides = WheelGlides::default();
    let mut now = aqua::Instant::ZERO;
    assert_eq!(
        glides.offer(
            app,
            &InputEvent::Scroll {
                at: Point::new(bounds.x + bounds.w / 2.0, bounds.y + bounds.h / 2.0),
                delta: ScrollDelta { x: 0.0, y: 16.0 },
            },
            now,
        ),
        Some(Surface::SettingsPage)
    );
    let first = {
        now += std::time::Duration::from_millis(16);
        glides.advance(app, now);
        app.build_frame().unwrap();
        app.tree().scroll_offset(scroller).unwrap().y
    };
    for _ in 0..60 {
        now += std::time::Duration::from_millis(16);
        let advance = glides.advance(app, now);
        app.build_frame().unwrap();
        if !advance.active {
            break;
        }
    }
    let rested = app.tree().scroll_offset(scroller).unwrap().y;
    assert!(
        first > 0.0 && first < rested,
        "the notch glides instead of jumping: {first} then {rested}"
    );
    assert!(
        (rested - 77.665_63).abs() < 1.0,
        "one notch scrolls three lines, got {rested}"
    );
    assert!(!glides.active());
}

#[test]
fn drifted_global_prompt_page_scrolls_to_its_editor_and_save_button() {
    use artisan_api::tls::{Identity, fingerprint};
    use editor_aqua::core::{ForgeClient, HostConfig, HostId, HostKind};
    use editor_aqua::shell::wheel::{Surface, WheelGlides};
    use std::sync::Arc;

    let viewport = Size::new(1100.0, 760.0);
    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(directory.path());
    let identity = Identity::generate("settings-host").unwrap();
    let fp = fingerprint(identity.certificate_der().unwrap().as_ref());
    let config = HostConfig {
        id: HostId("local".into()),
        name: "local".into(),
        address: "127.0.0.1:1".into(),
        fingerprint: fp.clone(),
        kind: HostKind::Local,
    };
    let client = ForgeClient::new(&config.address, &fp, &identity).unwrap();
    test_support::editor::core::connect_client(&mut core, &config, client);
    let host_id = config.id;
    let external = (0..60)
        .map(|line| format!("# Rule {line}\nKeep line {line} clear and explicit."))
        .collect::<Vec<_>>()
        .join("\n");
    core.host_mut(&host_id).unwrap().global_prompt = Some(Arc::new(artisan_api::GlobalPrompt {
        text: "Artisan prompt".to_owned(),
        version: 1,
        engines: vec![artisan_api::PromptEngineState {
            engine: artisan_api::EngineId::Claude,
            status: artisan_api::PromptEngineStatus::Drifted,
            file: None,
            external_text: Some(external),
            reason: None,
        }],
        candidates: Vec::new(),
    }));
    core.navigate(Screen::Settings(SettingsPage::GlobalPrompt));
    let mut controller = SettingsController::default();
    controller.sync(&mut core);
    let mut resources = ShellResources::new(1.0).unwrap();
    let app = &mut resources.ui;
    app.set_viewport(viewport.w, viewport.h, 1.0);
    app.mount(|_| UiBox::new().width(viewport.w).height(viewport.h).build())
        .unwrap();
    app.build_frame().unwrap();
    let owner = app.tree().root().unwrap();
    let page = view::build(
        &controller,
        &core,
        &ArtisanTheme::resolved(),
        &BuildInfo::default(),
        viewport,
        true,
        Rc::new(|_| {}),
    );
    let dialog = editor_aqua::shell::settings_frame::settings_dialog(
        &ArtisanTheme::resolved(),
        editor_aqua::shell::layout::Size {
            width: viewport.w,
            height: viewport.h,
        },
        UiBox::new().build(),
        page,
        Rc::new(|| {}),
    );
    let _host = DialogHost::open(app, owner, dialog).unwrap();
    app.build_frame().unwrap();
    let scroller = keyed_node(app, "settings-main-scroll").unwrap();
    let editor = keyed_node(app, "settings-global-prompt-editor").unwrap();
    let save = keyed_node(app, "settings-global-prompt-save").unwrap();
    let scroller_rect = app.tree().visual_rect(scroller).unwrap();
    let content = app.tree().content_size(scroller).unwrap();
    assert!(
        content.h > scroller_rect.h,
        "drifted global prompt page must scroll inside settings-main-scroll"
    );
    assert!(app.tree().rect(editor).unwrap().h > 0.0);
    assert!(app.tree().rect(save).unwrap().h > 0.0);
    let mut glides = WheelGlides::default();
    let mut now = aqua::Instant::ZERO;
    assert_eq!(
        glides.offer(
            app,
            &InputEvent::Scroll {
                at: Point::new(scroller_rect.x + 12.0, scroller_rect.y + 12.0),
                delta: ScrollDelta { x: 0.0, y: 16.0 },
            },
            now,
        ),
        Some(Surface::SettingsPage)
    );
    now += std::time::Duration::from_millis(16);
    glides.advance(app, now);
    app.build_frame().unwrap();
    assert!(app.tree().scroll_offset(scroller).unwrap().y > 0.0);
}
