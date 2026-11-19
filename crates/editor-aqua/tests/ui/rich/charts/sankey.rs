use aqua_core::NamedKey;
use test_support::editor::rich::MountedBody;

const GRAPH: &str =
    r#"{"links":[{"from":"a","to":"b","value":10},{"from":"b","to":"c","value":5}]}"#;
const BASE: &str = "Flow chart. Arrows explore nodes and flows; Home and End select endpoints; Escape clears the reading.";

#[test]
fn equal_graph_preserves_selection_and_changed_graph_clears_it() {
    let mut chart = MountedBody::block("sankey", GRAPH);
    chart.press(NamedKey::End);
    let last = format!("{BASE} b → c; Flow: 5; Of b: 100%");
    assert_eq!(chart.label(), last);
    chart.replace("sankey", GRAPH);
    assert_eq!(chart.label(), last);
    chart.replace("sankey", r#"{"links":[{"from":"a","to":"c","value":8}]}"#);
    assert_eq!(chart.label(), BASE);
}

#[test]
fn keyboard_traverses_nodes_before_links_and_clamps() {
    let mut chart = MountedBody::block("sankey", GRAPH);
    chart.press(NamedKey::Home);
    assert_eq!(chart.label(), format!("{BASE} a; Out: 10"));
    chart.press(NamedKey::Right);
    chart.press(NamedKey::Right);
    chart.press(NamedKey::Right);
    assert_eq!(chart.label(), format!("{BASE} a → b; Flow: 10; Of a: 100%"));
    chart.press(NamedKey::End);
    chart.press(NamedKey::Right);
    assert_eq!(chart.label(), format!("{BASE} b → c; Flow: 5; Of b: 100%"));
    chart.press_alt(NamedKey::Left);
    assert_eq!(chart.label(), BASE);
}
