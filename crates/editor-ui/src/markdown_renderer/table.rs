//! Virtualized Markdown table rows use equal shared native column tracks.
//! Every cell remains ordinary selectable inline text.
//!
//! Type follows the plugin `table`: 14 px / 24 px, header cells at 600 in
//! the foreground token above a hairline, body rows separated by hairlines,
//! 8 px cell padding with flush outer edges. Every cell is an ordinary
//! inline run, so emphasis, code, links, and selection behave exactly as in
//! a paragraph.

use gpui::{AnyElement, Div, IntoElement, ParentElement, Styled, div, prelude::*, px};

use crate::markdown::TableAlignment;
use crate::theme::{ArtisanTheme, ProseTypography};

use super::prepared::PreparedTable;
use super::{RenderContext, render_leaf};

/// Which part of the table a cell belongs to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RowKind {
    Header,
    Body,
}

/// Equal native tracks keep independently virtualized rows aligned.
pub(super) fn render_row(
    prepared: &PreparedTable,
    row_index: usize,
    render: &RenderContext<'_>,
) -> AnyElement {
    let theme = render.theme;
    let columns = prepared.alignments.len();
    let mut grid = div()
        .w_full()
        .grid()
        .grid_cols(u16::try_from(columns).unwrap_or(u16::MAX))
        .min_w_0()
        .text_size(px(ProseTypography::TABLE_SIZE_PX))
        .line_height(px(ProseTypography::TABLE_LINE_PX))
        .tracking(theme.typography.tracking_em);
    let kind = if row_index < prepared.header_rows {
        RowKind::Header
    } else {
        RowKind::Body
    };
    let Some(cells) = prepared.rows.get(row_index) else {
        return grid.into_any_element();
    };
    for (column, (alignment, prepared_cell)) in
        prepared.alignments.iter().zip(cells.iter()).enumerate()
    {
        let selector = render.id(prepared_cell.selector);
        let mut cell = cell_frame(kind, *alignment, column, columns, theme);
        if row_index + 1 < prepared.rows.len() || prepared.trailing_border {
            cell = cell.border_b_1().border_color(theme.colors.line.to_paint());
        }
        if let Some(leaf) = &prepared_cell.leaf {
            cell = cell.child(render_leaf(
                leaf,
                prepared_cell.selector,
                if column == 0 { "\n" } else { "\t" },
                render,
            ));
        }
        grid = grid.child(cell.debug_selector(move || selector.to_string()));
    }
    grid.into_any_element()
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
            .text_color(theme.colors.text.to_paint()),
        RowKind::Body => cell.pt(pad),
    }
}
