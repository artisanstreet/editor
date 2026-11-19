use super::*;
use crate::ui::markdown::MarkdownEngine;
use crate::ui::markdown::presenter::MarkdownPresenter;
use aqua_core::{InputEvent, Key, KeyEvent, Modifiers, NamedKey, Role, SemanticNode};
use aqua_text::{SharedFonts, SharedRuns};
use std::cell::RefCell;
use std::rc::Rc;

fn answer(source: &str, on_link: Rc<dyn Fn(LinkAction)>) -> aqua::UiApp {
    let engine = MarkdownEngine::new().unwrap();
    let mut presenter = MarkdownPresenter::default();
    presenter.prepare(&engine, source).unwrap();
    presenter.prepare_document(&engine).unwrap();
    let rendered = presenter
        .render_with_context(
            "answer",
            &ArtisanTheme::resolved(),
            BodyTone::Foreground,
            SelectionDocumentId(1),
            &[0],
            |index| SelectionSourceId(index as u64 + 1),
            Rc::new(|_, _, _| ui::Box::new().build()),
            Rc::new(|| {}),
            Some(on_link),
            Some(Rc::new(|_| {})),
        )
        .unwrap();
    let fonts = SharedFonts::new();
    crate::ui::fonts::register_bundled_fonts(&fonts);
    let mut app = aqua::UiApp::new(fonts, SharedRuns::new());
    app.mount(move |_| rendered.element).unwrap();
    app.set_viewport(600.0, 400.0, 1.0);
    app.build_frame().unwrap();
    app
}

fn semantics(app: &aqua::UiApp) -> Vec<SemanticNode> {
    app.tree().emit_semantics()
}

fn press(app: &mut aqua::UiApp, key: NamedKey) {
    let event = KeyEvent {
        key: Key::Named(key),
        modifiers: Modifiers::default(),
        repeat: false,
    };
    app.handle_input(&InputEvent::KeyDown(event));
    app.handle_input(&InputEvent::KeyUp(event));
}

fn links(app: &aqua::UiApp) -> Vec<(String, bool)> {
    app.a11y_nodes()
        .unwrap()
        .2
        .into_iter()
        .filter_map(|node| node.text)
        .flat_map(|text| text.links)
        .map(|link| (link.label, link.focused))
        .collect()
}

#[test]
fn answer_links_are_announced_and_open_from_the_keyboard() {
    let opened = Rc::new(RefCell::new(Vec::new()));
    let sink = Rc::clone(&opened);
    let mut app = answer(
        "Read [the docs](https://docs.example) or [the guide](https://guide.example).",
        Rc::new(move |action| sink.borrow_mut().push(action)),
    );
    assert_eq!(
        links(&app),
        vec![
            ("the docs".to_owned(), false),
            ("the guide".to_owned(), false)
        ]
    );
    press(&mut app, NamedKey::Tab);
    assert_eq!(
        links(&app),
        vec![
            ("the docs".to_owned(), true),
            ("the guide".to_owned(), false)
        ]
    );
    press(&mut app, NamedKey::Enter);
    assert_eq!(
        opened.borrow().as_slice(),
        &[LinkAction::OpenUrl("https://docs.example".to_owned())]
    );
}

#[test]
fn answer_headings_are_exposed_with_their_level() {
    let app = answer("# Overview\n\nBody text.\n\n### Details", Rc::new(|_| {}));
    let headings: Vec<_> = semantics(&app)
        .into_iter()
        .filter_map(|node| match node.role {
            Role::Heading(level) => Some((level, node.label)),
            _ => None,
        })
        .collect();
    assert_eq!(
        headings,
        vec![(1, "Overview".to_owned()), (3, "Details".to_owned())]
    );
}

#[test]
fn colour_swatches_keep_their_value_in_the_accessible_text() {
    let app = answer("Use #ff0000 for errors.", Rc::new(|_| {}));
    assert!(
        semantics(&app)
            .iter()
            .any(|node| node.label == "Use #ff0000 for errors."),
        "{:?}",
        semantics(&app)
            .iter()
            .map(|node| node.label.clone())
            .collect::<Vec<_>>()
    );
}
