use crate::row::painted_with_keyboard_focus;
use aqua_ui::element::Box as UiBox;
use editor_ui::kit::HitArea;
use editor_ui::theme::{ArtisanTheme, ThemeMode};

#[test]
fn a_hit_area_reached_with_tab_shows_a_focus_ring() {
    let theme = ArtisanTheme::for_mode(ThemeMode::Dark);
    let area = HitArea::new("area", theme, aqua_core::Role::Button, "Search")
        .child(UiBox::new().width(40.0).height(40.0).build())
        .on_activate(|| {})
        .build();
    let (resting, focused) = painted_with_keyboard_focus(
        UiBox::new().width(40.0).height(40.0).child(area).build(),
        "area",
    );
    assert!(
        focused > resting,
        "keyboard focus paints an indicator ({resting} items at rest, {focused} focused)"
    );
}
