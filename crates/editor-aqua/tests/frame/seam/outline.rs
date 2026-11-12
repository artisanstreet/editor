use aqua_core::vector::PathCommand;
use aqua_core::{Point, Rect};
use editor_aqua::frame::seam::outline::trace;

#[test]
fn the_outline_does_not_cross_the_tab_opening() {
    let body = Rect::from_edges(0.0, 48.0, 800.0, 548.0);
    for left in [0.0, 4.0, 120.0, 500.0, 700.0] {
        let tab = Rect::from_edges(left, 8.0, left + 140.0, 48.0);
        for inset in [0.0, 1.0, 2.0] {
            let mut from = Point::ZERO;
            let center = (tab.x + tab.w * 0.5).min(body.right() - 4.0);
            for command in trace(body, Some(tab), inset) {
                from = match command {
                    PathCommand::Move(to) | PathCommand::Arc { to, .. } => to,
                    PathCommand::Line(to) => {
                        assert!(
                            from.y != body.y + inset
                                || to.y != from.y
                                || !(from.x.min(to.x) < center && center < from.x.max(to.x)),
                            "the card's horizontal outline must stop at the selected tab"
                        );
                        to
                    }
                    _ => from,
                };
            }
        }
    }
}

#[test]
fn insets_keep_the_same_width_around_convex_and_concave_corners() {
    let body = Rect::from_edges(0.0, 48.0, 800.0, 548.0);
    let tab = Rect::from_edges(200.0, 8.0, 400.0, 48.0);
    for inset in [0.0, 1.0, 2.0] {
        let corners = trace(body, Some(tab), inset)
            .into_iter()
            .filter_map(|command| match command {
                PathCommand::Arc { radii, sweep, .. } => Some((radii.x, sweep)),
                _ => None,
            })
            .collect::<Vec<_>>();
        let cap = editor_aqua::frame::strip::RADIUS.px();
        assert_eq!(corners[0], (cap + inset, false));
        assert_eq!(corners[1], (cap - inset, true));
        assert_eq!(corners[2], (cap - inset, true));
        assert_eq!(corners[3], (cap + inset, false));
    }
}

#[test]
fn tabs_outside_the_sheet_leave_an_unbroken_card_outline() {
    let body = Rect::from_edges(0.0, 48.0, 800.0, 548.0);
    for tab in [
        Rect::from_edges(-250.0, 8.0, -50.0, 48.0),
        Rect::from_edges(850.0, 8.0, 1050.0, 48.0),
    ] {
        for inset in [0.0, 1.0, 2.0] {
            assert_eq!(trace(body, Some(tab), inset), trace(body, None, inset));
        }
    }
}

#[test]
fn moving_lips_only_rasterize_the_top_of_the_sheet() {
    use editor_aqua::frame::seam::outline::split;
    for height in [100.0, 500.0, 2000.0] {
        let sheet = Rect::from_edges(0.0, 48.0, 800.0, 48.0 + height);
        for left in [0.0, 120.0, 500.0] {
            for inset in [0.0, 1.0, 2.0] {
                let tab = Rect::from_edges(left, 8.0, left + 140.0, 48.0);
                let (top, body) = split(sheet, Some(tab), inset);
                let max_y = top
                    .iter()
                    .filter_map(|command| match command {
                        PathCommand::Move(to)
                        | PathCommand::Line(to)
                        | PathCommand::Arc { to, .. } => Some(to.y),
                        _ => None,
                    })
                    .fold(f32::NEG_INFINITY, f32::max);
                assert_eq!(max_y, body.y + 1.0);
                assert!(max_y <= sheet.y + 20.0);
                assert_eq!(body.bottom(), sheet.bottom() - inset);
                assert_eq!(body.x, sheet.x + inset);
                assert_eq!(body.right(), sheet.right() - inset);
                assert_eq!(top.last(), Some(&PathCommand::Close));
            }
        }
    }
}
