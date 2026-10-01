//! Native table rendering for the Markdown renderer.
//!
//! A table is one grid whose cells arrive in row order, so every column
//! shares one track and rows stay aligned without measuring text here. The
//! tracks are content-sized (`minmax(0, max-content)`): short columns hug
//! their content, and when the row outgrows the message width the tracks
//! shrink and cells wrap instead of overflowing. The reference table fills
//! its container (`width: 100%` under automatic table layout); GPUI grids
//! only offer uniform track templates, so the native table hugs its content
//! at the leading edge instead of stretching every column equally.
//!
//! Type follows the plugin `table`: 14 px / 24 px, header cells at 600 in
//! the foreground token above a hairline, body rows separated by hairlines,
//! 8 px cell padding with flush outer edges. Every cell is an ordinary
//! inline run, so emphasis, code, links, and selection behave exactly as in
//! a paragraph.

use gpui::{AnyElement, Div, IntoElement, ParentElement, Styled, div, prelude::*, px};

use crate::markdown::{Table, TableAlignment};
use crate::theme::{ArtisanTheme, ProseTypography};

use super::prepared::PreparedTable;
use super::{RenderContext, render_leaf};

/// Which part of the table a cell belongs to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RowKind {
    Header,
    Body,
}

/// Renders one prepared table. `table` is the parsed table, read only for
/// the cell spans a title change re-flattens.
pub(super) fn render_table(
    table: Option<&Table>,
    prepared: &PreparedTable,
    render: &RenderContext<'_>,
) -> AnyElement {
    let theme = render.theme;
    let selector = render.id(prepared.table);
    let columns = prepared.alignments.len();
    let mut grid = div()
        .grid()
        .grid_cols_max_content(u16::try_from(columns).unwrap_or(u16::MAX))
        .min_w_0()
        .text_size(px(ProseTypography::TABLE_SIZE_PX))
        .line_height(px(ProseTypography::TABLE_LINE_PX))
        .letter_spacing(px(ProseTypography::body_tracking_px(
            ProseTypography::TABLE_SIZE_PX,
        )));

    let last_row = prepared.rows.len().saturating_sub(1);
    for (row_index, cells) in prepared.rows.iter().enumerate() {
        let kind = if row_index == 0 {
            RowKind::Header
        } else {
            RowKind::Body
        };
        let authored = table.and_then(|table| {
            if row_index == 0 {
                Some(&table.header)
            } else {
                table.rows.get(row_index - 1)
            }
        });
        for (column, (alignment, prepared_cell)) in
            prepared.alignments.iter().zip(cells.iter()).enumerate()
        {
            let cell_selector = render.id(prepared_cell.selector);
            let mut cell = cell_frame(kind, *alignment, column, columns, theme);
            if row_index < last_row {
                cell = cell
                    .border_b_1()
                    .border_color(theme.colors.border.to_paint());
            }
            // The grammar pads short rows and drops excess cells; a missing
            // cell still takes its slot so later rows never shift columns.
            if let Some(leaf) = &prepared_cell.leaf {
                let spans = authored
                    .and_then(|cells| cells.get(column))
                    .map_or(&[][..], |cell| cell.spans.as_slice());
                let content_selector = render.id(prepared_cell.content);
                // The content keeps its automatic minimum width: it feeds
                // the track's max-content size, and a zero minimum here
                // collapses every column to its padding. Shrinking belongs
                // to the cell.
                cell = cell.child(
                    div()
                        .debug_selector(move || content_selector.to_string())
                        .child(render_leaf(spans, leaf, cell_selector.clone(), render)),
                );
            }
            grid = grid.child(cell.debug_selector(move || cell_selector.to_string()));
        }
    }

    // A flex row sizes the grid to its content and lets it shrink to the
    // message width; as a column child the grid would stretch instead.
    div()
        .w_full()
        .min_w_0()
        .flex()
        .flex_row()
        .child(grid.debug_selector(move || selector.to_string()))
        .into_any_element()
}

/// One cell's box: padding, alignment, and the header type treatment. The
/// content hugs its text, so cross-axis alignment places it on the
/// column's authored edge.
fn cell_frame(
    kind: RowKind,
    alignment: TableAlignment,
    column: usize,
    columns: usize,
    theme: &ArtisanTheme,
) -> Div {
    let pad = px(ProseTypography::TABLE_CELL_PAD_PX);
    let mut cell = div().min_w_0().flex().flex_col().pb(pad);
    cell = match alignment {
        TableAlignment::Unset | TableAlignment::Left => cell.items_start(),
        TableAlignment::Center => cell.items_center(),
        TableAlignment::Right => cell.items_end(),
    };
    if column > 0 {
        cell = cell.pl(pad);
    }
    if column + 1 < columns {
        cell = cell.pr(pad);
    }
    match kind {
        // Header cells sit on the hairline (`vertical-align: bottom`) and
        // take no top padding.
        RowKind::Header => cell
            .justify_end()
            .font_weight(ProseTypography::STRONG_WEIGHT)
            .text_color(theme.colors.foreground.to_paint()),
        RowKind::Body => cell.pt(pad),
    }
}
