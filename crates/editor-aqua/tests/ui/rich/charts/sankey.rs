use aqua_core::NamedKey;
use editor_aqua::ui::rich::charts::SankeyFixture;

const GRAPH: &str =
    r#"{"links":[{"from":"a","to":"b","value":10},{"from":"b","to":"c","value":5}]}"#;

#[test]
fn equal_graph_preserves_selection_and_changed_graph_clears_it() {
    let mut chart = SankeyFixture::new(GRAPH);
    chart.key(NamedKey::End, false);
    assert_eq!(chart.selected(), Some((false, 1)));
    chart.replace(GRAPH);
    assert_eq!(chart.selected(), Some((false, 1)));
    chart.replace(r#"{"links":[{"from":"a","to":"c","value":8}]}"#);
    assert_eq!(chart.selected(), None);
}
#[test]
fn keyboard_traverses_nodes_before_links_and_clamps() {
    let chart = SankeyFixture::new(GRAPH);
    chart.key(NamedKey::Home, false);
    assert_eq!(chart.selected(), Some((true, 0)));
    chart.key(NamedKey::Right, false);
    chart.key(NamedKey::Right, false);
    chart.key(NamedKey::Right, false);
    assert_eq!(chart.selected(), Some((false, 0)));
    chart.key(NamedKey::End, false);
    chart.key(NamedKey::Right, false);
    assert_eq!(chart.selected(), Some((false, 1)));
    chart.key(NamedKey::Left, true);
    assert_eq!(chart.selected(), None);
}
#[test]
fn cubic_band_and_node_hit_testing_follow_the_real_layout() {
    let chart = SankeyFixture::new(GRAPH);
    let width = 640.0;
    for index in 0..3 {
        let (x, y) = chart.node_center(index, width);
        assert_eq!(chart.hit(width, x, y), Some((true, index)));
    }
    for index in 0..2 {
        let (x, y) = chart.link_center(index, width);
        assert_eq!(chart.hit(width, x, y), Some((false, index)));
    }
    assert_eq!(chart.hit(width, -100.0, -100.0), None);
}
