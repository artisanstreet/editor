use super::*;
use crate::ui::kit::Row;
use aqua_core::{Key, KeyEvent, NamedKey, Point, Rect};
use artisan_api::ItemId;

const PANEL_PITCH: f32 = match Row::MenuItem.height() {
    Some(height) => height,
    None => 0.0,
};

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
    test_support::assert_near!(navigator::panel_scroll_offset(0, 50, 180.0), 0.0);
    test_support::assert_near!(navigator::panel_scroll_offset(10, 50, 180.0), 288.0);
    test_support::assert_near!(navigator::panel_scroll_offset(49, 50, 180.0), 1620.0);
    test_support::assert_near!(navigator::panel_scroll_offset(1, 2, 180.0), 0.0);
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
fn the_panel_and_chevron_open_then_close_and_settle() {
    let start = Instant::now();
    let later = start + Duration::from_secs(1);
    let closed = later + Duration::from_secs(1);
    let mut navigation = ThreadNavigation::default();
    navigation.interaction.available = true;
    navigation.toggle(start, false);
    assert_eq!(navigation.chevron(start), Some(0.0));
    assert!(navigation.interaction.panel.value(start, OPEN) < 1.0);
    assert_eq!(navigation.chevron(later), Some(1.0));
    test_support::assert_near!(navigation.interaction.panel.value(later, OPEN), 1.0);
    assert!(navigation.interaction.settle(later));
    assert!(!navigation.interaction.settle(later));
    navigation.toggle(later, false);
    assert!(!navigation.interaction.open());
    assert!(navigation.interaction.panel.value(later, CLOSE) > 0.0);
    test_support::assert_near!(navigation.interaction.panel.value(closed, CLOSE), 0.0);
    assert_eq!(navigation.chevron(closed), Some(0.0));
}

#[test]
fn reduced_motion_snaps_the_panel_and_chevron() {
    let now = Instant::now();
    let mut navigation = ThreadNavigation::default();
    navigation.interaction.available = true;
    navigation.toggle(now, true);
    assert_eq!(navigation.chevron(now), Some(1.0));
    test_support::assert_near!(navigation.interaction.panel.value(now, OPEN), 1.0);
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

#[test]
fn escape_closes_only_an_open_outline() {
    let now = Instant::now();
    let mut navigation = ThreadNavigation::default();
    navigation.interaction.available = true;
    assert!(!navigation.close(now, false));
    navigation.toggle(now, false);
    assert!(navigation.close(now, false));
    assert!(!navigation.interaction.open());
    assert!(!navigation.close(now, false));
}

fn opened(entries: Vec<Seq>, cursor: usize) -> ThreadNavigation {
    let mut navigation = ThreadNavigation::default();
    navigation.interaction.available = true;
    navigation.toggle(Instant::now(), true);
    navigation.interaction.entries = entries;
    navigation.interaction.cursor = Some(cursor);
    navigation
}

fn press(key: NamedKey, modifiers: aqua_core::Modifiers) -> KeyEvent {
    KeyEvent {
        key: Key::Named(key),
        modifiers,
        repeat: false,
    }
}

#[test]
fn arrow_keys_walk_the_outline_and_stop_at_its_ends() {
    let mut navigation = opened(vec![10, 20, 30], 1);
    let walk = |navigation: &mut ThreadNavigation, step| {
        navigation.interaction.steer(step);
        navigation.interaction.cursor
    };
    assert_eq!(walk(&mut navigation, Step::Down), Some(2));
    assert_eq!(walk(&mut navigation, Step::Down), Some(2));
    assert_eq!(walk(&mut navigation, Step::Up), Some(1));
    assert_eq!(walk(&mut navigation, Step::First), Some(0));
    assert_eq!(walk(&mut navigation, Step::Up), Some(0));
    assert_eq!(walk(&mut navigation, Step::Last), Some(2));
    assert_eq!(navigation.interaction.reveal, Some(Reveal::Keep(2)));
}

#[test]
fn enter_jumps_to_the_turn_under_the_cursor() {
    let mut navigation = opened(vec![10, 20, 30], 1);
    assert_eq!(navigation.interaction.steer(Step::Jump), Steer::Jump(20));
    navigation.interaction.cursor = Some(7);
    assert_eq!(navigation.interaction.steer(Step::Jump), Steer::Jump(30));
}

#[test]
fn a_closed_outline_leaves_the_keys_alone() {
    let mut navigation = opened(vec![10, 20, 30], 1);
    navigation.close(Instant::now(), true);
    assert_eq!(navigation.interaction.steer(Step::Down), Steer::Ignored);
    assert!(!navigation.key(&press(NamedKey::Down, aqua_core::Modifiers::NONE)));
}

#[test]
fn only_bare_navigation_keys_steer_the_outline() {
    let bare = aqua_core::Modifiers::NONE;
    assert_eq!(step(&press(NamedKey::Down, bare)), Some(Step::Down));
    assert_eq!(step(&press(NamedKey::Enter, bare)), Some(Step::Jump));
    assert_eq!(
        step(&press(NamedKey::Down, aqua_core::Modifiers::SHIFT)),
        None
    );
    assert_eq!(step(&press(NamedKey::Escape, bare)), None);
}

#[test]
fn opening_puts_the_cursor_on_the_turn_being_read() {
    let mut navigation = opened(vec![10, 20, 30], 2);
    navigation.close(Instant::now(), true);
    navigation.toggle(Instant::now(), true);
    assert!(navigation.interaction.recentre);
}

#[test]
fn clicks_close_the_outline_only_outside_the_drawer_and_chevron() {
    let drawer = Some(Rect {
        x: 0.0,
        y: 0.0,
        w: 320.0,
        h: 600.0,
    });
    let chevron = Some(Rect {
        x: 400.0,
        y: -30.0,
        w: 16.0,
        h: 16.0,
    });
    assert!(!outside(Point::new(100.0, 580.0), drawer, chevron));
    assert!(!outside(Point::new(405.0, -25.0), drawer, chevron));
    assert!(outside(Point::new(500.0, 300.0), drawer, chevron));
    assert!(outside(Point::new(500.0, 300.0), None, None));
}

#[test]
fn the_cursor_scrolls_the_panel_only_when_it_leaves_the_view() {
    let pitch = PANEL_PITCH;
    let view = pitch * 5.0;
    test_support::assert_near!(navigator::panel_keep_offset(2, 0.0, view), 0.0);
    test_support::assert_near!(navigator::panel_keep_offset(5, 0.0, view), pitch);
    test_support::assert_near!(
        navigator::panel_keep_offset(3, pitch * 4.0, view),
        pitch * 3.0
    );
}

#[test]
fn jump_to_latest_pops_in_slower_than_it_leaves() {
    let now = Instant::now();
    let at = |ms| now + Duration::from_millis(ms);
    let mut latest = LatestPresence::default();
    latest.show(true, now, false);
    test_support::assert_near!(latest.value(now), 0.0);
    assert!(latest.value(at(149)) < 1.0);
    test_support::assert_near!(latest.value(at(250)), 1.0);
    latest.show(false, at(250), false);
    assert!(latest.value(at(300)) > 0.0);
    test_support::assert_near!(latest.value(at(400)), 0.0);
}

#[test]
fn jump_to_latest_keeps_its_glide_while_visibility_holds() {
    let now = Instant::now();
    let mut latest = LatestPresence::default();
    latest.show(true, now, false);
    let midway = now + Duration::from_millis(100);
    let before = latest.value(midway);
    latest.show(true, midway, false);
    test_support::assert_near!(latest.value(midway), before);
}

#[test]
fn reduced_motion_shows_and_hides_jump_to_latest_instantly() {
    let now = Instant::now();
    let mut latest = LatestPresence::default();
    latest.show(true, now, true);
    test_support::assert_near!(latest.value(now), 1.0);
    latest.show(false, now, true);
    test_support::assert_near!(latest.value(now), 0.0);
}

fn mounted_drawer(progress: f32) -> aqua::UiApp {
    use aqua_text::{SharedFonts, SharedRuns};
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(move |_| {
        let theme = ArtisanTheme::resolved();
        let panel = crate::ui::kit::surface::drawer::build(
            &theme,
            navigator::PANEL_WIDTH,
            progress,
            Vec::new(),
        )
        .with_key(OVERLAY_KEY);
        UiBox::new()
            .width(900.0)
            .height(600.0)
            .child(crate::conversation::view::thread_frame(
                UiBox::new().build(),
                Vec::new(),
                None,
                Some(panel),
                900.0,
                1.0,
            ))
            .build()
    })
    .unwrap();
    app.set_viewport(900.0, 600.0, 1.0);
    app.build_frame().unwrap();
    app
}

#[test]
fn clicking_beside_the_open_drawer_closes_it_and_clicking_inside_does_not() {
    let now = Instant::now();
    let app = mounted_drawer(1.0);
    let mut navigation = opened(vec![10, 20], 0);
    assert!(!navigation.dismiss(&app, Point::new(160.0, 590.0), now, true));
    assert!(navigation.interaction.open());
    let gutter = navigator::PANEL_WIDTH + 8.0;
    assert!(navigation.dismiss(&app, Point::new(gutter, 300.0), now, true));
    assert!(!navigation.interaction.open());
}
