use editor_aqua::core::SettingsPage;
use editor_aqua::sidebar::settings_nav;
use editor_aqua::ui::theme::ArtisanTheme;
use std::{cell::Cell, rc::Rc, time::Instant};
use test_support::editor::ui::{click, mounted};

#[test]
fn computers_can_be_opened_directly_from_general_settings() {
    let selected = Rc::new(Cell::new(None));
    let sink = Rc::clone(&selected);
    let emit: Rc<dyn Fn(SettingsPage)> = Rc::new(move |page| sink.set(Some(page)));
    let theme = ArtisanTheme::resolved();
    let nav = settings_nav::build(
        None,
        SettingsPage::General,
        &theme,
        &settings_nav::NavMotion::default(),
        Instant::now(),
        true,
        &emit,
    );
    let mut app = mounted(276.0, 700.0, nav);
    click(&mut app, "sidebar-settings-computers");
    assert_eq!(selected.get(), Some(SettingsPage::Hosts));
}
