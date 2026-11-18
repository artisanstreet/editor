use super::*;
use crate::ui::markdown::MarkdownEngine;
use crate::ui::markdown::presenter::MarkdownPresenter;
use aqua_core::SemanticNode;
use aqua_text::{SharedFonts, SharedRuns};

fn semantics(element: Element) -> Vec<SemanticNode> {
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(move |_| element).unwrap();
    app.set_viewport(600.0, 400.0, 1.0);
    app.build_frame().unwrap();
    app.tree().emit_semantics()
}

fn prepared(source: &str) -> MarkdownPresenter {
    let engine = MarkdownEngine::new().unwrap();
    let mut presenter = MarkdownPresenter::default();
    presenter.prepare(&engine, source).unwrap();
    presenter.prepare_document(&engine).unwrap();
    presenter
}

const TASKS: &str = "- [x] Ship it\n- [ ] Tell everyone\n  1. Nested step\n";

fn items(nodes: &[SemanticNode]) -> Vec<(Option<usize>, Option<bool>, Option<(usize, usize)>)> {
    nodes
        .iter()
        .filter(|node| node.role == Role::ListItem)
        .map(|node| {
            (
                node.level,
                node.checked,
                node.collection
                    .and_then(|collection| collection.position_in_set.zip(collection.set_size)),
            )
        })
        .collect()
}

#[test]
fn rendered_lists_expose_items_nesting_and_task_state() {
    let presenter = prepared(TASKS);
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
            None,
            None,
        )
        .unwrap();
    let nodes = semantics(rendered.element);
    assert_eq!(
        nodes.iter().filter(|node| node.role == Role::List).count(),
        1
    );
    assert_eq!(
        items(&nodes),
        vec![
            (Some(1), Some(true), Some((1, 2))),
            (Some(1), Some(false), Some((2, 2))),
            (Some(2), None, Some((1, 1))),
        ]
    );
    assert!(
        !nodes
            .iter()
            .any(|node| node.label == "☑" || node.label == "☐")
    );
}

#[test]
fn streamed_list_rows_expose_items_and_task_state() {
    let presenter = prepared(TASKS);
    let rows = presenter.projected_count().unwrap();
    let nodes: Vec<_> = (0..rows)
        .flat_map(|row| {
            let rendered = presenter
                .render_projected(
                    row,
                    &ArtisanTheme::resolved(),
                    BodyTone::Foreground,
                    SelectionDocumentId(1),
                    &[0],
                    |index| SelectionSourceId(index as u64 + 1),
                    None,
                    None,
                    None,
                )
                .unwrap();
            semantics(rendered.element)
        })
        .collect();
    assert_eq!(
        items(&nodes)
            .into_iter()
            .map(|(level, checked, _)| (level, checked))
            .collect::<Vec<_>>(),
        vec![
            (Some(1), Some(true)),
            (Some(1), Some(false)),
            (Some(2), None)
        ]
    );
    assert!(
        !nodes
            .iter()
            .any(|node| node.label == "☑" || node.label == "☐")
    );
}
