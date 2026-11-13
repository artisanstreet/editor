use editor_aqua::core::{Core, EditorFiles, Screen, SettingsPage};
use editor_aqua::shell::host::ShellResources;
use editor_aqua::sidebar::{controller::Sidebar, settings_nav};
use editor_aqua::ui::theme::ArtisanTheme;
use std::{
    rc::Rc,
    time::{Duration, Instant},
};

#[test]
fn late_first_frame_finishes_settings_expansion_without_pointer_input() {
    let directory = tempfile::tempdir().unwrap();
    let mut core = Core::for_tests(EditorFiles {
        dir: directory.path().to_owned(),
    })
    .unwrap();
    core.navigate(Screen::Settings(SettingsPage::General));
    assert_eq!(core.screen(), &Screen::Settings(SettingsPage::General));
    let mut sidebar = Sidebar::new(false);
    let initial = Instant::now();
    sidebar.sync(&core, artisan_api::now());
    let terminal = Instant::now() + Duration::from_secs(1);
    let theme = ArtisanTheme::resolved();
    let build = |sidebar: &Sidebar, now| {
        settings_nav::build(
            sidebar.host_data(&core),
            SettingsPage::General,
            theme,
            &sidebar.settings_motion,
            now,
            false,
            Rc::new(|_| {}),
        )
    };
    let mut resources = ShellResources::new(1.25).unwrap();
    resources.ui.set_viewport(300.0, 800.0, 1.25);
    resources.ui.mount(|_| build(&sidebar, initial)).unwrap();
    resources.ui.build_frame().unwrap();
    let about = |ui: &aqua::UiApp| {
        let node = ui
            .a11y_nodes()
            .unwrap()
            .2
            .into_iter()
            .find(|node| {
                node.semantic.role == aqua_core::Role::Button && node.semantic.label == "About"
            })
            .expect("the About button must be exposed to accessibility");
        (
            ui.tree().visual_rect(node.semantic.node).unwrap(),
            node.bounds.unwrap(),
        )
    };
    let (collapsed, collapsed_access) = about(&resources.ui);
    assert!(
        sidebar.advance_motion(terminal, false),
        "a first frame arriving after the animation duration must still request its completion rebuild"
    );
    resources
        .ui
        .refresh(vec![build(&sidebar, terminal)])
        .unwrap();
    resources.ui.build_frame().unwrap();
    let (expanded, expanded_access) = about(&resources.ui);
    let children = settings_nav::children_height(4);
    assert!(
        (expanded.y - collapsed.y - children).abs() < 1.0 / 1.25,
        "the completion frame must expose all four General children without pointer input"
    );
    assert!(
        (expanded_access.y - collapsed_access.y - children * 1.25).abs() < 1.0,
        "accessibility must follow the completed layout in physical pixels at 1.25 DPI"
    );
    assert_eq!(
        expanded_access.y,
        expanded.y * 1.25,
        "the final About hit target and accessibility rectangle must agree"
    );
    assert!(
        !sidebar.advance_motion(terminal + Duration::from_millis(1), false),
        "the completion rebuild must be requested only once after motion has stopped"
    );
}
