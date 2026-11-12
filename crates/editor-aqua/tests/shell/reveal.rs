use editor_aqua::shell::root::reveal::{estimated_row, find_ignoring_case};

#[test]
fn the_match_is_found_in_rendered_text_whatever_its_case() {
    assert_eq!(
        find_ignoring_case("Raising the Scene Bound fixed it.", "scene bound"),
        Some(12..23)
    );
    assert_eq!(find_ignoring_case("Brød og smør", "SMØR"), None);
    assert_eq!(find_ignoring_case("Brød og smør", "smør"), Some(9..14));
    assert_eq!(find_ignoring_case("nothing here", "scene"), None);
}

#[test]
fn a_hit_deep_in_a_long_message_aims_at_the_row_at_the_same_depth() {
    let rows = [10, 11, 12, 13, 14];
    assert_eq!(estimated_row(&rows, (0, 1000)), Some(10));
    assert_eq!(estimated_row(&rows, (500, 1000)), Some(12));
    assert_eq!(estimated_row(&rows, (990, 1000)), Some(14));
    assert_eq!(estimated_row(&rows, (5, 0)), Some(14));
    assert_eq!(estimated_row(&[], (5, 10)), None);
}
