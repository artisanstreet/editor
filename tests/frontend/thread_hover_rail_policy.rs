//! Focused dependency-free coverage for the thread hover-rail policy.
//!
//! The production module is included directly so this harness can be checked
//! with pinned Rust without Cargo, Bazel, or shared module registration.

#![forbid(unsafe_code)]

#[path = "../../modules/frontend/src/thread_hover_rail_policy.rs"]
mod thread_hover_rail_policy;

use thread_hover_rail_policy::{
    CARD_EDGE_INSET, HoverRailAction, Pointer, PointerBounds, Rect, Rectangle,
    ThreadHoverRailPolicy, VISIBLE_WORKING_ROWS, WorkingRowsLayout, working_rows_layout,
};

type StringPolicy = ThreadHoverRailPolicy<String>;

fn rect(left: f64, top: f64, width: f64, height: f64) -> Rect {
    Rectangle::new(left, top, width, height)
}

fn pointer(x: f64, y: f64) -> Pointer {
    Pointer::new(x, y)
}

fn thread(value: &str) -> String {
    value.to_owned()
}

fn bounds(zone: Rectangle, card: Option<Rectangle>) -> PointerBounds {
    PointerBounds::new(Some(zone), card)
}

fn assert_exact_float(actual: f64, expected: f64) {
    assert_eq!(actual.to_bits(), expected.to_bits());
}

#[test]
fn working_row_height_is_zero_for_no_rows() {
    let layout = working_rows_layout(240.0, 0);

    assert_exact_float(layout.row_height(), 0.0);
    assert!(!layout.scrolls());
    assert_eq!(layout.cap_height(), None);
}

#[test]
fn working_row_height_preserves_fractional_and_negative_measurements() {
    let fractional = WorkingRowsLayout::new(101.25, 4);
    assert_exact_float(fractional.row_height(), 25.3125);
    assert!(!fractional.scrolls());

    let negative = WorkingRowsLayout::new(-44.0, 11);
    assert_exact_float(negative.row_height(), -4.0);
    assert!(!negative.scrolls());
    assert_eq!(negative.cap_height(), None);
}

#[test]
fn exactly_ten_rows_do_not_scroll() {
    let layout = WorkingRowsLayout::new(200.0, VISIBLE_WORKING_ROWS);

    assert_exact_float(layout.row_height(), 20.0);
    assert!(!layout.scrolls());
    assert_eq!(layout.cap_height(), None);
}

#[test]
fn more_than_ten_positive_rows_scroll_with_exact_ten_row_cap() {
    let layout = WorkingRowsLayout::new(271.5, 13);

    assert_exact_float(layout.row_height(), 271.5 / 13.0);
    assert!(layout.scrolls());
    assert_eq!(layout.cap_height(), Some((271.5 / 13.0) * 10.0));
}

#[test]
fn nonpositive_row_height_never_enables_scrolling() {
    for total_height in [0.0, -1.0] {
        let layout = WorkingRowsLayout::new(total_height, 11);

        assert!(!layout.scrolls());
        assert_eq!(layout.cap_height(), None);
    }
}

#[test]
fn card_placement_matches_the_unclamped_oracle_formula() {
    let zone = rect(100.5, 40.25, 80.0, 100.0);
    let row = rect(-3.0, 103.75, 20.0, 12.0);
    let mut policy = StringPolicy::new();

    assert!(policy.place_card(row, Some(zone), 24.0));
    assert_exact_float(policy.card_y(), 63.5);

    // A row below the zone is capped by the card's bottom inset.
    let low_row = rect(0.0, 400.0, 1.0, 1.0);
    policy.place_card(low_row, Some(zone), 24.0);
    assert_exact_float(policy.card_y(), 68.0);

    // A row above the zone is still raised to the exact eight-pixel inset;
    // the inputs themselves are not normalized first.
    let high_row = rect(0.0, -100.0, 1.0, 1.0);
    policy.place_card(high_row, Some(zone), 24.0);
    assert_exact_float(policy.card_y(), CARD_EDGE_INSET);
}

#[test]
fn missing_zone_leaves_card_geometry_untouched() {
    let mut policy = StringPolicy::new();
    policy.place_card(
        rect(0.0, 50.0, 1.0, 1.0),
        Some(rect(0.0, 0.0, 100.0, 100.0)),
        20.0,
    );
    let before = policy.card_y();

    assert!(!policy.place_card(rect(0.0, 999.0, 1.0, 1.0), None, 1.0));
    assert_exact_float(policy.card_y(), before);
}

#[test]
fn first_engagement_is_not_travelled_and_retargeting_is_travelled() {
    let zone = rect(10.0, 20.0, 100.0, 200.0);
    let mut policy = StringPolicy::new();

    policy.hover_row(
        thread("first"),
        rect(0.0, 40.0, 10.0, 10.0),
        Some(zone),
        30.0,
    );
    assert_eq!(policy.card_thread().map(String::as_str), Some("first"));
    assert!(policy.card_engaged());
    assert!(!policy.card_travelled());

    policy.hover_row(
        thread("second"),
        rect(0.0, 120.0, 10.0, 10.0),
        Some(zone),
        30.0,
    );
    assert_eq!(policy.card_thread().map(String::as_str), Some("second"));
    assert!(policy.card_engaged());
    assert!(policy.card_travelled());
}

#[test]
fn conceal_clears_engagement_and_travel_but_retains_thread_and_geometry() {
    let zone = rect(10.0, 20.0, 100.0, 200.0);
    let mut policy = StringPolicy::new();
    policy.hover_row(
        thread("first"),
        rect(0.0, 80.0, 10.0, 10.0),
        Some(zone),
        30.0,
    );
    policy.hover_row(
        thread("second"),
        rect(0.0, 140.0, 10.0, 10.0),
        Some(zone),
        30.0,
    );
    let selected = policy.card_thread().cloned();
    let y = policy.card_y();

    policy.conceal();

    assert!(!policy.near());
    assert!(!policy.card_engaged());
    assert!(!policy.card_travelled());
    assert_eq!(policy.card_thread().cloned(), selected);
    assert_eq!(policy.selected_thread().cloned(), selected);
    assert_exact_float(policy.card_y(), y);
}

#[test]
fn suppression_reconciliation_hides_and_forgets_active_proximity_without_replay() {
    let zone = rect(10.0, 20.0, 100.0, 200.0);
    let mut policy = StringPolicy::new();
    policy.hover_row(
        thread("thread"),
        rect(0.0, 50.0, 10.0, 10.0),
        Some(zone),
        20.0,
    );
    assert_eq!(policy.reveal(false), HoverRailAction::RequestNowMs);

    policy.reconcile_suppression(true);
    assert!(!policy.near());
    assert!(!policy.card_engaged());
    assert_eq!(policy.card_thread().map(String::as_str), Some("thread"));

    // Closing the suppressing surface does not replay the remembered hover.
    policy.reconcile_suppression(false);
    assert!(!policy.near());
    assert!(!policy.card_engaged());
    assert_eq!(policy.reveal(true), HoverRailAction::NoOp);
    assert!(!policy.near());
}

#[test]
fn reveal_requests_time_only_on_the_not_near_to_near_transition() {
    let mut policy = StringPolicy::new();

    assert_eq!(policy.reveal(false), HoverRailAction::RequestNowMs);
    assert!(policy.near());
    assert_eq!(policy.reveal(false), HoverRailAction::NoOp);
    assert_eq!(policy.reveal(true), HoverRailAction::NoOp);

    policy.conceal();
    assert_eq!(policy.reveal(true), HoverRailAction::NoOp);
    assert!(!policy.near());
    assert!(HoverRailAction::NoOp.is_no_op());
    assert!(!HoverRailAction::NoOp.requests_now_ms());
}

#[test]
fn rectangle_edges_are_inclusive() {
    let zone = rect(10.0, 20.0, 30.0, 40.0);

    for edge in [
        pointer(10.0, 20.0),
        pointer(40.0, 20.0),
        pointer(10.0, 60.0),
        pointer(40.0, 60.0),
    ] {
        assert!(zone.contains_inclusive(edge));
    }
    assert!(!zone.contains_inclusive(pointer(40.0001, 60.0)));
}

#[test]
fn zero_and_negative_width_rectangles_are_absent() {
    for width in [0.0, -1.0] {
        let zone = rect(10.0, 20.0, width, 40.0);
        let mut policy = StringPolicy::new();

        assert!(!zone.contains_inclusive(pointer(10.0, 20.0)));
        assert_eq!(
            policy.track_pointer(pointer(10.0, 20.0), bounds(zone, None), false),
            HoverRailAction::NoOp
        );
        assert!(!policy.near());
    }
}

#[test]
fn pointer_memory_updates_even_when_geometry_is_missing_or_suppressed() {
    let mut policy = StringPolicy::new();
    let remembered = pointer(-12.5, 4.25);

    let _ = policy.track_pointer(remembered, PointerBounds::default(), false);
    assert_eq!(policy.last_pointer(), remembered);

    let zone = rect(0.0, 0.0, 100.0, 100.0);
    let _ = policy.track_pointer(pointer(10.0, 10.0), bounds(zone, None), false);
    assert!(policy.near());
    assert!(policy.proximity_active());
    let _ = policy.track_pointer(pointer(-2.0, 0.5), bounds(zone, None), true);

    assert_eq!(policy.last_pointer(), pointer(-2.0, 0.5));
    assert!(!policy.near());
}

#[test]
fn suppressed_pointer_tracking_conceals_without_requesting_time() {
    let zone = rect(0.0, 0.0, 100.0, 100.0);
    let mut policy = StringPolicy::new();
    policy.hover_row(
        thread("thread"),
        rect(0.0, 10.0, 10.0, 10.0),
        Some(zone),
        20.0,
    );
    let _ = policy.reveal(false);

    assert_eq!(
        policy.track_pointer(pointer(10.0, 10.0), bounds(zone, None), true),
        HoverRailAction::NoOp
    );
    assert!(!policy.near());
    assert!(!policy.card_engaged());
}

#[test]
fn gap_between_zone_and_engaged_card_is_reachable() {
    let zone = rect(0.0, 0.0, 100.0, 100.0);
    let card = rect(120.0, 30.0, 80.0, 40.0);
    let mut policy = StringPolicy::new();
    policy.hover_row(
        thread("thread"),
        rect(0.0, 30.0, 10.0, 10.0),
        Some(zone),
        20.0,
    );

    assert_eq!(
        policy.track_pointer(pointer(110.0, 50.0), bounds(zone, Some(card)), false),
        HoverRailAction::RequestNowMs
    );
    assert!(policy.near());

    // The card's original left edge is 120, but the reachable rectangle
    // starts at the zone's right edge (100), bridging the ten-pixel gap.
    assert_eq!(
        PointerBounds::new(Some(zone), Some(card)).reachable_card(),
        Some(Rectangle::from_edges(100.0, 30.0, 200.0, 70.0))
    );
}

#[test]
fn an_unengaged_card_does_not_widen_the_hit_band() {
    let zone = rect(0.0, 0.0, 100.0, 100.0);
    let card = rect(120.0, 30.0, 80.0, 40.0);
    let mut policy = StringPolicy::new();

    assert_eq!(
        policy.track_pointer(pointer(110.0, 50.0), bounds(zone, Some(card)), false),
        HoverRailAction::NoOp
    );
    assert!(!policy.near());
}

#[test]
fn leaving_zone_and_card_conceals_only_active_proximity() {
    let zone = rect(0.0, 0.0, 100.0, 100.0);
    let card = rect(120.0, 30.0, 80.0, 40.0);
    let mut policy = StringPolicy::new();
    policy.hover_row(
        thread("thread"),
        rect(0.0, 30.0, 10.0, 10.0),
        Some(zone),
        20.0,
    );

    // With proximity inactive, leaving both rectangles is an intentional
    // no-op and does not hide a separately engaged card.
    let _ = policy.track_pointer(pointer(250.0, 50.0), bounds(zone, Some(card)), false);
    assert!(!policy.near());
    assert!(policy.card_engaged());

    let _ = policy.reveal(false);
    assert!(policy.near());
    let _ = policy.track_pointer(pointer(250.0, 50.0), bounds(zone, Some(card)), false);
    assert!(!policy.near());
    assert!(!policy.card_engaged());
}

#[test]
fn inclusive_reachable_card_edges_and_fractional_coordinates_are_preserved() {
    let zone = rect(-20.5, -10.25, 30.25, 40.5);
    let card = rect(25.75, 12.5, 10.5, 20.25);
    let mut policy = StringPolicy::new();
    policy.hover_row(
        thread("fractional"),
        rect(0.0, 0.0, 1.0, 1.0),
        Some(zone),
        12.0,
    );

    for edge in [
        pointer(9.75, 12.5),
        pointer(36.25, 12.5),
        pointer(9.75, 32.75),
        pointer(36.25, 32.75),
    ] {
        policy.conceal();
        policy.hover_row(
            thread("fractional"),
            rect(0.0, 0.0, 1.0, 1.0),
            Some(zone),
            12.0,
        );
        assert_eq!(
            policy.track_pointer(edge, bounds(zone, Some(card)), false),
            HoverRailAction::RequestNowMs
        );
    }
}

#[test]
fn focus_departure_uses_pointer_memory_to_keep_an_open_zone_alive() {
    let zone = rect(10.0, 20.0, 100.0, 100.0);
    let mut policy = StringPolicy::new();
    let _ = policy.track_pointer(pointer(40.0, 50.0), bounds(zone, None), false);
    assert!(policy.near());

    policy.focus_departure(false, Some(zone));
    assert!(policy.near());

    policy.focus_departure(true, None);
    assert!(policy.near());

    policy.focus_departure(false, Some(zone));
    let _ = policy.track_pointer(pointer(200.0, 200.0), bounds(zone, None), false);
    assert!(!policy.near());
}

#[test]
fn no_op_outside_pointer_sequence_does_not_mutate_card_when_not_near() {
    let zone = rect(0.0, 0.0, 100.0, 100.0);
    let mut policy = StringPolicy::new();
    policy.hover_row(
        thread("retained"),
        rect(0.0, 50.0, 10.0, 10.0),
        Some(zone),
        20.0,
    );
    let before_thread = policy.card_thread().cloned();
    let before_y = policy.card_y();

    assert_eq!(
        policy.track_pointer(pointer(500.0, -500.0), bounds(zone, None), false),
        HoverRailAction::NoOp
    );
    assert_eq!(policy.card_thread().cloned(), before_thread);
    assert_exact_float(policy.card_y(), before_y);
    assert!(policy.card_engaged());
    assert_eq!(policy.last_pointer(), pointer(500.0, -500.0));
}

#[test]
fn default_state_matches_new_and_pointer_aliases_preserve_values() {
    let policy = ThreadHoverRailPolicy::<String>::default();

    assert_eq!(policy, StringPolicy::new());
    assert_eq!(policy.last_pointer(), pointer(0.0, 0.0));
    assert_exact_float(policy.last_pointer().client_x(), 0.0);
    assert_exact_float(policy.last_pointer().client_y(), 0.0);
}
