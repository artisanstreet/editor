use super::*;

fn tree(json: &str) -> FileTree {
    serde_json::from_str(json).unwrap()
}

#[test]
fn collapsing_nested_directories_preserves_scoped_file_action() {
    let tree = tree(
        r#"{"entries":[{"path":"src/a.rs","change":"added"},{"path":"src/nested/b.rs","change":"deleted"},{"path":"README.md"}]}"#,
    );
    let mut state = FileTreeState::default();
    assert!(state.reconcile(&tree));
    let initial = state.visible(&tree);
    let parent = initial
        .iter()
        .copied()
        .find(
            |index| matches!(&tree.0.rows[*index].kind, RowKind::Dir { key, .. } if key == "src/"),
        )
        .unwrap();
    let nested = initial.iter().copied().find(|index| matches!(&tree.0.rows[*index].kind, RowKind::Dir { key, .. } if key == "src/nested/")).unwrap();
    let file = initial.iter().copied().find(|index| matches!(&tree.0.rows[*index].kind, RowKind::File { entry } if tree.0.entries[*entry].path == "src/nested/b.rs")).unwrap();
    assert!(
        state
            .dispatch(&tree, "block", None, Action::Activate(nested))
            .changed
    );
    assert!(!state.visible(&tree).contains(&file));
    assert!(
        state
            .dispatch(&tree, "block", None, Action::Activate(parent))
            .changed
    );
    assert!(
        state
            .dispatch(&tree, "block", None, Action::Activate(parent))
            .changed
    );
    assert!(!state.visible(&tree).contains(&file));
    assert!(
        state
            .dispatch(&tree, "block", None, Action::Activate(nested))
            .changed
    );
    let scope = ResourceScope {
        host: "forge".into(),
        thread: artisan_api::ThreadId::new(),
    };
    assert_eq!(
        state
            .dispatch(&tree, "block", Some(scope.clone()), Action::Activate(file))
            .open,
        Some(OpenReference {
            scope: Some(scope),
            path: "src/nested/b.rs".into(),
            line: None
        })
    );
}

#[test]
fn navigation_uses_visible_rows_and_model_replacement_rejects_stale_actions() {
    let first = tree(r#"{"entries":[{"path":"src/a.rs"},{"path":"src/b.rs"},{"path":"top.rs"}]}"#);
    let second = tree(r#"{"entries":[{"path":"src/c.rs"},{"path":"other/x.rs"}]}"#);
    let mut state = FileTreeState::default();
    assert!(state.reconcile(&first));
    let visible = state.visible(&first);
    let source = visible[1];
    let result = state.dispatch(
        &first,
        "block",
        None,
        Action::Navigate(source, Navigation::Last),
    );
    assert_eq!(result.focus.as_ref().unwrap().2, visible.len() - 1);
    let result = state.dispatch(
        &first,
        "block",
        None,
        Action::Navigate(source, Navigation::Left),
    );
    assert_eq!(result.focus.as_ref().unwrap().2, 0);
    assert!(
        state
            .dispatch(
                &first,
                "block",
                None,
                Action::Navigate(visible[0], Navigation::Left)
            )
            .changed
    );
    assert!(state.reconcile(&second));
    assert_eq!(
        state.dispatch(&first, "block", None, Action::Activate(source)),
        Effect::default()
    );
    assert!(state.visible(&second).iter().any(
        |index| matches!(&second.0.rows[*index].kind, RowKind::Dir { key, .. } if key == "src/")
    ));
    assert!(!state.visible(&second).iter().any(|index| matches!(&second.0.rows[*index].kind, RowKind::File { entry } if second.0.entries[*entry].path == "src/c.rs")));
}

#[test]
fn rows_expose_tree_levels_and_only_handle_reference_keyboard_commands() {
    use aqua_core::{
        InputEvent, Key, KeyEvent, Modifiers, NamedKey, Point, PointerButton, PointerEvent,
        PointerKind, Role,
    };
    use aqua_text::{SharedFonts, SharedRuns};

    let tree = tree(r#"{"entries":[{"path":"src/a.rs"},{"path":"src/b.rs"}]}"#);
    let state = Rc::new(RefCell::new(FileTreeState::default()));
    let actions = Rc::new(RefCell::new(Vec::new()));
    let sink = actions.clone();
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| {
        tree.render(
            &ArtisanTheme::resolved(),
            "block",
            state.clone(),
            Rc::new(move |action| sink.borrow_mut().push(action)),
        )
    })
    .unwrap();
    app.set_viewport(500.0, 300.0, 1.0);
    app.build_frame().unwrap();
    let semantics = app.tree().emit_semantics();
    assert!(
        semantics
            .iter()
            .any(|node| node.role == Role::Tree && node.label == "Changed files")
    );
    let branch = semantics
        .iter()
        .find(|node| node.role == Role::TreeItem && node.label == "src/")
        .unwrap();
    assert_eq!(branch.level, Some(1));
    assert_eq!(branch.expanded, Some(true));
    assert!(
        semantics
            .iter()
            .any(|node| node.role == Role::TreeItem && node.level == Some(2))
    );
    assert!(app.focus_key("block-dir-src/"));
    for key in [Key::Named(NamedKey::Enter), Key::Char(' ')] {
        let event = KeyEvent {
            key,
            modifiers: Modifiers::default(),
            repeat: false,
        };
        app.handle_input(&InputEvent::KeyDown(event));
        app.handle_input(&InputEvent::KeyUp(event));
    }
    assert!(actions.borrow().is_empty());
    let event = KeyEvent {
        key: Key::Named(NamedKey::Left),
        modifiers: Modifiers::default(),
        repeat: false,
    };
    app.handle_input(&InputEvent::KeyDown(event));
    assert_eq!(
        actions.borrow().as_slice(),
        &[Action::Navigate(0, Navigation::Left)]
    );
    actions.borrow_mut().clear();
    let bounds = app.tree().rect(branch.node).unwrap();
    let pointer = PointerEvent {
        contact: 0,
        kind: PointerKind::Mouse,
        position: Point::new(bounds.x + 2.0, bounds.y + bounds.h / 2.0),
        buttons: 1,
    };
    app.handle_input(&InputEvent::PointerDown(pointer, PointerButton::Primary));
    app.handle_input(&InputEvent::PointerUp(pointer, PointerButton::Primary));
    assert_eq!(actions.borrow().as_slice(), &[Action::Activate(0)]);
    actions.borrow_mut().clear();
    app.tree().fire_action(aqua_ui::tree::ActionEvent {
        source: None,
        node: branch.node,
        kind: aqua_ui::tree::ActionKind::AccessibilityActivate,
        text: None,
        focused: None,
    });
    assert_eq!(actions.borrow().as_slice(), &[Action::Activate(0)]);
}
