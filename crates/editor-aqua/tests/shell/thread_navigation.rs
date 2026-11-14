use super::*;
use artisan_api::ItemId;

#[test]
fn latest_is_offered_only_for_a_current_reading_thread_with_composer() {
    assert!(show_latest(true, true, false, false, true));
    assert!(!show_latest(false, true, false, false, true));
    assert!(!show_latest(true, false, false, false, true));
    assert!(!show_latest(true, true, true, false, true));
    assert!(!show_latest(true, true, false, true, true));
    assert!(!show_latest(true, true, false, false, false));
}

#[test]
fn active_turn_tracks_latest_outline_marker_at_or_before_reader() {
    let markers: Vec<_> = [10, 40, 100]
        .into_iter()
        .enumerate()
        .map(|(index, position)| OutlineEntry {
            id: ItemId::from(format!("item-{index}")),
            position,
            label: format!("turn {index}"),
        })
        .collect();
    assert_eq!(active_index(&markers, None), 0);
    assert_eq!(active_index(&markers, Some(9)), 0);
    assert_eq!(active_index(&markers, Some(40)), 1);
    assert_eq!(active_index(&markers, Some(99)), 1);
    assert_eq!(active_index(&markers, Some(100)), 2);
}

#[test]
fn panel_centering_clamps_to_the_actual_scrollable_extent() {
    assert_eq!(navigator::panel_scroll_offset(0, 50, 180.0), 0.0);
    assert_eq!(navigator::panel_scroll_offset(10, 50, 180.0), 288.0);
    assert_eq!(navigator::panel_scroll_offset(49, 50, 180.0), 1620.0);
    assert_eq!(navigator::panel_scroll_offset(1, 2, 180.0), 0.0);
}

#[test]
fn the_outline_toggles_only_when_the_thread_has_turns() {
    let now = Instant::now();
    let mut navigation = ThreadNavigation::default();
    navigation.toggle(now, false);
    assert_eq!(navigation.chevron(now), None);
    assert!(!navigation.interaction.open());
    navigation.interaction.available = true;
    navigation.toggle(now, false);
    assert!(navigation.interaction.open());
    assert!(navigation.interaction.recentre);
}

#[test]
fn the_panel_opens_slower_than_it_closes_and_the_chevron_flips_fastest() {
    let start = Instant::now();
    let at = |ms| start + Duration::from_millis(ms);
    let mut navigation = ThreadNavigation::default();
    navigation.interaction.available = true;
    navigation.toggle(start, false);
    assert_eq!(navigation.chevron(start), Some(0.0));
    let panel = navigation.interaction.panel.value(at(100), OPEN);
    assert!(navigation.chevron(at(100)).is_some_and(|flip| flip > panel && flip < 1.0));
    assert_eq!(navigation.chevron(at(250)), Some(1.0));
    assert!(navigation.interaction.panel.value(at(250), OPEN) < 1.0);
    assert_eq!(navigation.interaction.panel.value(at(400), OPEN), 1.0);
    assert!(navigation.interaction.settle(at(400)));
    assert!(!navigation.interaction.settle(at(400)));
    navigation.toggle(at(400), false);
    assert!(!navigation.interaction.open());
    assert!(navigation.interaction.panel.value(at(600), CLOSE) > 0.0);
    assert_eq!(navigation.interaction.panel.value(at(750), CLOSE), 0.0);
    assert_eq!(navigation.chevron(at(650)), Some(0.0));
}

#[test]
fn reduced_motion_snaps_the_panel_and_chevron() {
    let now = Instant::now();
    let mut navigation = ThreadNavigation::default();
    navigation.interaction.available = true;
    navigation.toggle(now, true);
    assert_eq!(navigation.chevron(now), Some(1.0));
    assert_eq!(navigation.interaction.panel.value(now, OPEN), 1.0);
    assert!(!navigation.interaction.settle(now));
}

#[test]
fn disposing_the_view_closes_the_outline() {
    let now = Instant::now();
    let mut navigation = ThreadNavigation::default();
    navigation.interaction.available = true;
    navigation.toggle(now, true);
    navigation.dispose();
    assert_eq!(navigation.chevron(now), None);
    assert!(!navigation.interaction.open());
}
