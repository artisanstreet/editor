use artisan_payload::licences::Licence;

#[test]
fn the_about_page_mounts_only_the_licences_in_view() {
    static MANY: std::sync::LazyLock<Vec<Licence>> = std::sync::LazyLock::new(|| {
        (0..400)
            .map(|index| Licence {
                name: Box::leak(format!("crate-{index:03}").into_boxed_str()),
                text: "MIT",
            })
            .collect()
    });
    use aqua_ui::element::Box as UiBox;
    use editor_aqua::core::{Screen, SettingsPage};
    use editor_aqua::settings::{about::BuildInfo, controller::SettingsController, view};
    use editor_aqua::shell::{host::ShellResources, wheel::keyed_node};
    use editor_aqua::ui::theme::ArtisanTheme;
    let viewport = aqua_core::Size::new(1100.0, 760.0);
    let directory = tempfile::tempdir().unwrap();
    let mut core = test_support::editor::core::test_core(directory.path());
    core.navigate(Screen::Settings(SettingsPage::About));
    let build = BuildInfo {
        licences: MANY.as_slice(),
        ..BuildInfo::default()
    };
    let mut resources = ShellResources::new(1.0).unwrap();
    let app = &mut resources.ui;
    app.set_viewport(viewport.w, viewport.h, 1.0);
    let page = view::build(
        &SettingsController::default(),
        &core,
        &ArtisanTheme::resolved(),
        &build,
        viewport,
        true,
        std::rc::Rc::new(|_| {}),
    );
    app.mount(move |_| {
        UiBox::new()
            .width(viewport.w)
            .height(viewport.h)
            .child(page)
            .build()
    })
    .unwrap();
    app.build_frame().unwrap();
    app.build_frame().unwrap();
    let mounted: Vec<usize> = (0..400)
        .filter(|index| keyed_node(app, &format!("settings-licence-{index}")).is_some())
        .collect();
    assert!(
        mounted.contains(&0) && mounted.len() < 40,
        "only a window of licence rows is mounted: {} rows",
        mounted.len()
    );
}
