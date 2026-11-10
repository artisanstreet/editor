use editor_aqua::frame::inbox::history_height;

#[test]
fn history_uses_empty_inbox_space_and_caps_at_its_content() {
    assert_eq!(history_height(800.0, 250.0, 900.0, 1.0), 507.0);
    assert_eq!(history_height(800.0, 250.0, 200.0, 1.0), 200.0);
    assert_eq!(history_height(800.0, 250.0, 900.0, 0.5), 253.5);
    assert_eq!(history_height(800.0, 250.0, 900.0, 0.0), 0.0);
}

#[test]
fn crowded_inbox_and_history_share_the_available_height() {
    assert_eq!(history_height(800.0, 1000.0, 900.0, 1.0), 378.5);
    assert_eq!(history_height(30.0, 1000.0, 900.0, 1.0), 0.0);
    assert_eq!(history_height(800.0, 250.0, 900.0, 2.0), 507.0);
    assert_eq!(history_height(800.0, 250.0, 900.0, -1.0), 0.0);
}
