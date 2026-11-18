use aqua::UiApp;
use aqua_core::{InputEvent, Key as Pressed, KeyEvent, Modifiers, NamedKey, Role};
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::{Box as UiBox, Key, Text};
use aqua_ui::tree::hash_key;
use editor_aqua::frame::search::{self, Anchor, Overlay};
use editor_aqua::ui::theme::ArtisanTheme;
use std::rc::Rc;

fn workspace(overlay: Overlay) -> aqua_ui::element::Element {
    let theme = ArtisanTheme::resolved();
    UiBox::new()
        .width(800.0)
        .height(600.0)
        .child(
            UiBox::new()
                .width(80.0)
                .height(30.0)
                .build()
                .focusable(true)
                .with_key("workspace")
                .role(Role::Button)
                .aria_label("Workspace"),
        )
        .child(search::overlay(&theme, overlay))
        .build()
}

fn anchor() -> Anchor {
    Anchor {
        left: 16.0,
        top: 16.0,
        width: 320.0,
        bottom: 16.0,
    }
}

fn closed() -> Overlay {
    Overlay {
        progress: 0.0,
        anchor: anchor(),
        bar: None,
        results: None,
        emit: Rc::new(|_| {}),
    }
}

fn open() -> Overlay {
    Overlay {
        progress: 1.0,
        anchor: anchor(),
        bar: Some(
            Text::new("")
                .editable(true)
                .width(200.0)
                .height(30.0)
                .build()
                .with_key(search::QUERY),
        ),
        results: Some(
            UiBox::new()
                .width(200.0)
                .height(30.0)
                .build()
                .focusable(true)
                .with_key("result")
                .role(Role::Button)
                .aria_label("Result"),
        ),
        emit: Rc::new(|_| {}),
    }
}

#[test]
fn open_search_hides_the_workspace_and_keeps_tab_inside() {
    let mut app = UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| workspace(open())).unwrap();
    app.set_viewport(800.0, 600.0, 1.0);
    app.build_frame().unwrap();
    let background = app
        .tree()
        .sole_node_with_key(hash_key(Key::from("workspace")))
        .unwrap();
    assert!(app.focus_key(search::QUERY));
    let labels: Vec<String> = app
        .tree()
        .emit_semantics()
        .into_iter()
        .map(|node| node.label)
        .collect();
    assert!(labels.iter().any(|label| label == "Result"), "{labels:?}");
    assert!(
        !labels.iter().any(|label| label == "Workspace"),
        "{labels:?}"
    );
    for modifiers in [Modifiers::default(), Modifiers::SHIFT] {
        for _ in 0..3 {
            let tab = KeyEvent {
                key: Pressed::Named(NamedKey::Tab),
                modifiers,
                repeat: false,
            };
            app.handle_input(&InputEvent::KeyDown(tab));
            app.handle_input(&InputEvent::KeyUp(tab));
            assert_ne!(app.tree().focused(), Some(background));
        }
    }
}

#[test]
fn closing_search_returns_focus_to_the_workspace() {
    let mut app = UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| workspace(closed())).unwrap();
    app.set_viewport(800.0, 600.0, 1.0);
    app.build_frame().unwrap();
    assert!(app.focus_key("workspace"));
    let background = app.tree().focused();
    app.refresh(vec![workspace(open())]).unwrap();
    app.build_frame().unwrap();
    assert!(app.focus_key(search::QUERY));
    app.refresh(vec![workspace(closed())]).unwrap();
    app.build_frame().unwrap();
    assert_eq!(app.tree().focused(), background);
}
