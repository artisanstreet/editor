use super::presenter::PresenterError;
use super::visual::{BodyTone, LinkAction, interactive_table_leaf};
use super::{Table, TableAlignment};
use crate::ui::rich::actions::{ResourceScope, ShowCopyMenu};
use crate::ui::theme::{ArtisanTheme, ProseTypography};
use aqua_core::Insets;
use aqua_text::document::{SelectionDocumentId, SelectionSource, SelectionSourceId};
use aqua_ui::element::{Element, ui};
use aqua_ui::style::{Length, Style};
use std::rc::Rc;

pub(super) fn render(
    table: &Table,
    theme: &ArtisanTheme,
    tone: BodyTone,
    document: SelectionDocumentId,
    prefix: &[u64],
    source_id: &mut dyn FnMut(usize) -> SelectionSourceId,
    sources: &mut Vec<SelectionSource>,
    on_link: Option<Rc<dyn Fn(LinkAction)>>,
    on_color: Option<Rc<dyn Fn(ShowCopyMenu)>>,
    scope: Option<ResourceScope>,
) -> Result<Element, PresenterError> {
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
                let leaf = interactive_table_leaf(
                    &value.spans,
                    theme,
                    tone,
                    row == 0,
                    document,
                    source_id(sources.len()),
                    order,
                    if column == 0 { "\n" } else { "\t" },
                    on_link.clone(),
                    on_color.clone(),
                    scope.clone(),
                )
                .map_err(PresenterError::Inline)?;
                sources.push(leaf.source);
                cell = cell.child(leaf.element);
            }
            grid = grid.child(cell.build());
        }
        output = output.child(grid.build());
    }
    Ok(output.build())
}
