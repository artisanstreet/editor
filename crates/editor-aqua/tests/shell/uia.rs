use super::dialogs::started;
use aqua::Application;
use aqua::prelude::WindowEvent;
use aqua_a11y_uia::probe::{ProbeElement, ProbeKind, UiaProbe};
use aqua_protocol::AccessTreeUpdate;
use std::time::Duration;

fn probed() -> Vec<ProbeElement> {
    let (mut aqua, mut app, id, _directory) = started();
    let mut size = aqua::prelude::WindowOptions::default().inner_size;
    size.width = 800.0;
    size.height = 600.0;
    aqua.dispatch_window_event(&mut app, id, &WindowEvent::Resized(id, size));
    let observed = aqua::prelude::ObservedWindowState::new(id, "test".into(), size);
    app.prepare_accessibility(&observed, aqua.clock());
    app.build_scene(&observed, aqua.clock());
    let (root, focus, nodes) = app.a11y_nodes_for(id).expect("editor semantic tree");
    let probe = UiaProbe::spawn(AccessTreeUpdate { root, focus, nodes }).unwrap();
    probe
        .wait_for(Duration::from_secs(10), |seen| !seen.is_empty())
        .unwrap()
}

#[test]
fn every_actionable_element_in_the_editor_window_has_an_accessible_name() {
    let seen = probed();
    assert!(
        seen.iter().any(|element| element.kind == ProbeKind::Button),
        "the client sees the window's buttons"
    );
    let unnamed: Vec<_> = seen
        .iter()
        .filter(|element| element.kind == ProbeKind::Button && element.name.trim().is_empty())
        .collect();
    assert!(unnamed.is_empty(), "unnamed buttons: {unnamed:#?}");
}
