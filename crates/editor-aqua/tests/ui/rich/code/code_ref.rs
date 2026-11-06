use std::{cell::RefCell, rc::Rc};

use aqua_core::{InputEvent, Point, PointerButton, PointerEvent, PointerKind};
use aqua_text::{SharedFonts, SharedRuns};
use artisan_api::ThreadId;

use super::*;

fn block(value: &str) -> CodeRefBlock {
    serde_json::from_str(value).unwrap()
}

#[test]
fn copy_preserves_the_original_excerpt_and_falls_back_to_reference() {
    let value = block(r#"{"ref":"src/x.rs:7-9","excerpt":"a\t\r\nb\n","note":"  note  "}"#);
    assert_eq!(value.copy(), "a\t\r\nb\n");
    assert_eq!(value.lines.as_ref(), ["a    ", "b"]);
    assert_eq!(value.note(), Some("note"));
    let empty = block(r#"{"ref":"src/x.rs:7-9","excerpt":" \n"}"#);
    assert_eq!(empty.copy(), "src/x.rs:7-9");
}

#[test]
fn clicking_path_dispatches_original_reference_with_captured_scope() {
    let value = block(r#"{"ref":"src/x.rs:7-9","excerpt":"one\ntwo"}"#);
    let directory = tempfile::tempdir().unwrap();
    let mut core = crate::core::Core::for_tests(crate::core::EditorFiles {
        dir: directory.path().join("editor"),
    })
    .unwrap();
    let identity = artisan_api::tls::Identity::generate("code-ref-test").unwrap();
    let fingerprint = artisan_api::tls::fingerprint(identity.certificate_der().unwrap().as_ref());
    let client = crate::core::ForgeClient::new("127.0.0.1:1", &fingerprint, &identity).unwrap();
    let id = crate::core::HostId("host-a".into());
    let config = crate::core::HostConfig {
        id: id.clone(),
        name: "Forge".into(),
        address: "127.0.0.1:1".into(),
        fingerprint,
        kind: crate::core::HostKind::Local,
    };
    core.add_test_host(&config, client.clone());
    let instance = core.host(&id).unwrap().instance();
    let scope = ResourceScope {
        host: "host-a".into(),
        thread: ThreadId("thread-a".into()),
    };
    let received = Rc::new(RefCell::new(Vec::new()));
    let sink = received.clone();
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| {
        value.render(
            &ArtisanTheme::resolved(),
            "block",
            Some(scope.clone()),
            Some(instance),
            Rc::new(move |action, instance| sink.borrow_mut().push((action, instance))),
        )
    })
    .unwrap();
    core.remove_host(&id).unwrap();
    core.add_test_host(&config, client);
    assert!(core.host_if_instance(&id, instance).is_none());
    app.set_viewport(500.0, 300.0, 1.0);
    app.build_frame().unwrap();
    let code = app.tree().traverse().into_iter().find(|id| {
        matches!(&app.tree().get(*id).unwrap().kind, aqua_ui::tree::NodeKind::Text(text) if text.text() == "one")
    }).unwrap();
    let code_bounds = app.tree().rect(code).unwrap();
    let code_pointer = PointerEvent {
        contact: 0,
        kind: PointerKind::Mouse,
        position: Point::new(code_bounds.x + 2.0, code_bounds.y + code_bounds.h / 2.0),
        buttons: 1,
    };
    app.handle_input(&InputEvent::PointerDown(
        code_pointer,
        PointerButton::Primary,
    ));
    app.handle_input(&InputEvent::PointerUp(code_pointer, PointerButton::Primary));
    assert!(
        matches!(&app.tree().get(code).unwrap().kind, aqua_ui::tree::NodeKind::Text(text) if text.selection.is_none())
    );
    assert!(received.borrow().is_empty());
    let key = aqua_ui::tree::hash_key(aqua_ui::element::Key::from("block-path"));
    let button = app
        .tree()
        .traverse()
        .into_iter()
        .find(|id| {
            app.tree()
                .get(*id)
                .is_some_and(|node| node.key == Some(key))
        })
        .unwrap();
    let bounds = app.tree().rect(button).unwrap();
    let pointer = PointerEvent {
        contact: 0,
        kind: PointerKind::Mouse,
        position: Point::new(bounds.x + bounds.w / 2.0, bounds.y + bounds.h / 2.0),
        buttons: 1,
    };
    app.handle_input(&InputEvent::PointerDown(pointer, PointerButton::Primary));
    app.handle_input(&InputEvent::PointerUp(pointer, PointerButton::Primary));
    assert_eq!(
        &*received.borrow(),
        &[(
            OpenReference {
                scope: Some(scope),
                path: "src/x.rs".into(),
                line: Some(7)
            },
            Some(instance)
        )]
    );
}
