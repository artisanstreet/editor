use aqua_core::{Rect, Size};
use aqua_ui::overlay::Side;
use editor_aqua::ui::hover_tooltip::{TooltipPlacement, tooltip_plan};

#[test]
fn beside_chooses_roomier_side_and_caps_width_to_actual_space() {
    let viewport = Size::new(500.0, 300.0);
    let placement = TooltipPlacement::Beside { max_width: 320.0 };
    let right = tooltip_plan(
        Rect::from_edges(20.0, 40.0, 40.0, 60.0),
        viewport,
        placement,
    );
    assert_eq!(right.side, Side::After);
    assert_eq!(right.width, 320.0);
    assert!(right.fixed);
    let left = tooltip_plan(
        Rect::from_edges(460.0, 40.0, 480.0, 60.0),
        viewport,
        placement,
    );
    assert_eq!(left.side, Side::Before);
    assert_eq!(left.width, 320.0);
    let narrow = tooltip_plan(
        Rect::from_edges(200.0, 40.0, 300.0, 60.0),
        viewport,
        placement,
    );
    assert_eq!(narrow.side, Side::After);
    assert_eq!(narrow.width, 192.0);
}

#[test]
fn above_is_content_sized_with_viewport_capped_width() {
    let plan = tooltip_plan(
        Rect::from_edges(10.0, 10.0, 20.0, 20.0),
        Size::new(180.0, 100.0),
        TooltipPlacement::Above { max_width: 240.0 },
    );
    assert_eq!(plan.side, Side::Above);
    assert_eq!(plan.width, 180.0);
    assert!(!plan.fixed);
}
