use super::*;
use aqua_core::{InputEvent, Key, KeyEvent, Modifiers, NamedKey, Role, SemanticNode};
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::tree::{ActionEvent, ActionKind};

fn mounted(view: Element) -> aqua::UiApp {
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| view).unwrap();
    app.set_viewport(600.0, 400.0, 1.0);
    app.build_frame().unwrap();
    app
}

fn press(app: &mut aqua::UiApp, key: Key) {
    let event = KeyEvent {
        key,
        modifiers: Modifiers::default(),
        repeat: false,
    };
    app.handle_input(&InputEvent::KeyDown(event));
    app.handle_input(&InputEvent::KeyUp(event));
}

fn header(app: &aqua::UiApp) -> SemanticNode {
    app.tree()
        .emit_semantics()
        .into_iter()
        .find(|node| node.role == Role::Button && node.label == "Warning: Migration")
        .unwrap_or_else(|| panic!("no disclosure button named after the callout"))
}

fn shows_body(app: &aqua::UiApp) -> bool {
    app.tree().traverse().into_iter().any(|node| {
        match &app.tree().get(node).map(|record| &record.kind) {
            Some(aqua_ui::tree::NodeKind::Text(text)) => text.text() == "Back up first.",
            _ => false,
        }
    })
}

#[test]
fn a_collapsed_callout_opens_from_the_keyboard_and_assistive_tech() {
    let callout: Callout = serde_json::from_str(
        r#"{"kind":"warning","title":"Migration","body":"Back up first.","collapsed":true}"#,
    )
    .unwrap();
    let states = CalloutState::default();
    let render = || {
        mounted(callout.view(
            "c".to_owned(),
            &ArtisanTheme::resolved(),
            CalloutNotes {
                subtitle: None,
                caption: None,
            },
            states.clone(),
            Rc::new(|_, size, _| ui::Box::new().width(size).height(size).build()),
            Rc::new(|| {}),
        ))
    };
    let mut app = render();
    let closed = header(&app);
    assert_eq!(closed.expanded, Some(false));
    assert!(!shows_body(&app));
    assert!(app.focus_key("c-header"));
    press(&mut app, Key::Named(NamedKey::Enter));
    let mut app = render();
    assert_eq!(header(&app).expanded, Some(true));
    assert!(shows_body(&app));
    assert!(app.focus_key("c-header"));
    press(&mut app, Key::Char(' '));
    let app = render();
    assert_eq!(header(&app).expanded, Some(false));
    assert!(!shows_body(&app));
    let node = header(&app).node;
    app.tree().fire_action(ActionEvent {
        source: None,
        node,
        kind: ActionKind::Activate,
        text: None,
        focused: None,
        request: None,
    });
    let app = render();
    assert_eq!(header(&app).expanded, Some(true));
    assert!(shows_body(&app));
}
