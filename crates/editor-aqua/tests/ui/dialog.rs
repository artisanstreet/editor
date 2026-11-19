use std::{cell::Cell, rc::Rc};

use aqua::UiApp;
use aqua_core::InputEvent;
use aqua_text::{FontFamily, SharedFonts, SharedRuns};
use aqua_ui::element::{Box as UiBox, Key, Text};
use aqua_ui::tree::{NodeKind, hash_key};
use editor_aqua::ui::{
    dialog::{Dialog, DialogHost},
    theme::{ArtisanTheme, FontSet, TypographyTokens},
};

fn content() -> aqua_ui::element::Element {
    Text::new("stream")
        .editable(true)
        .width(200.0)
        .height(30.0)
        .build()
        .with_key("terminal-content")
}

#[test]
fn refresh_preserves_focus_and_selection_then_disposes_before_owner() {
    let mut app = UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| {
        UiBox::new()
            .width(600.0)
            .height(400.0)
            .build()
            .focusable(true)
    })
    .unwrap();
    app.set_viewport(600.0, 400.0, 1.0);
    app.build_frame().unwrap();
    let owner = app.tree().root().unwrap();
    assert!(app.tree_mut().set_focused(Some(owner)));
    let dismissed = Rc::new(Cell::new(0));
    let callback = Rc::clone(&dismissed);
    let dialog = Dialog::new(
        "terminal-dialog",
        ArtisanTheme::resolved(),
        "Terminal output",
        content(),
    )
    .bare()
    .max_width(504.0)
    .on_dismiss(move |_| callback.set(callback.get() + 1));
    let host = DialogHost::open(&mut app, owner, dialog).unwrap();
    let key = hash_key(Key::from("terminal-content"));
    let node = app
        .tree()
        .traverse()
        .into_iter()
        .find(|node| {
            app.tree()
                .get(*node)
                .is_some_and(|record| record.key == Some(key))
        })
        .unwrap();
    assert!(app.tree_mut().set_focused(Some(node)));
    app.handle_input(&InputEvent::TextInput { text: "!".into() });
    let before = app.tree().editor_snapshot(node).unwrap();
    host.refresh_content(&mut app, content()).unwrap();
    assert!(host.is_open(&app));
    assert_eq!(app.tree().focused(), Some(node));
    let after = app.tree().editor_snapshot(node).unwrap();
    assert_eq!(before.text, after.text);
    assert_eq!(before.selection, after.selection);
    host.dispose(&mut app);
    assert!(app.host().service().is_empty());
    assert_eq!(app.tree().focused(), Some(owner));
    assert_eq!(dismissed.get(), 0);
    assert!(app.tree_mut().remove(owner));
}

fn family_of(app: &UiApp, key: &str) -> Vec<FontFamily> {
    let key = hash_key(Key::from(key));
    app.tree()
        .traverse()
        .into_iter()
        .find_map(|node| {
            let record = app.tree().get(node)?;
            match &record.kind {
                NodeKind::Text(text) if record.key == Some(key) => {
                    Some(text.effective_text_style().families)
                }
                _ => None,
            }
        })
        .unwrap()
}

#[test]
fn unstyled_content_follows_the_font_set_across_a_runtime_switch() {
    let mut app = UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| UiBox::new().width(600.0).height(400.0).build())
        .unwrap();
    app.set_viewport(600.0, 400.0, 1.0);
    app.build_frame().unwrap();
    let owner = app.tree().root().unwrap();
    let label = || Text::new("label").build().with_key("unstyled-label");
    let themed = |fonts| {
        let mut theme = ArtisanTheme::resolved();
        theme.typography = TypographyTokens::new(fonts);
        theme
    };
    let spline = themed(FontSet::Spline);
    let system = themed(FontSet::System);
    let dialog = Dialog::new("font-dialog", spline, "Fonts", label()).bare();
    let host = DialogHost::open(&mut app, owner, dialog).unwrap();
    assert_eq!(
        family_of(&app, "unstyled-label"),
        vec![FontFamily::Named(spline.typography.sans.family.to_owned())]
    );
    host.set_theme(system);
    host.refresh_content(&mut app, label()).unwrap();
    assert_eq!(
        family_of(&app, "unstyled-label"),
        vec![FontFamily::Named(system.typography.sans.family.to_owned())]
    );
    host.dispose(&mut app);
}

#[test]
fn an_open_dialog_hides_the_background_from_assistive_tech_and_keeps_tab_inside() {
    use aqua_core::{Key as Pressed, KeyEvent, Modifiers, NamedKey, Role};
    let mut app = UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| {
        UiBox::new()
            .width(600.0)
            .height(400.0)
            .child(
                UiBox::new()
                    .width(80.0)
                    .height(30.0)
                    .build()
                    .focusable(true)
                    .with_key("background")
                    .role(Role::Button)
                    .aria_label("Background"),
            )
            .build()
    })
    .unwrap();
    app.set_viewport(600.0, 400.0, 1.0);
    app.build_frame().unwrap();
    let owner = app.tree().root().unwrap();
    let background = app
        .tree()
        .sole_node_with_key(hash_key(Key::from("background")))
        .unwrap();
    assert!(app.tree_mut().set_focused(Some(background)));
    let inside = UiBox::new()
        .width(80.0)
        .height(30.0)
        .build()
        .focusable(true)
        .with_key("inside")
        .role(Role::Button)
        .aria_label("Inside");
    let host = DialogHost::open(
        &mut app,
        owner,
        Dialog::new("modal", ArtisanTheme::resolved(), "Confirm", inside),
    )
    .unwrap();
    app.build_frame().unwrap();
    let labels: Vec<String> = app
        .tree()
        .emit_semantics()
        .into_iter()
        .map(|node| node.label)
        .collect();
    assert!(labels.iter().any(|label| label == "Inside"), "{labels:?}");
    assert!(
        !labels.iter().any(|label| label == "Background"),
        "{labels:?}"
    );
    let tab = KeyEvent {
        key: Pressed::Named(NamedKey::Tab),
        modifiers: Modifiers::default(),
        repeat: false,
    };
    for _ in 0..4 {
        app.handle_input(&InputEvent::KeyDown(tab));
        app.handle_input(&InputEvent::KeyUp(tab));
        assert_ne!(app.tree().focused(), Some(background));
    }
    host.dispose(&mut app);
}

#[test]
fn an_open_dialog_reads_as_one_dialog_named_by_its_title() {
    use aqua_core::Role;
    let mut app = UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(|_| UiBox::new().width(600.0).height(400.0).build())
        .unwrap();
    app.set_viewport(600.0, 400.0, 1.0);
    app.build_frame().unwrap();
    let owner = app.tree().root().unwrap();
    let host = DialogHost::open(
        &mut app,
        owner,
        Dialog::new("modal", ArtisanTheme::resolved(), "Settings", content()),
    )
    .unwrap();
    app.build_frame().unwrap();
    let dialogs: Vec<String> = app
        .tree()
        .emit_semantics()
        .into_iter()
        .filter(|node| node.role == Role::Dialog)
        .map(|node| node.label)
        .collect();
    assert_eq!(dialogs, vec!["Settings".to_owned()]);
    host.dispose(&mut app);
}
