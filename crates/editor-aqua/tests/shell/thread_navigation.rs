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
fn menu_centering_clamps_to_the_actual_scrollable_extent() {
    assert_eq!(navigator::menu_scroll_offset(0, 50, 180.0), 0.0);
    assert_eq!(navigator::menu_scroll_offset(10, 50, 180.0), 288.0);
    assert_eq!(navigator::menu_scroll_offset(49, 50, 180.0), 1620.0);
    assert_eq!(navigator::menu_scroll_offset(1, 2, 180.0), 0.0);
}

fn outline(count: usize) -> Vec<OutlineEntry> {
    (0..count)
        .map(|index| OutlineEntry {
            id: ItemId::from(format!("item-{index}")),
            position: index as Seq * 10,
            label: format!("turn {index}"),
        })
        .collect()
}

fn built_navigator(count: usize, viewport_height: f32, active: Seq) -> navigator::Navigator {
    let mut navigator = navigator::Navigator::default();
    let dispatch: Dispatch = Rc::new(|_| {});
    let rail = navigator.rail(
        &outline(count),
        Some(active),
        viewport_height,
        None,
        0.0,
        false,
        Instant::now(),
        ArtisanTheme::resolved(),
        dispatch,
    );
    assert!(rail.is_some());
    navigator
}

#[test]
fn active_shift_stays_in_place_while_the_tick_set_is_unchanged() {
    let mut navigator = built_navigator(10, 600.0, 0);
    assert!(navigator.shift_active(3, Instant::now(), false));
    assert!(navigator.shift_active(9, Instant::now(), false));
}

#[test]
fn active_shift_asks_for_a_rebuild_when_the_tick_set_changes() {
    let mut navigator = built_navigator(500, 600.0, 0);
    assert!(!navigator.shift_active(250, Instant::now(), false));
    assert!(!navigator.shift_active(1_000, Instant::now(), false));
}

#[test]
fn tick_presence_runs_from_start_to_target_and_reports_flight() {
    let start = Instant::now();
    let (begin, begin_moving) = navigator::tick_presence(true, Some(start), start);
    assert_eq!(begin, 0.0);
    assert!(begin_moving);
    let (end, end_moving) =
        navigator::tick_presence(true, Some(start), start + Duration::from_secs(2));
    assert_eq!(end, 1.0);
    assert!(!end_moving);
    let (fading, _) = navigator::tick_presence(false, Some(start), start);
    assert_eq!(fading, 1.0);
    let (idle, idle_moving) = navigator::tick_presence(false, None, start);
    assert_eq!(idle, 0.0);
    assert!(!idle_moving);
}
