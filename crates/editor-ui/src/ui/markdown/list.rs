use aqua_core::{CollectionSemantics, Insets, Role};
use aqua_text::{FontFamily, TextStyle};
use aqua_ui::element::{Element, ui};
use aqua_ui::style::{Length, Style};

use super::presenter::PresenterError;
use super::visual::{BodyContext, BodyTone, LeafKind, Sources, interactive_leaf, raw_leaf};
use super::{Block, ListItem};
use crate::ui::theme::ProseTypography;

struct Marker {
    indent: usize,
    glyph: String,
    ordered: bool,
    task: Option<bool>,
    position: usize,
    size: usize,
}

struct Row<'a> {
    block: Option<&'a Block>,
    indent: usize,
    markers: Vec<Marker>,
    gap: f32,
}

fn margins(block: &Block) -> (f32, f32) {
    match block {
        Block::Heading { level, .. } => {
            let heading = ProseTypography::heading(*level);
            (heading.margin_top_px, heading.margin_bottom_px)
        }
        Block::Paragraph { .. } => (
            ProseTypography::ITEM_PARAGRAPH_MARGIN_PX,
            ProseTypography::ITEM_PARAGRAPH_MARGIN_PX,
        ),
        Block::Html { .. } => (
            ProseTypography::PARAGRAPH_MARGIN_PX,
            ProseTypography::PARAGRAPH_MARGIN_PX,
        ),
        Block::List { .. } => (
            ProseTypography::NESTED_LIST_MARGIN_PX,
            ProseTypography::NESTED_LIST_MARGIN_PX,
        ),
        Block::Code(_) => (
            ProseTypography::FENCE_MARGIN_PX,
            ProseTypography::FENCE_MARGIN_PX,
        ),
        Block::Table(_) => (
            ProseTypography::TABLE_MARGIN_PX,
            ProseTypography::TABLE_MARGIN_PX,
        ),
    }
}

fn visit<'a>(blocks: &'a [Block], indent: usize, rows: &mut Vec<Row<'a>>) {
    let mut previous_bottom: f32 = 0.0;
    let mut previous_heading = false;
    for (index, block) in blocks.iter().enumerate() {
        let (top, bottom) = margins(block);
        let gap = if index == 0 {
            0.0
        } else if previous_heading {
            previous_bottom
        } else {
            previous_bottom.max(top)
        };
        let first = rows.len();
        match block {
            Block::List {
                ordered,
                start,
                items,
                ..
            } => visit_items(*ordered, start.unwrap_or(1), items, indent, rows),
            Block::Heading { .. }
            | Block::Paragraph { .. }
            | Block::Code(_)
            | Block::Html { .. }
            | Block::Table(_) => rows.push(Row {
                block: Some(block),
                indent,
                markers: Vec::new(),
                gap: 0.0,
            }),
        }
        if let Some(row) = rows.get_mut(first) {
            row.gap += gap;
        }
        previous_bottom = bottom;
        previous_heading = matches!(block, Block::Heading { level: 2..=4, .. });
    }
}

fn visit_items<'a>(
    ordered: bool,
    start: u64,
    items: &'a [ListItem],
    indent: usize,
    rows: &mut Vec<Row<'a>>,
) {
    for (index, item) in items.iter().enumerate() {
        let first = rows.len();
        if item.blocks.is_empty() {
            rows.push(Row {
                block: None,
                indent: indent + 1,
                markers: Vec::new(),
                gap: 0.0,
            });
        }
        visit(&item.blocks, indent + 1, rows);
        if let Some(row) = rows.get_mut(first) {
            let glyph = match item.task {
                Some(true) => "☑".to_owned(),
                Some(false) => "☐".to_owned(),
                None if ordered => format!("{}.", start.saturating_add(index as u64)),
                None => "•".to_owned(),
            };
            row.markers.push(Marker {
                indent,
                glyph,
                ordered,
                task: item.task,
                position: index + 1,
                size: items.len(),
            });
            if index > 0 {
                row.gap += ProseTypography::ITEM_GAP_PX;
            }
        }
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "one list render: each item's leaf, marker, indent and list semantics"
)]
pub(super) fn render(
    ordered: bool,
    start: Option<u64>,
    items: &[ListItem],
    body: BodyContext<'_>,
    prefix: &[u64],
    sources: &mut Sources<'_>,
) -> Result<Element, PresenterError> {
    let theme = body.theme;
    let mut rows = Vec::new();
    visit_items(ordered, start.unwrap_or(1), items, 0, &mut rows);
    let mut list = ui::Box::new().layout("column").width(Length::Pct(100.0));
    for (ordinal, row) in rows.into_iter().enumerate() {
        let mut order = prefix.to_vec();
        order.push(ordinal as u64);
        let source = sources.leaf(order, "\n");
        let leaf = match row.block {
            Some(Block::Paragraph { spans, .. }) => {
                interactive_leaf(spans, body, source, LeafKind::Prose, Vec::new())
                    .map_err(PresenterError::Inline)?
            }
            Some(Block::Heading { level, spans, .. }) => interactive_leaf(
                spans,
                body.with_tone(BodyTone::Foreground),
                source,
                LeafKind::Heading(*level),
                Vec::new(),
            )
            .map_err(PresenterError::Inline)?,
            Some(Block::Html { source: raw }) => raw_leaf(
                raw,
                theme,
                body.tone,
                source.document,
                source.id,
                source.order,
                source.separator,
            ),
            None => interactive_leaf(&[], body, source, LeafKind::Prose, Vec::new())
                .map_err(PresenterError::Inline)?,
            Some(Block::Code(_)) => return Err(PresenterError::Unavailable("list code fence")),
            Some(Block::Table(_)) => return Err(PresenterError::Unavailable("list table")),
            Some(Block::List { .. }) => return Err(PresenterError::Unavailable("list flow")),
        };
        sources.collected.push(leaf.source);
        let mut frame = ui::Box::new()
            .layout("column")
            .width(Length::Pct(100.0))
            .style(Style::new().padding_insets(Insets {
                left: super::super::rich::charts::scale::f32_of_count(row.indent)
                    * ProseTypography::LIST_INDENT_PX,
                top: row.gap,
                ..Insets::default()
            }))
            .child(leaf.element);
        let item = row
            .markers
            .first()
            .map(|marker| (marker.indent + 1, marker.task, marker.position, marker.size));
        for Marker {
            indent,
            glyph,
            ordered,
            task,
            ..
        } in row.markers
        {
            let color = theme.colors.text_muted.to_reference_text_color();
            let style = TextStyle {
                families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
                size_px: theme.typography.size(ProseTypography::BODY_SIZE_PX),
                line_height_px: theme.typography.size(ProseTypography::BODY_LINE_PX),
                weight: if ordered {
                    aqua_text::FontWeight::NORMAL
                } else {
                    ProseTypography::BODY_WEIGHT
                },
                letter_spacing_em: Some(theme.typography.tracking_em),
                ..TextStyle::default()
            };
            let mut marker = ui::Text::new(glyph)
                .text_style(style)
                .gpui_source_color(color)
                .wrap(false)
                .build();
            if task.is_some() {
                marker = marker.aria_hidden();
            }
            frame = frame.child(
                ui::Box::new()
                    .style(Style::new().absolute_at(
                        Some(
                            super::super::rich::charts::scale::f32_of_count(indent)
                                * ProseTypography::LIST_INDENT_PX,
                        ),
                        Some(row.gap),
                        None,
                        None,
                    ))
                    .child(marker)
                    .build(),
            );
        }
        let mut frame = frame.build();
        if let Some((level, task, position, size)) = item {
            frame = frame
                .role(Role::ListItem)
                .aria_level(level)
                .collection_semantics(CollectionSemantics {
                    position_in_set: Some(position),
                    set_size: Some(size),
                    ..CollectionSemantics::default()
                });
            if let Some(checked) = task {
                frame = frame.aria_checked(checked);
            }
        }
        list = list.child(frame);
    }
    Ok(list.build().role(Role::List))
}

#[cfg(test)]
#[path = "../../../tests/markdown/list.rs"]
mod tests;
