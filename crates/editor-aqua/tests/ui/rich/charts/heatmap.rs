use aqua_core::NamedKey;
use editor_aqua::ui::rich::charts::HeatmapFixture;

const MODEL: &str = r#"{"x":["a","b","c"],"y":["first","second"],"values":[[1,2,3],[4,null,6]]}"#;

#[test]
fn changed_models_reset_selection_and_scroll_while_equal_models_preserve_them() {
    let mut map = HeatmapFixture::new(MODEL);
    assert!(map.key(NamedKey::End, false));
    let reveal = map.pending_reveal().unwrap();
    map.replace(MODEL);
    assert_eq!(map.selected(), Some((1, 2)));
    assert_eq!(map.pending_reveal(), Some(reveal));
    assert_eq!(map.generation(), 0);
    map.replace(r#"{"values":[[7,8],[9,10]]}"#);
    assert_eq!(map.selected(), None);
    assert_eq!(map.pending_reveal(), None);
    assert_eq!(map.generation(), 1);
}

#[test]
fn keyboard_selection_uses_bounded_linear_order_and_clear() {
    let map = HeatmapFixture::new(MODEL);
    map.key(NamedKey::Left, false);
    assert_eq!(map.selected(), Some((0, 0)));
    map.key(NamedKey::End, false);
    map.key(NamedKey::Right, false);
    assert_eq!(map.selected(), Some((1, 2)));
    map.key(NamedKey::Up, false);
    assert_eq!(map.selected(), Some((1, 1)));
    map.key(NamedKey::Left, true);
    assert_eq!(map.selected(), None);
    assert_eq!(map.pending_reveal(), None);
}

#[test]
fn hover_maps_cell_boundaries_and_clears_outside_plot() {
    let map = HeatmapFixture::new(MODEL);
    assert!(map.hover(1, 0.0, 0.5));
    assert_eq!(map.selected(), Some((1, 0)));
    assert!(!map.hover(1, 0.1, 0.5));
    assert!(map.hover(1, 0.99, 0.5));
    assert_eq!(map.selected(), Some((1, 2)));
    assert!(map.hover(1, 1.0, 0.5));
    assert_eq!(map.selected(), None);
    assert!(!map.hover(1, 0.5, -0.01));
}

#[test]
fn reveal_retries_until_matching_collection_and_index_are_acknowledged() {
    let map = HeatmapFixture::new(MODEL);
    map.key(NamedKey::End, false);
    let (key, row) = map.pending_reveal().unwrap();
    map.revealed("wrong-generation", row);
    map.revealed(&key, row - 1);
    assert_eq!(map.pending_reveal(), Some((key.clone(), row)));
    map.revealed(&key, row);
    assert_eq!(map.pending_reveal(), None);
}
