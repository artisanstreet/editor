//! External coverage for GFM tables in the Markdown engine and renderer.
//!
//! Engine tests pin the owned table model: column alignments, inline
//! structure inside cells, grammar-normalized row widths, streaming
//! prefixes, and tables nested in list items. Mounted tests pin the grid:
//! columns share one leading edge, rows stack in source order, the table
//! hugs its content, and a table wider than the message shrinks to fit.
//! Exact text widths stay out of scope: test-environment font metrics
//! differ from production.

use artisan_ui::markdown::{Block, MarkdownEngine, Span, Table, TableAlignment, spans_text};
use artisan_ui::markdown_renderer::{MarkdownBodyTone, MarkdownRenderer};
use artisan_ui::theme::{ArtisanTheme, ThemeMode};
use gpui::{
    Bounds, Context, InteractiveElement, IntoElement, ParentElement, Pixels, Render, Styled,
    TestAppContext, VisualTestContext, Window, div, px,
};

const RUNS_TABLE: &str = "\
| Run | Time | Attempts |
|---|---|---|
| Baseline (Sep 24) | 26.5 min | 69 |
| #4 inspector in record | **5.5 min** | **1** |
";

fn engine() -> MarkdownEngine {
    MarkdownEngine::new().expect("markdown engine construction must succeed")
}

/// Parses `source` and returns its only block as a table.
fn only_table(source: &str) -> Table {
    let parsed = engine().parse_document(source).expect("parse succeeds");
    match parsed.blocks() {
        [Block::Table(table)] => table.clone(),
        blocks => panic!("expected exactly one table, got {blocks:?}"),
    }
}

fn row_text(table: &Table, row: usize) -> Vec<String> {
    table.rows[row]
        .iter()
        .map(|cell| spans_text(&cell.spans))
        .collect()
}

#[test]
fn table_parses_into_header_and_rows() {
    let table = only_table(RUNS_TABLE);

    assert_eq!(table.alignments, vec![TableAlignment::Unset; 3]);
    let header = table
        .header
        .iter()
        .map(|cell| spans_text(&cell.spans))
        .collect::<Vec<_>>();
    assert_eq!(header, ["Run", "Time", "Attempts"]);
    assert_eq!(table.rows.len(), 2);
    assert_eq!(row_text(&table, 0), ["Baseline (Sep 24)", "26.5 min", "69"]);
    assert_eq!(
        row_text(&table, 1),
        ["#4 inspector in record", "5.5 min", "1"]
    );
    assert_eq!(table.range, 0..RUNS_TABLE.len());
}

#[test]
fn cells_keep_inline_structure() {
    let table = only_table(
        "| a | b |\n|---|---|\n| **bold** `code` | [link](https://example.com) _em_ |\n",
    );

    assert_eq!(
        table.rows[0][0].spans,
        vec![
            Span::Strong(vec![Span::Text("bold".to_owned())]),
            Span::Text(" ".to_owned()),
            Span::Code("code".to_owned()),
        ]
    );
    assert_eq!(
        table.rows[0][1].spans,
        vec![
            Span::Link {
                label: vec![Span::Text("link".to_owned())],
                destination: "https://example.com".to_owned(),
            },
            Span::Text(" ".to_owned()),
            Span::Emphasis(vec![Span::Text("em".to_owned())]),
        ]
    );
}

#[test]
fn delimiter_row_sets_column_alignment() {
    let table = only_table("| a | b | c | d |\n|---|:---|:---:|---:|\n| 1 | 2 | 3 | 4 |\n");

    assert_eq!(
        table.alignments,
        vec![
            TableAlignment::Unset,
            TableAlignment::Left,
            TableAlignment::Center,
            TableAlignment::Right,
        ]
    );
}

#[test]
fn short_and_long_rows_settle_to_the_column_count() {
    let table = only_table("| a | b |\n|---|---|\n| only |\n| one | two | three |\n");

    assert_eq!(table.alignments.len(), 2);
    assert_eq!(row_text(&table, 0), ["only", ""]);
    assert_eq!(row_text(&table, 1), ["one", "two"]);
}

#[test]
fn escaped_pipe_stays_inside_its_cell() {
    let table = only_table("| a | b |\n|---|---|\n| x \\| y | z |\n");

    assert_eq!(row_text(&table, 0), ["x | y", "z"]);
}

#[test]
fn header_without_delimiter_row_stays_a_paragraph() {
    // A streaming prefix that has not reached the delimiter row yet.
    let parsed = engine()
        .parse_document("| Run | Time |\n")
        .expect("parse succeeds");

    assert!(matches!(parsed.blocks(), [Block::Paragraph { .. }]));
    assert_eq!(parsed.blocks()[0].text_content(), "| Run | Time |");
}

#[test]
fn delimiter_row_alone_opens_an_empty_table() {
    let table = only_table("| Run | Time |\n|---|---|\n");

    assert_eq!(table.header.len(), 2);
    assert!(table.rows.is_empty());
}

#[test]
fn blocks_around_a_table_keep_their_text() {
    let parsed = engine()
        .parse_document("## Runs\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\nAfter **bold**.\n")
        .expect("parse succeeds");

    let blocks = parsed.blocks();
    assert!(matches!(blocks[0], Block::Heading { level: 2, .. }));
    assert!(matches!(blocks[1], Block::Table(_)));
    assert_eq!(blocks[1].text_content(), "a\tb\n1\t2");
    assert_eq!(blocks[2].text_content(), "After bold.");
    assert_eq!(blocks.len(), 3);
}

#[test]
fn table_inside_a_list_item_follows_the_item_text() {
    let parsed = engine()
        .parse_document("- results\n\n  | a | b |\n  |---|---|\n  | 1 | 2 |\n")
        .expect("parse succeeds");

    let [Block::List { items, .. }] = parsed.blocks() else {
        panic!("expected one list, got {:?}", parsed.blocks());
    };
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].blocks.len(), 2);
    assert_eq!(items[0].blocks[0].text_content(), "results");
    assert!(matches!(items[0].blocks[1], Block::Table(_)));
}

const HOST_SELECTOR: &str = "table-host";
const HOST_WIDTH_PX: f32 = 600.0;

struct TableProbe {
    renderer: MarkdownRenderer,
    source: &'static str,
}

impl Render for TableProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(HOST_WIDTH_PX))
            .debug_selector(|| HOST_SELECTOR.to_string())
            .child(self.renderer.render_source_with_tone(
                self.source,
                ArtisanTheme::for_mode(ThemeMode::Dark),
                "probe",
                MarkdownBodyTone::Foreground,
            ))
    }
}

fn mount<'a>(cx: &'a mut TestAppContext, source: &'static str) -> &'a mut VisualTestContext {
    let (_, cx) = cx.add_window_view(|_, _| TableProbe {
        renderer: MarkdownRenderer::new(),
        source,
    });
    cx
}

fn cell(cx: &mut VisualTestContext, row: usize, column: usize) -> Bounds<Pixels> {
    let selector = format!("probe-markdown-block-0-table-r{row}-c{column}");
    cx.debug_bounds(Box::leak(selector.into_boxed_str()))
        .expect("table cell must paint inspectable bounds")
}

fn cell_content(cx: &mut VisualTestContext, row: usize, column: usize) -> Bounds<Pixels> {
    let selector = format!("probe-markdown-block-0-table-r{row}-c{column}-content");
    cx.debug_bounds(Box::leak(selector.into_boxed_str()))
        .expect("table cell content must paint inspectable bounds")
}

#[gpui::test]
fn mounted_cells_form_aligned_rows_and_columns(cx: &mut TestAppContext) {
    let cx = mount(cx, RUNS_TABLE);

    for column in 0..3 {
        let header = cell(cx, 0, column);
        assert!(
            cell_content(cx, 0, column).size.width > px(16.0),
            "column {column} must size to its text instead of collapsing"
        );
        assert_eq!(
            header.size.height,
            px(33.0),
            "a header cell is one 24 px line, 8 px bottom padding, and the hairline"
        );
        for row in 1..3 {
            let body = cell(cx, row, column);
            assert_eq!(
                body.left(),
                header.left(),
                "column {column} must share one leading edge"
            );
            assert_eq!(
                body.size.width, header.size.width,
                "column {column} must share one track width"
            );
        }
    }
    for row in 0..3 {
        let first = cell(cx, row, 0);
        for column in 1..3 {
            let next = cell(cx, row, column);
            assert_eq!(next.top(), first.top(), "row {row} must share one top");
            assert_eq!(
                next.size.height, first.size.height,
                "row {row} must share one height"
            );
            assert!(
                next.left() >= cell(cx, row, column - 1).right(),
                "row {row} columns must not overlap"
            );
        }
    }
    for row in 1..3 {
        assert_eq!(
            cell(cx, row, 0).top(),
            cell(cx, row - 1, 0).bottom(),
            "rows must stack in source order"
        );
    }
}

#[gpui::test]
fn mounted_table_hugs_its_content(cx: &mut TestAppContext) {
    let cx = mount(cx, "| a | b |\n|---|---|\n| 1 | 2 |\n");

    let table = cx
        .debug_bounds("probe-markdown-block-0-table")
        .expect("table must paint inspectable bounds");
    assert!(table.size.width > px(0.0));
    assert!(
        table.size.width < px(HOST_WIDTH_PX / 2.0),
        "a two-letter table must not stretch across the message: {table:?}"
    );
}

#[gpui::test]
fn mounted_wide_table_shrinks_to_the_message_width(cx: &mut TestAppContext) {
    let cx = mount(
        cx,
        "| finding | rationale |\n|---|---|\n\
         | The score counts three severe gaps and lands in the lower band for real reasons | \
         The same record that scored high with a false finding now scores low because every \
         severe gap is cited against the library text it violates |\n",
    );

    let host = cx
        .debug_bounds(HOST_SELECTOR)
        .expect("host must paint inspectable bounds");
    let table = cx
        .debug_bounds("probe-markdown-block-0-table")
        .expect("table must paint inspectable bounds");
    assert!(
        table.right() <= host.right(),
        "table {table:?} must stay inside the message {host:?}"
    );
    let header = cell(cx, 0, 0);
    let body = cell(cx, 1, 0);
    assert!(
        body.size.height > header.size.height,
        "squeezed prose cells must wrap: {body:?} vs {header:?}"
    );
}

#[gpui::test]
fn mounted_cell_content_sits_on_the_authored_edge(cx: &mut TestAppContext) {
    let cx = mount(
        cx,
        "| leading label | centered label | trailing label |\n|:---|:---:|---:|\n| a | b | c |\n",
    );

    // Body text is far narrower than its header, so each column has slack
    // for the alignment to place.
    let leading = (cell(cx, 1, 0), cell_content(cx, 1, 0));
    assert!(leading.1.size.width < leading.0.size.width / 2.0);
    assert_eq!(leading.1.left(), leading.0.left());

    // Interior columns carry 8 px padding on both sides.
    let centered = (cell(cx, 1, 1), cell_content(cx, 1, 1));
    assert!(centered.1.size.width < centered.0.size.width / 2.0);
    let slack_before = centered.1.left() - centered.0.left();
    let slack_after = centered.0.right() - centered.1.right();
    assert!(
        (slack_before - slack_after).abs() <= px(1.0),
        "centered content must split its slack: {centered:?}"
    );

    let trailing = (cell(cx, 1, 2), cell_content(cx, 1, 2));
    assert!(trailing.1.size.width < trailing.0.size.width / 2.0);
    assert_eq!(trailing.1.right(), trailing.0.right());
}
