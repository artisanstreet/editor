use super::*;
use crate::ui::markdown::MarkdownEngine;
use crate::ui::markdown::presenter::BodyServices;
use crate::ui::markdown::presenter::MarkdownPresenter;
use crate::ui::markdown::presenter::SelectionPlace;
use crate::ui::markdown::visual::BodyContext;
use crate::ui::markdown::visual::BodyTone;
use crate::ui::theme::ArtisanTheme;
use aqua_core::{NodeId, SemanticNode};
use aqua_text::document::{SelectionDocumentId, SelectionSourceId};
use aqua_text::{SharedFonts, SharedRuns};
use std::rc::Rc;

const TABLE: &str = "| Name | Size |\n| --- | ---: |\n| alpha | 1 |\n| beta | 2 |\n";

fn semantics(element: Element) -> Vec<SemanticNode> {
    let mut app = aqua::UiApp::new(SharedFonts::new(), SharedRuns::new());
    app.mount(move |_| element).unwrap();
    app.set_viewport(600.0, 400.0, 1.0);
    app.build_frame().unwrap();
    app.tree().emit_semantics()
}

fn prepared() -> MarkdownPresenter {
    let engine = MarkdownEngine::new().unwrap();
    let mut presenter = MarkdownPresenter::default();
    presenter.prepare(&engine, TABLE).unwrap();
    presenter.prepare_document(&engine).unwrap();
    presenter
}

fn text_of(nodes: &[SemanticNode], id: NodeId) -> String {
    let Some(node) = nodes.iter().find(|node| node.node == id) else {
        return String::new();
    };
    match node.role {
        Role::Text => node.label.clone(),
        Role::PasswordField
        | Role::ComboBox
        | Role::Option
        | Role::Separator
        | Role::Tooltip
        | Role::Group
        | Role::Button
        | Role::Checkbox
        | Role::Switch
        | Role::RadioButton
        | Role::TextField
        | Role::Heading(_)
        | Role::List
        | Role::ListBox
        | Role::RadioGroup
        | Role::ListItem
        | Role::Tree
        | Role::TreeItem
        | Role::Table
        | Role::Row
        | Role::ColumnHeader
        | Role::Cell
        | Role::Slider
        | Role::ProgressIndicator
        | Role::TabList
        | Role::Tab
        | Role::Image
        | Role::ScrollRegion
        | Role::Dialog
        | Role::Menu
        | Role::MenuItem
        | Role::Link => node
            .children
            .iter()
            .map(|child| text_of(nodes, *child))
            .collect(),
    }
}

fn rows(nodes: &[SemanticNode]) -> Vec<Vec<(Role, String)>> {
    nodes
        .iter()
        .filter(|node| node.role == Role::Row)
        .map(|row| {
            row.children
                .iter()
                .filter_map(|child| nodes.iter().find(|node| node.node == *child))
                .map(|cell| (cell.role, text_of(nodes, cell.node)))
                .collect()
        })
        .collect()
}

fn expected() -> Vec<Vec<(Role, String)>> {
    vec![
        vec![
            (Role::ColumnHeader, "Name".to_owned()),
            (Role::ColumnHeader, "Size".to_owned()),
        ],
        vec![
            (Role::Cell, "alpha".to_owned()),
            (Role::Cell, "1".to_owned()),
        ],
        vec![
            (Role::Cell, "beta".to_owned()),
            (Role::Cell, "2".to_owned()),
        ],
    ]
}

#[test]
fn rendered_tables_keep_rows_and_column_headers() {
    let glyph: crate::ui::rich::narrative::callout::GlyphFactory =
        Rc::new(|_, _, _| ui::Box::new().build());
    let invalidate: Rc<dyn Fn()> = Rc::new(|| {});
    let rendered = prepared()
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
    let table = nodes.iter().find(|node| node.role == Role::Table).unwrap();
    assert_eq!(
        table
            .collection
            .and_then(|collection| collection.row_count.zip(collection.column_count)),
        Some((3, 2))
    );
    assert_eq!(table.children.len(), 3);
    assert_eq!(rows(&nodes), expected());
}

#[test]
fn streamed_table_rows_keep_rows_and_column_headers() {
    let presenter = prepared();
    let rows: Vec<_> = (0..presenter.projected_count().unwrap())
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
            rows(&semantics(rendered.element))
        })
        .collect();
    assert_eq!(rows, expected());
}
