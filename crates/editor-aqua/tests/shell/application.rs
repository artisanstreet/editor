use super::dialogs::started;
use aqua::Application;
use aqua::prelude::{InputEvent, ObservedWindowState, WindowOptions};
use aqua_core::{Key, KeyEvent, Modifiers, NamedKey};

fn observed(id: aqua::prelude::AquaWindowId) -> ObservedWindowState {
    let mut size = WindowOptions::default().inner_size;
    size.width = 800.0;
    size.height = 600.0;
    ObservedWindowState::new(id, "test".into(), size)
}

#[test]
fn the_accessibility_revision_holds_still_until_the_tree_changes() {
    let (mut aqua, mut app, id, _directory) = started();
    let window = observed(id);
    app.prepare_accessibility(&window, aqua.clock());
    app.build_scene(&window, aqua.clock());
    let settled = app
        .a11y_revision_for(id)
        .expect("the editor window reports an accessibility revision");
    app.prepare_accessibility(&window, aqua.clock());
    app.build_scene(&window, aqua.clock());
    assert_eq!(
        app.a11y_revision_for(id),
        Some(settled),
        "an unchanged frame skips the accessibility snapshot"
    );
    let tab = KeyEvent {
        key: Key::Named(NamedKey::Tab),
        modifiers: Modifiers::default(),
        repeat: false,
    };
    aqua.dispatch_input(&mut app, id, &InputEvent::KeyDown(tab));
    aqua.dispatch_input(&mut app, id, &InputEvent::KeyUp(tab));
    app.prepare_accessibility(&window, aqua.clock());
    app.build_scene(&window, aqua.clock());
    assert_ne!(
        app.a11y_revision_for(id),
        Some(settled),
        "moving focus publishes a fresh snapshot"
    );
}

#[test]
fn assistive_tech_hears_the_editor_window_in_english() {
    let (aqua, mut app, id, _directory) = started();
    let window = observed(id);
    app.prepare_accessibility(&window, aqua.clock());
    app.build_scene(&window, aqua.clock());
    let (root, _, nodes) = app.a11y_nodes_for(id).expect("editor semantic tree");
    let root = nodes
        .iter()
        .find(|node| node.semantic.node == root)
        .expect("the root is in the tree");
    assert_eq!(root.semantic.language.as_deref(), Some("en"));
}

#[test]
fn assistive_tech_reads_the_editor_window_by_name() {
    let (aqua, mut app, id, _directory) = started();
    let window = observed(id);
    app.prepare_accessibility(&window, aqua.clock());
    app.build_scene(&window, aqua.clock());
    let (root, _, nodes) = app.a11y_nodes_for(id).expect("editor semantic tree");
    let root = nodes
        .iter()
        .find(|node| node.semantic.node == root)
        .expect("the root is in the tree");
    assert_eq!(root.semantic.label, "Artisan Editor");
}
