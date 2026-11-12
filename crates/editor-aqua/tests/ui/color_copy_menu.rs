use std::{cell::RefCell, rc::Rc};

use aqua::UiApp;
use aqua_core::{InputEvent, Key, KeyEvent, Modifiers, NamedKey, Point};
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::Box as UiBox;
use editor_aqua::ui::{
    color_copy_menu::{ColorCopyEffect, ColorCopyMenuHost},
    rich::actions::{CopyEntry, ShowCopyMenu},
    theme::ArtisanTheme,
};

fn key(key: NamedKey) -> InputEvent {
    InputEvent::KeyDown(KeyEvent {
        key: Key::Named(key),
        modifiers: Modifiers::NONE,
        repeat: false,
    })
}

#[test]
fn navigation_copies_the_selected_original_payload_and_dismisses() {
    let mut app = UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| {
        UiBox::new()
            .width(400.0)
            .height(300.0)
            .child(
                UiBox::new()
                    .width(80.0)
                    .height(30.0)
                    .build()
                    .with_key("owner")
                    .focusable(true),
            )
            .build()
    })
    .unwrap();
    app.set_viewport(400.0, 300.0, 1.0);
    app.build_frame().unwrap();
    assert!(app.focus_key("owner"));
    let owner = app.tree().focused().unwrap();
    let effects = Rc::new(RefCell::new(Vec::new()));
    let capture = effects.clone();
    let menu = ShowCopyMenu {
        position: Point::new(30.0, 30.0),
        entries: vec![
            CopyEntry::primary("Copy Value", "#ff0000"),
            CopyEntry::alternative("RGB", "rgb(255, 0, 0)"),
        ],
    };
    let mut host = ColorCopyMenuHost::open(
        &mut app,
        owner,
        menu,
        None,
        ArtisanTheme::resolved(),
        true,
        0.0,
        move |effect| capture.borrow_mut().push(effect),
    )
    .unwrap()
    .unwrap();
    app.build_frame().unwrap();
    assert!(host.handle_input(&mut app, &key(NamedKey::Down)));
    assert!(host.handle_input(&mut app, &key(NamedKey::Down)));
    host.handle_input(&mut app, &key(NamedKey::Enter));
    host.sync(&mut app);
    assert!(!host.is_open(&app));
    assert_eq!(app.tree().focused(), Some(owner));
    let effects = effects.borrow();
    assert!(effects.contains(&ColorCopyEffect::Clipboard("rgb(255, 0, 0)".to_owned())));
    assert_eq!(
        effects
            .iter()
            .filter(|effect| **effect == ColorCopyEffect::Dismissed)
            .count(),
        1
    );
}

#[test]
fn disposal_during_entrance_releases_motion_and_restores_focus() {
    let mut app = UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| {
        UiBox::new()
            .width(400.0)
            .height(300.0)
            .child(
                UiBox::new()
                    .width(80.0)
                    .height(30.0)
                    .build()
                    .with_key("owner")
                    .focusable(true),
            )
            .build()
    })
    .unwrap();
    app.set_viewport(400.0, 300.0, 1.0);
    app.build_frame().unwrap();
    assert!(app.focus_key("owner"));
    let owner = app.tree().focused().unwrap();
    let effects = Rc::new(RefCell::new(Vec::new()));
    let capture = effects.clone();
    let menu = ShowCopyMenu {
        position: Point::new(30.0, 30.0),
        entries: vec![CopyEntry::primary("Copy Value", "red")],
    };
    let host = ColorCopyMenuHost::open(
        &mut app,
        owner,
        menu,
        None,
        ArtisanTheme::resolved(),
        false,
        0.0,
        move |effect| capture.borrow_mut().push(effect),
    )
    .unwrap()
    .unwrap();
    assert!(host.is_open(&app));
    host.dispose(&mut app);
    assert!(app.host().motion().is_empty());
    assert_eq!(app.tree().focused(), Some(owner));
    assert_eq!(
        effects
            .borrow()
            .iter()
            .filter(|effect| **effect == ColorCopyEffect::Dismissed)
            .count(),
        1
    );
}
