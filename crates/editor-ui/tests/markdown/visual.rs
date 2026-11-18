use super::*;
use crate::ui::markdown::MarkdownEngine;
use crate::ui::markdown::presenter::MarkdownPresenter;
use aqua_core::{Role, SemanticNode};
use aqua_text::{SharedFonts, SharedRuns};

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
