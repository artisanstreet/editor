use aqua_core::NamedKey;
use editor_aqua::ui::rich::charts::HierarchyFixture;

const TREE: &str = r#"{"name":"root","layout":"icicle","children":[{"name":"branch","children":[{"name":"leaf","value":6}]},{"name":"other","value":4}]}"#;

#[test]
fn equal_tree_preserves_focus_and_changed_tree_resets_it() {
    let mut chart = HierarchyFixture::new(TREE);
    chart.key(NamedKey::Home, false);
    chart.key(NamedKey::Right, false);
    chart.key(NamedKey::Enter, false);
    assert_eq!(chart.focus(), 1);
    chart.replace(TREE);
    assert_eq!(chart.focus(), 1);
    chart.replace(r#"{"name":"different","value":3}"#);
    assert_eq!(chart.focus(), 0);
    assert_eq!(chart.selected(), None);
}
#[test]
fn keyboard_enters_leaf_and_parent_and_escape_return_to_root() {
    let chart = HierarchyFixture::new(TREE);
    chart.key(NamedKey::Home, false);
    chart.key(NamedKey::Right, false);
    chart.key(NamedKey::Enter, false);
    assert_eq!(chart.focus(), 1);
    chart.key(NamedKey::End, false);
    chart.key(NamedKey::Enter, false);
    assert_eq!(chart.focus(), 2);
    chart.key(NamedKey::Backspace, false);
    assert_eq!(chart.focus(), 1);
    chart.key(NamedKey::Escape, false);
    assert_eq!(chart.focus(), 0);
}
#[test]
fn flame_rows_reverse_icicle_and_leaf_pointer_click_does_not_zoom() {
    let mut chart = HierarchyFixture::new(TREE);
    let icicle = chart.parts();
    let branch = icicle.iter().find(|part| part.0 == 1).unwrap();
    let leaf = icicle.iter().find(|part| part.0 == 2).unwrap();
    assert!(branch.2 < leaf.2);
    assert!(!chart.click(leaf.1 + leaf.3 / 2.0, leaf.2 + leaf.4 / 2.0));
    chart.replace(r#"{"name":"root","layout":"flame","children":[{"name":"branch","children":[{"name":"leaf","value":6}]},{"name":"other","value":4}]}"#);
    let flame = chart.parts();
    assert!(
        flame.iter().find(|part| part.0 == 1).unwrap().2
            > flame.iter().find(|part| part.0 == 2).unwrap().2
    );
    assert_eq!(chart.resolved_levels(), 6);
}
