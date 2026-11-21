use aqua_core::NamedKey;
use test_support::editor::rich::MountedBody;

const TREE: &str = r#"{"name":"root","layout":"icicle","children":[{"name":"branch","children":[{"name":"leaf","value":6}]},{"name":"other","value":4}]}"#;

fn trail(chart: &MountedBody) -> Vec<usize> {
    let block = chart.block_id("hierarchy");
    (0..16)
        .filter(|node| chart.keyed(&format!("{block}-trail-{node}")).is_some())
        .collect()
}

const BASE: &str = "Hierarchy. Arrows explore nodes; Enter zooms; Backspace returns to parent; Escape returns to root.";

#[test]
fn equal_tree_preserves_focus_and_changed_tree_resets_it() {
    let mut chart = MountedBody::block("hierarchy", TREE);
    chart.press(NamedKey::Home);
    chart.press(NamedKey::Right);
    chart.press(NamedKey::Enter);
    assert_eq!(trail(&chart), [0, 1]);
    chart.replace("hierarchy", TREE);
    assert_eq!(trail(&chart), [0, 1]);
    chart.replace("hierarchy", r#"{"name":"different","value":3}"#);
    assert_eq!(trail(&chart), [0]);
    assert_eq!(chart.label(), BASE);
}

#[test]
fn keyboard_enters_leaf_and_parent_and_escape_return_to_root() {
    let mut chart = MountedBody::block("hierarchy", TREE);
    chart.press(NamedKey::Home);
    chart.press(NamedKey::Right);
    chart.press(NamedKey::Enter);
    assert_eq!(trail(&chart), [0, 1]);
    chart.press(NamedKey::End);
    chart.press(NamedKey::Enter);
    assert_eq!(trail(&chart), [0, 1, 2]);
    chart.press(NamedKey::Backspace);
    assert_eq!(trail(&chart), [0, 1]);
    chart.press(NamedKey::Escape);
    assert_eq!(trail(&chart), [0]);
}

#[test]
fn flame_rows_reverse_icicle_and_leaf_pointer_click_does_not_zoom() {
    let mut chart = MountedBody::block("hierarchy", TREE);
    let (branch, leaf) = (chart.text_rect("branch"), chart.text_rect("leaf"));
    assert!(branch.y < leaf.y);
    chart.click_at(leaf.x + 4.0, leaf.y + leaf.h / 2.0);
    assert_eq!(trail(&chart), [0]);
    chart.click_at(branch.x + 4.0, branch.y + branch.h / 2.0);
    assert_eq!(trail(&chart), [0, 1]);
    let flame = TREE.replace("icicle", "flame");
    let chart = MountedBody::block("hierarchy", &flame);
    assert!(chart.text_rect("branch").y > chart.text_rect("leaf").y);
    let chain = (0..8).rev().fold(
        serde_json::json!({"name": "l8", "value": 1}),
        |child, level| serde_json::json!({"name": format!("l{level}"), "children": [child]}),
    );
    let mut deep = chain;
    deep["layout"] = "flame".into();
    let chart = MountedBody::block("hierarchy", &deep.to_string());
    assert!(chart.shows("l6"));
    assert!(!chart.shows("l7"));
}

#[test]
fn hierarchy_labels_grow_with_the_text_size() {
    use editor_aqua::ui::preferences::{Preferences, SystemAccess};
    use editor_aqua::ui::theme::{FontSet, ThemeMode};
    let theme = |text_scale| {
        Preferences {
            theme: ThemeMode::Dark,
            fonts: FontSet::Spline,
            outline_text: true,
            reduced_motion: None,
            high_contrast: None,
            text_scale: Some(text_scale),
            streaming: editor_aqua::ui::preferences::StreamingMode::Smooth,
            lower_frame_rate: false,
            vsync: true,
            system: SystemAccess::default(),
        }
        .theme()
    };
    let height = |text_scale| {
        MountedBody::themed(
            &test_support::editor::rich::fence("hierarchy", TREE),
            &theme(text_scale),
        )
        .text_rect("branch")
        .h
    };
    let (standard, larger) = (height(1.0), height(1.75));
    assert!(larger > standard * 1.5, "{standard} grew only to {larger}");
}
