use super::presenter::PresenterError;
use super::visual::{BodyContext, LeafKind, Sources, interactive_leaf};
use super::{Table, TableAlignment};
use crate::ui::theme::ProseTypography;
use aqua_core::{CollectionSemantics, Insets, Role};
use aqua_ui::element::{Element, ui};
use aqua_ui::style::{Length, Style};

#[expect(
    clippy::too_many_lines,
    reason = "one table render: column widths, alignment, header row and cell leaves"
)]
pub(super) fn render(
    table: &Table,
    body: BodyContext<'_>,
    prefix: &[u64],
    sources: &mut Sources<'_>,
) -> Result<Element, PresenterError> {
    let theme = body.theme;
    let columns = table.alignments.len();
    if columns == 0 {
        return Ok(ui::Box::new().build());
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "table column count is limited by the preview budget"
    )]
    let width = 100.0 / columns as f32;
    let mut output = ui::Box::new().layout("column").width(Length::Pct(100.0));
    let rows = std::iter::once(&table.header).chain(&table.rows);
    let total_rows = table.rows.len() + 1;
    for (row, cells) in rows.enumerate() {
        let mut grid = ui::Box::new().layout("row").width(Length::Pct(100.0));
        for (column, alignment) in table.alignments.iter().enumerate() {
            let align = match alignment {
                TableAlignment::Unset | TableAlignment::Left => aqua_ui::AlignItems::START,
                TableAlignment::Center => aqua_ui::AlignItems::CENTER,
                TableAlignment::Right => aqua_ui::AlignItems::END,
            };
            let mut style = Style::new()
                .width(Length::Pct(width))
                .min_width(Length::Px(0.0))
                .align(align)
                .padding_insets(Insets {
                    left: if column > 0 {
                        ProseTypography::TABLE_CELL_PAD_PX
                    } else {
                        0.0
                    },
                    right: if column + 1 < columns {
                        ProseTypography::TABLE_CELL_PAD_PX
                    } else {
                        0.0
                    },
                    top: if row > 0 {
                        ProseTypography::TABLE_CELL_PAD_PX
                    } else {
                        0.0
                    },
                    bottom: ProseTypography::TABLE_CELL_PAD_PX,
                });
            if row == 0 {
                style = style.justify(aqua_ui::JustifyContent::END);
            }
            if row + 1 < total_rows {
                style = style.gpui_border(
                    Insets {
                        bottom: 1.0,
                        ..Insets::default()
                    },
                    theme.colors.line.to_reference_paint(),
                    false,
                );
            }
            let mut cell = ui::Box::new().layout("column").style(style);
            if let Some(value) = cells.get(column) {
                let mut order = prefix.to_vec();
                order.push(row as u64);
                order.push(column as u64);
                let source = sources.leaf(order, if column == 0 { "\n" } else { "\t" });
                let leaf = interactive_leaf(
                    &value.spans,
                    body,
                    source,
                    LeafKind::TableCell { header: row == 0 },
                    Vec::new(),
                )
                .map_err(PresenterError::Inline)?;
                sources.collected.push(leaf.source);
                cell = cell.child(leaf.element);
            }
            let role = match row {
                0 => Role::ColumnHeader,
                _ => Role::Cell,
            };
            grid = grid.child(
                cell.build()
                    .role(role)
                    .collection_semantics(CollectionSemantics {
                        row_index: Some(row),
                        column_index: Some(column),
                        ..CollectionSemantics::default()
                    }),
            );
        }
        output = output.child(grid.build().role(Role::Row).collection_semantics(
            CollectionSemantics {
                row_index: Some(row),
                ..CollectionSemantics::default()
            },
        ));
    }
    Ok(output
        .build()
        .role(Role::Table)
        .collection_semantics(CollectionSemantics {
            row_count: Some(total_rows),
            column_count: Some(columns),
            ..CollectionSemantics::default()
        }))
}

#[cfg(test)]
#[path = "../../../tests/markdown/table_view.rs"]
mod tests;
