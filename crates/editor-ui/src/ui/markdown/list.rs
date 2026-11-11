use std::rc::Rc;

use aqua_core::Insets;
use aqua_text::document::{SelectionDocumentId, SelectionSource, SelectionSourceId};
use aqua_text::{FontFamily, TextStyle};
use aqua_ui::element::{Element, ui};
use aqua_ui::style::{Length, Style};

use super::presenter::PresenterError;
use super::visual::{BodyTone, LinkAction, interactive_leaf, raw_leaf};
use super::{Block, ListItem};
use crate::ui::rich::actions::{ResourceScope, ShowCopyMenu};
use crate::ui::theme::{ArtisanTheme, ProseTypography};

struct Row<'a> {
    block: Option<&'a Block>,
    indent: usize,
    markers: Vec<(usize, String, bool)>,
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
            _ => rows.push(Row {
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
            let marker = match item.task {
                Some(true) => "☑".to_owned(),
                Some(false) => "☐".to_owned(),
                None if ordered => format!("{}.", start.saturating_add(index as u64)),
                None => "•".to_owned(),
            };
            row.markers.push((indent, marker, ordered));
            if index > 0 {
                row.gap += ProseTypography::ITEM_GAP_PX;
            }
        }
    }
}

pub(super) fn render(
    ordered: bool,
    start: Option<u64>,
    items: &[ListItem],
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
    let mut rows = Vec::new();
    visit_items(ordered, start.unwrap_or(1), items, 0, &mut rows);
    let mut list = ui::Box::new().layout("column").width(Length::Pct(100.0));
    for (ordinal, row) in rows.into_iter().enumerate() {
        let mut order = prefix.to_vec();
        order.push(ordinal as u64);
        let id = source_id(sources.len());
        let leaf = match row.block {
            Some(Block::Paragraph { spans, .. }) => interactive_leaf(
                spans,
                theme,
                tone,
                document,
                id,
                order,
                "\n",
                on_link.clone(),
                on_color.clone(),
                scope.clone(),
                None,
            )
            .map_err(PresenterError::Inline)?,
            Some(Block::Heading { level, spans, .. }) => interactive_leaf(
                spans,
                theme,
                BodyTone::Foreground,
                document,
                id,
                order,
                "\n",
                on_link.clone(),
                on_color.clone(),
                scope.clone(),
                Some(*level),
            )
            .map_err(PresenterError::Inline)?,
            Some(Block::Html { source }) => {
                raw_leaf(source, theme, tone, document, id, order, "\n")
            }
            None => interactive_leaf(
                &[],
                theme,
                tone,
                document,
                id,
                order,
                "\n",
                on_link.clone(),
                on_color.clone(),
                scope.clone(),
                None,
            )
            .map_err(PresenterError::Inline)?,
            Some(Block::Code(_)) => return Err(PresenterError::Unavailable("list code fence")),
            Some(Block::Table(_)) => return Err(PresenterError::Unavailable("list table")),
            Some(Block::List { .. }) => return Err(PresenterError::Unavailable("list flow")),
        };
        sources.push(leaf.source);
        let mut frame = ui::Box::new()
            .width(Length::Pct(100.0))
            .style(Style::new().padding_insets(Insets {
                left: super::super::rich::charts::scale::f32_of_count(row.indent)
                    * ProseTypography::LIST_INDENT_PX,
                top: row.gap,
                ..Insets::default()
            }))
            .child(leaf.element);
        for (indent, marker, ordered) in row.markers {
            let color = theme.colors.text_muted.to_reference_text_color();
            let style = TextStyle {
                families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
                size_px: ProseTypography::BODY_SIZE_PX,
                line_height_px: ProseTypography::BODY_LINE_PX,
                weight: if ordered {
                    aqua_text::FontWeight::NORMAL
                } else {
                    ProseTypography::BODY_WEIGHT
                },
                ..TextStyle::default()
            };
            let marker = ui::Text::new(marker)
                .text_style(style)
                .gpui_source_color(color)
                .wrap(false)
                .build();
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
        list = list.child(frame.build());
    }
    Ok(list.build())
}
