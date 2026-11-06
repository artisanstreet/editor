use editor_aqua::shell::menu_keys::{Direction, next_index};

#[test]
fn forward_wraps_from_the_last_item_to_the_first() {
    assert_eq!(next_index(Some(2), 3, Direction::Forward), Some(0));
    assert_eq!(next_index(Some(0), 3, Direction::Forward), Some(1));
}

#[test]
fn backward_wraps_from_the_first_item_to_the_last() {
    assert_eq!(next_index(Some(0), 3, Direction::Backward), Some(2));
    assert_eq!(next_index(Some(2), 3, Direction::Backward), Some(1));
}

#[test]
fn focus_outside_the_items_enters_at_the_matching_end() {
    assert_eq!(next_index(None, 4, Direction::Forward), Some(0));
    assert_eq!(next_index(None, 4, Direction::Backward), Some(3));
}

#[test]
fn an_empty_menu_has_nowhere_to_go() {
    assert_eq!(next_index(None, 0, Direction::Forward), None);
    assert_eq!(next_index(Some(0), 0, Direction::Backward), None);
}
