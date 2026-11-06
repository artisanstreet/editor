use super::dialogs::started;
use aqua::prelude::WindowEvent;
use aqua::Application;
use aqua_a11y_uia::probe::{ProbeElement, ProbeKind, UiaProbe};
use aqua_protocol::AccessTreeUpdate;
use std::time::Duration;

fn probed() -> (Vec<ProbeElement>, usize) {
    let (mut aqua, mut app, id, _directory) = started();
    let mut size = aqua::prelude::WindowOptions::default().inner_size;
    size.width = 800.0;
    size.height = 600.0;
    aqua.dispatch_window_event(&mut app, id, &WindowEvent::Resized(id, size));
    let observed = aqua::prelude::ObservedWindowState::new(id, "test".into(), size);
    app.prepare_accessibility(&observed, aqua.clock());
    app.build_scene(&observed, aqua.clock());
    let (root, focus, nodes) = app.a11y_nodes_for(id).expect("editor semantic tree");
    let count = nodes.len();
    let probe = UiaProbe::spawn(AccessTreeUpdate { root, focus, nodes }).unwrap();
    let seen = probe
        .wait_for(Duration::from_secs(10), |seen| !seen.is_empty())
        .unwrap();
    (seen, count)
}

#[test]
fn the_editor_window_exposes_its_semantic_tree_to_a_ui_automation_client() {
    let (seen, count) = probed();
    assert!(count > 0);
    assert!(seen.iter().any(|element| element.kind == ProbeKind::Button));
}

#[test]
fn every_actionable_element_in_the_editor_window_has_an_accessible_name() {
    let (seen, _) = probed();
    let unnamed: Vec<_> = seen
        .iter()
        .filter(|element| element.kind == ProbeKind::Button && element.name.trim().is_empty())
        .collect();
    assert!(unnamed.is_empty(), "unnamed buttons: {unnamed:#?}");
}
