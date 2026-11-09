use aqua_core::{InputEvent, Point, ScrollDelta, Size};
use aqua_ui::element::Box as UiBox;
use editor_aqua::core::{Core, EditorFiles, Screen, SettingsPage};
use editor_aqua::settings::{about::BuildInfo, controller::SettingsController, view};
use editor_aqua::shell::{host::ShellResources, wheel::keyed_node};
use editor_aqua::ui::{dialog::DialogHost, theme::ArtisanTheme};
use std::rc::Rc;

fn settings_scroll(viewport: Size, scale: f32) {
    let directory = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: directory.path().into(),
    })
    .unwrap();
    core.navigate(Screen::Settings(SettingsPage::General));
    let controller = SettingsController::default();
    let build = BuildInfo {
        version: String::new(),
        commit: String::new(),
        profile: String::new(),
        target: String::new(),
    };
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
        ArtisanTheme::resolved(),
        &build,
        viewport,
        true,
        Rc::new(|_| {}),
    );
    let dialog = editor_aqua::shell::view::settings_dialog(
        ArtisanTheme::resolved(),
        dimensions,
        UiBox::new().build(),
        page,
        Rc::new(|| {}),
    );
    let host = DialogHost::open(app, owner, dialog).unwrap();
    app.build_frame().unwrap();
    let scroller = keyed_node(app, "settings-screen-General").unwrap();
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
        ArtisanTheme::resolved(),
        &build,
        viewport,
        true,
        Rc::new(|_| {}),
    );
    host.refresh_content(
        app,
        editor_aqua::shell::view::settings_panel(
            ArtisanTheme::resolved(),
            dimensions,
            UiBox::new().build(),
            page,
            Rc::new(|| {}),
        ),
    )
    .unwrap();
    app.build_frame().unwrap();
    let scroller = keyed_node(app, "settings-screen-General").unwrap();
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
