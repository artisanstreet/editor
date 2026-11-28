use super::*;
use crate::ui::markdown::MarkdownEngine;
use crate::ui::markdown::presenter::BodyServices;
use crate::ui::markdown::presenter::MarkdownPresenter;
use crate::ui::markdown::presenter::SelectionPlace;
use crate::ui::markdown::visual::BodyContext;
use crate::ui::markdown::visual::BodyTone;
use crate::ui::theme::ArtisanTheme;
use aqua_core::SemanticNode;
use aqua_text::document::{SelectionDocumentId, SelectionSourceId};
use aqua_text::{SharedFonts, SharedRuns};
use std::rc::Rc;

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

type Item = (Option<usize>, Option<bool>, Option<(usize, usize)>);

fn items(nodes: &[SemanticNode]) -> Vec<Item> {
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
    let glyph: crate::ui::rich::narrative::callout::GlyphFactory =
        Rc::new(|_, _, _| ui::Box::new().build());
    let invalidate: Rc<dyn Fn()> = Rc::new(|| {});
    let rendered = presenter
        .render(
            "answer",
            BodyContext {
                theme: &ArtisanTheme::resolved(),
                tone: BodyTone::Foreground,
                on_link: None,
                on_color: None,
                scope: None,
            },
            SelectionPlace {
                document: SelectionDocumentId(1),
                order: &[0],
            },
            |index| SelectionSourceId(index as u64 + 1),
            BodyServices {
                glyph: &glyph,
                invalidate: &invalidate,
                charts: None,
                code: None,
            },
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
                    BodyContext {
                        theme: &ArtisanTheme::resolved(),
                        tone: BodyTone::Foreground,
                        on_link: None,
                        on_color: None,
                        scope: None,
                    },
                    SelectionPlace {
                        document: SelectionDocumentId(1),
                        order: &[0],
                    },
                    |index| SelectionSourceId(index as u64 + 1),
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

fn text_width(element: Element, needle: &str, viewport_width: f32) -> f32 {
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(move |_| element).unwrap();
    app.set_viewport(viewport_width, 400.0, 1.0);
    app.build_frame().unwrap();
    app.tree()
        .traverse()
        .into_iter()
        .find_map(|id| {
            let node = app.tree().get(id)?;
            match &node.kind {
                aqua_ui::tree::NodeKind::Text(text) if text.text() == needle => {
                    Some(app.tree().rect(id)?.w)
                }
                aqua_ui::tree::NodeKind::Text(_)
                | aqua_ui::tree::NodeKind::Box(_)
                | aqua_ui::tree::NodeKind::Media(_)
                | aqua_ui::tree::NodeKind::Scroll(_)
                | aqua_ui::tree::NodeKind::Canvas(_)
                | aqua_ui::tree::NodeKind::Viewport3D(_) => None,
            }
        })
        .expect("list item text node")
}

#[test]
fn list_item_text_fills_available_width_instead_of_shrinking_to_content() {
    let path = r"D:\malachite\variants\09_bold_wide_bands";
    let source = format!("- `{path}`\n");
    let presenter = prepared(&source);
    let viewport_width = 600.0;
    let expected = viewport_width - ProseTypography::LIST_INDENT_PX;
    let projected = presenter
        .render_projected(
            0,
            BodyContext::plain(&ArtisanTheme::resolved(), BodyTone::Foreground),
            SelectionPlace {
                document: SelectionDocumentId(1),
                order: &[0],
            },
            |index| SelectionSourceId(index as u64 + 1),
        )
        .unwrap();
    assert!(
        (text_width(projected.element, path, viewport_width) - expected).abs() < 1.0,
        "projected list item text must fill its frame width"
    );
    let glyph: crate::ui::rich::narrative::callout::GlyphFactory =
        Rc::new(|_, _, _| ui::Box::new().build());
    let invalidate: Rc<dyn Fn()> = Rc::new(|| {});
    let full = presenter
        .render(
            "answer",
            BodyContext::plain(&ArtisanTheme::resolved(), BodyTone::Foreground),
            SelectionPlace {
                document: SelectionDocumentId(1),
                order: &[0],
            },
            |index| SelectionSourceId(index as u64 + 1),
            BodyServices {
                glyph: &glyph,
                invalidate: &invalidate,
                charts: None,
                code: None,
            },
        )
        .unwrap();
    assert!(
        (text_width(full.element, path, viewport_width) - expected).abs() < 1.0,
        "rendered list item text must fill its frame width"
    );
}
