use aqua_ui::collection_runtime::CollectionViewport;
use editor_aqua::shell::thread_navigation::viewport_moves_navigation;

fn viewport(start: usize, scroll_y: f32, height: f32) -> CollectionViewport {
    use aqua_ui::virtualization::RowRange;
    CollectionViewport {
        visible: RowRange {
            start,
            end: start + 6,
        },
        mounted: RowRange {
            start: start.saturating_sub(2),
            end: start + 9,
        },
        scroll_y,
        viewport_height: height,
        distance_from_start: scroll_y,
        distance_from_end: 4000.0 - scroll_y,
        follow_state: aqua_ui::collection_runtime::FollowState::Normal,
    }
}

#[test]
fn navigation_follows_the_active_marker_and_height_but_not_scroll() {
    let base = viewport(10, 120.0, 800.0);
    assert!(viewport_moves_navigation(None, base, None, None));
    assert!(!viewport_moves_navigation(
        Some(base),
        viewport(14, 133.5, 800.0),
        Some(2),
        Some(2)
    ));
    assert!(viewport_moves_navigation(
        Some(base),
        viewport(11, 120.0, 800.0),
        Some(2),
        Some(3)
    ));
    assert!(viewport_moves_navigation(
        Some(base),
        viewport(10, 120.0, 640.0),
        Some(2),
        Some(2)
    ));
}
