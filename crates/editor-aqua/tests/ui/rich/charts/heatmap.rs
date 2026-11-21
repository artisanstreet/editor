use aqua_core::{NamedKey, Rect};
use test_support::editor::rich::{ITEM, MountedBody};

const MODEL: &str = r#"{"x":["a","b","c"],"y":["first","second"],"values":[[1,2,3],[4,null,6]]}"#;
const BASE: &str =
    "Heatmap. Arrows explore cells; Home and End select endpoints; Escape clears the reading.";

fn plot(map: &MountedBody, row: usize) -> Rect {
    let row = map.rect(&format!("{}-row-{row}", map.block_id("heatmap")));
    Rect {
        x: row.x + 96.0,
        y: row.y,
        w: row.w - 96.0,
        h: row.h,
    }
}

#[test]
fn changed_models_reset_selection_and_scroll_while_equal_models_preserve_them() {
    let mut map = MountedBody::block("heatmap", MODEL);
    map.press(NamedKey::End);
    let reveal = map.presenter().heatmap_reveals(ITEM);
    assert_eq!(reveal.len(), 1);
    map.replace("heatmap", MODEL);
    assert_eq!(map.label(), format!("{BASE} second · c: 6"));
    assert_eq!(map.presenter().heatmap_reveals(ITEM), reveal);
    map.replace("heatmap", r#"{"values":[[7,8],[9,10]]}"#);
    assert_eq!(map.label(), BASE);
    assert!(map.presenter().heatmap_reveals(ITEM).is_empty());
    map.press(NamedKey::End);
    let fresh = map.presenter().heatmap_reveals(ITEM);
    assert_eq!(fresh.len(), 1);
    assert_ne!(fresh[0].1, reveal[0].1);
}

#[test]
fn keyboard_selection_uses_bounded_linear_order_and_clear() {
    let mut map = MountedBody::block("heatmap", MODEL);
    map.press(NamedKey::Left);
    assert_eq!(map.label(), format!("{BASE} first · a: 1"));
    map.press(NamedKey::End);
    map.press(NamedKey::Right);
    assert_eq!(map.label(), format!("{BASE} second · c: 6"));
    map.press(NamedKey::Up);
    assert_eq!(map.label(), format!("{BASE} second · b: –"));
    map.press_alt(NamedKey::Left);
    assert_eq!(map.label(), BASE);
    assert!(map.presenter().heatmap_reveals(ITEM).is_empty());
}

#[test]
fn hover_maps_cell_boundaries_beneath_the_reading_and_clears_outside_plot() {
    let mut map = MountedBody::block("heatmap", MODEL);
    let second = plot(&map, 1);
    map.move_within(second, 0.0, 0.5);
    assert_eq!(map.label(), format!("{BASE} second · a: 4"));
    map.move_within(second, 0.1, 0.5);
    assert_eq!(map.label(), format!("{BASE} second · a: 4"));
    map.move_within(second, 0.99, 0.5);
    assert_eq!(map.label(), format!("{BASE} second · c: 6"));
    map.move_to(second.x - 40.0, second.y + second.h / 2.0);
    assert_eq!(map.label(), BASE);
    map.move_within(plot(&map, 0), 0.5, 0.5);
    assert_eq!(map.label(), format!("{BASE} first · b: 2"));
    map.move_within(second, 0.5, 0.5);
    assert_eq!(map.label(), format!("{BASE} second · b: –"));
}

#[test]
fn reveal_retries_until_matching_collection_and_index_are_acknowledged() {
    let mut map = MountedBody::block("heatmap", MODEL);
    map.press(NamedKey::End);
    let [(block, key, row)] = <[_; 1]>::try_from(map.presenter().heatmap_reveals(ITEM))
        .expect("one heatmap asks to reveal its selected row");
    map.presenter()
        .heatmap_revealed(&block, "wrong-generation", row);
    map.presenter().heatmap_revealed(&block, &key, row - 1);
    assert_eq!(
        map.presenter().heatmap_reveals(ITEM),
        [(block.clone(), key.clone(), row)]
    );
    map.presenter().heatmap_revealed(&block, &key, row);
    assert!(map.presenter().heatmap_reveals(ITEM).is_empty());
}
