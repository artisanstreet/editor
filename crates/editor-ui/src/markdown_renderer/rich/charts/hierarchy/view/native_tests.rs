use super::super::NodeSpec;
use super::*;
use gpui::{Context, Render, TestAppContext};

struct Fixture {
    tree: Arc<Tree>,
    focus: gpui::FocusHandle,
}
impl Render for Fixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(640.0))
            .track_focus(&self.focus)
            .tab_stop(false)
            .on_action(crate::focus_navigation::focus_next)
            .on_action(crate::focus_navigation::focus_previous)
            .debug_selector(|| "native-hierarchy".into())
            .child(HierarchyView {
                id: "native-hierarchy".into(),
                theme: ArtisanTheme::default(),
                tree: self.tree.clone(),
                layout: Layout::Treemap,
                unit: None,
                levels: 3,
            })
    }
}
fn tree(source: &str) -> Arc<Tree> {
    Arc::new(Tree::from_spec(
        &serde_json::from_str::<NodeSpec>(source).unwrap(),
    ))
}
fn painted_leaf(cx: &mut gpui::VisualTestContext) -> bool {
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
        let scale = window.scale_factor();
        window.painted_quads().iter().any(|quad| {
            !quad.background.is_transparent()
                && quad.bounds.size.width.as_f32() / scale > 600.0
                && quad.bounds.size.height.as_f32() / scale > 300.0
        })
    })
}

#[gpui::test]
fn leaf_only_treemap_paints_a_real_data_rectangle(cx: &mut TestAppContext) {
    let (_, cx) = cx.add_window_view(|window, cx| Fixture {
        focus: crate::focus_navigation::fixture_root_focus(window, cx),
        tree: tree(r#"{"name":"Only leaf","value":9}"#),
    });
    assert!(
        painted_leaf(cx),
        "native leaf data rectangle must fill the treemap plot"
    );
}

#[gpui::test]
fn replacement_after_keyboard_zoom_resets_native_hierarchy_state(cx: &mut TestAppContext) {
    cx.update(crate::focus_navigation::bind_actions);
    let (view, cx) = cx.add_window_view(|window, cx| Fixture {
        focus: crate::focus_navigation::fixture_root_focus(window, cx),
        tree: tree(r#"{"name":"Root","children":[{"name":"a","value":2},{"name":"b","value":3}]}"#),
    });
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    cx.simulate_keystrokes("tab end enter");
    assert!(
        cx.update(|window, cx| window.focused(cx)).is_some(),
        "chart owns native focus"
    );
    view.update(cx, |view, cx| {
        view.tree = tree(r#"{"name":"Replacement leaf","value":1}"#);
        cx.notify();
    });
    cx.run_until_parked();
    assert!(
        painted_leaf(cx),
        "replacement must reset stale node focus before native rendering"
    );
}
