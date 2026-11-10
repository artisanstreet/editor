use super::Dispatch;
use crate::conversation::activity;
use crate::conversation::adapter::Action;
use crate::conversation::fold::RunSummary;
use crate::conversation::phase;
use aqua_core::{CursorIcon, Insets};
use aqua_text::{FontFamily, TextStyle};
use aqua_ui::AlignItems;
use aqua_ui::element::{Box as UiBox, Button as UiButton, Element, Text};
use aqua_ui::style::{Length, Style};
use artisan_api::{Item, ItemId, ItemKind, RunId, Status, TextContent, Timestamp};
use editor_ui::asset::source_asset_glyph;
use editor_ui::theme::{ArtisanTheme, ProseTypography};
use std::collections::HashMap;

fn label(
    text: impl Into<String>,
    theme: ArtisanTheme,
    foreground: aqua_core::GpuiTextColor,
    mono: bool,
) -> Element {
    Text::new(text.into())
        .text_style(TextStyle {
            families: vec![FontFamily::Named(
                if mono {
                    theme.typography.mono.family
                } else {
                    theme.typography.sans.family
                }
                .to_owned(),
            )],
            size_px: if mono {
                theme.typography.control_text
            } else {
                ProseTypography::BODY_SIZE_PX
            },
            line_height_px: 24.0,
            weight: ProseTypography::BODY_WEIGHT,
            ..TextStyle::default()
        })
        .gpui_source_color(foreground)
        .build()
}

pub fn section_title(summary: &RunSummary, now: Timestamp) -> String {
    let end = summary.ended_at.unwrap_or(now);
    let elapsed = summary
        .started_at
        .and_then(|start| u64::try_from(end.saturating_sub(start)).ok())
        .unwrap_or(0);
    phase::section_title(summary.phase, summary.worked, elapsed)
}

pub fn header(
    run: Option<&RunId>,
    segment: usize,
    has_work: bool,
    summary: Option<&RunSummary>,
    theme: ArtisanTheme,
    animated_label: Element,
    chevron: impl FnOnce(bool) -> Element,
    dispatch: Dispatch,
) -> Option<Element> {
    if segment != 0 {
        return None;
    }
    let summary = summary?;
    let controlled = !summary.live() && run.is_some() && has_work;
    let mut near = UiBox::new()
        .layout("row")
        .gap(theme.spacing.steps(1.0))
        .style(
            Style::new()
                .min_width(Length::Px(0.0))
                .align(AlignItems::CENTER),
        )
        .child(animated_label);
    if controlled {
        near = near.child(chevron(summary.open));
    }
    let near = near.build();
    let style = Style::new()
        .min_width(Length::Px(0.0))
        .padding_insets(Insets {
            bottom: theme.spacing.steps(2.0),
            ..Insets::default()
        });
    if let Some(run) = run.filter(|_| controlled).cloned() {
        let open = !summary.open;
        Some(
            UiButton::new()
                .width(Length::Pct(100.0))
                .style(style)
                .child(near)
                .on_activate(move || {
                    dispatch(Action::ToggleWork {
                        run: run.clone(),
                        open,
                    })
                })
                .build()
                .cursor(CursorIcon::Pointer),
        )
    } else {
        Some(
            UiBox::new()
                .width(Length::Pct(100.0))
                .style(style)
                .child(near)
                .build(),
        )
    }
}

pub fn chain(
    ids: &[ItemId],
    items: &HashMap<ItemId, std::sync::Arc<Item>>,
    open: bool,
    theme: ArtisanTheme,
    phrase: impl FnOnce(String, aqua_core::GpuiTextColor) -> Element,
    chevron: impl FnOnce(bool, aqua_core::GpuiHsla) -> Element,
    dispatch: Dispatch,
) -> Option<Element> {
    let first = ids.first()?.clone();
    let categories = activity::chain_categories(
        ids.iter()
            .filter_map(|id| items.get(id).map(std::sync::Arc::as_ref)),
    );
    let text = activity::chain_clause(categories.iter().copied());
    let icon = activity::chain_icon(categories.iter().copied());
    let color = if open {
        theme.colors.text
    } else {
        theme.colors.text_muted
    };
    let child = UiBox::new()
        .layout("row")
        .width(Length::Pct(100.0))
        .gap(theme.spacing.steps(1.0))
        .style(
            Style::new()
                .align(AlignItems::CENTER)
                .min_width(Length::Px(0.0))
                .padding_insets(Insets {
                    top: theme.spacing.steps(0.5),
                    bottom: theme.spacing.steps(0.5),
                    ..Insets::default()
                }),
        )
        .child(source_asset_glyph(icon, 16.0, color.to_reference_paint()))
        .child(phrase(text, color.to_reference_text_color()))
        .child(chevron(open, color.to_reference_paint()))
        .build();
    Some(
        UiButton::new()
            .width(Length::Pct(100.0))
            .child(child)
            .on_activate(move || {
                dispatch(Action::ToggleChain {
                    id: first.clone(),
                    open: !open,
                })
            })
            .build()
            .cursor(CursorIcon::Pointer),
    )
}

pub fn agent(
    item: &Item,
    theme: ArtisanTheme,
    truncate: impl FnOnce(String, aqua_core::GpuiTextColor) -> Element,
    dispatch: Dispatch,
) -> Option<Element> {
    let ItemKind::Agent { description, .. } = &item.kind else {
        return None;
    };
    let task = match description {
        TextContent::Inline(text) => text.lines().next().unwrap_or_default().to_owned(),
        TextContent::Stored(_) => "Stored task. Open the agent to read it.".to_owned(),
    };
    let id = item.id.clone();
    let content = UiBox::new()
        .layout("row")
        .width(Length::Pct(100.0))
        .gap(theme.spacing.steps(2.0))
        .style(
            Style::new()
                .align(AlignItems::CENTER)
                .min_width(Length::Px(0.0))
                .padding_insets(Insets {
                    top: theme.spacing.steps(0.5),
                    bottom: theme.spacing.steps(0.5),
                    ..Insets::default()
                }),
        )
        .child(label(
            "Delegated",
            theme,
            theme.colors.text.to_reference_text_color(),
            false,
        ))
        .child(truncate(
            task,
            theme.colors.text_muted.to_reference_text_color(),
        ))
        .build();
    Some(
        UiButton::new()
            .width(Length::Pct(100.0))
            .child(content)
            .on_activate(move || dispatch(Action::OpenAgent(id.clone())))
            .build()
            .cursor(CursorIcon::Pointer),
    )
}

pub fn activity_row(
    item: &Item,
    expanded: bool,
    agents: &HashMap<ItemId, String>,
    theme: ArtisanTheme,
    truncate: impl FnOnce(String, aqua_core::GpuiTextColor, bool) -> Element,
    dispatch: Dispatch,
    stored_failure: impl FnOnce() -> anyhow::Result<Element>,
) -> anyhow::Result<Option<Element>> {
    if activity::category(item).is_none() {
        return Ok(None);
    }
    let words = activity::line(item, agents);
    let failure = match &item.kind {
        ItemKind::Tool {
            output: Some(output),
            ..
        } if item.status == Status::Failed && output.text.byte_len() > 0 => Some(&output.text),
        _ => None,
    };
    let has_details = failure.is_some()
        || activity::agent_message(item).is_some_and(|message| message.text.byte_len() > 0);
    let line = UiBox::new()
        .layout("row")
        .gap(theme.spacing.steps(2.0))
        .style(
            Style::new()
                .align(AlignItems::CENTER)
                .min_width(Length::Px(0.0)),
        )
        .child(label(
            words.verb,
            theme,
            theme.colors.text.to_reference_text_color(),
            false,
        ))
        .child(truncate(
            words.object,
            theme.colors.text_muted.to_reference_text_color(),
            words.code,
        ))
        .build();
    let style = Style::new()
        .min_width(Length::Px(0.0))
        .padding_insets(Insets {
            top: theme.spacing.steps(0.5),
            bottom: theme.spacing.steps(0.5),
            ..Insets::default()
        });
    let sentence = if has_details {
        let id = item.id.clone();
        UiButton::new()
            .width(Length::Pct(100.0))
            .style(style)
            .child(line)
            .on_activate(move || {
                dispatch(Action::ToggleItem {
                    id: id.clone(),
                    expanded: !expanded,
                })
            })
            .build()
            .cursor(CursorIcon::Pointer)
    } else {
        UiBox::new()
            .width(Length::Pct(100.0))
            .style(style)
            .child(line)
            .build()
    };
    Ok(match failure.filter(|_| expanded && has_details) {
        Some(error) => Some(
            UiBox::new()
                .layout("column")
                .width(Length::Pct(100.0))
                .child(sentence)
                .child(
                    UiBox::new()
                        .width(Length::Pct(100.0))
                        .style(Style::new().padding_insets(Insets {
                            bottom: theme.spacing.steps(1.0),
                            ..Insets::default()
                        }))
                        .child(match error {
                            TextContent::Stored(_) => stored_failure()?,
                            TextContent::Inline(error) => Text::new(error.trim().to_owned())
                                .text_style(TextStyle {
                                    families: vec![FontFamily::Named(
                                        theme.typography.sans.family.to_owned(),
                                    )],
                                    size_px: theme.typography.control_text,
                                    ..TextStyle::default()
                                })
                                .gpui_source_color(
                                    theme.colors.destructive_text.to_reference_text_color(),
                                )
                                .build(),
                        })
                        .build(),
                )
                .build(),
        ),
        None => Some(sentence),
    })
}

pub fn plan_entry(item: &Item, theme: ArtisanTheme) -> Option<Element> {
    let ItemKind::Plan { approval, .. } = &item.kind else {
        return None;
    };
    let mut content = UiBox::new().layout("column").width(Length::Pct(100.0));
    if let Some(decision) = approval.as_ref().and_then(|approval| approval.decision) {
        let text = match decision {
            artisan_api::Decision::AllowOnce => "You approved this request.",
            artisan_api::Decision::AllowAlways => "You allowed this from now on.",
            artisan_api::Decision::DenyOnce => "You denied this request.",
            artisan_api::Decision::DenyAlways => "You denied this from now on.",
            artisan_api::Decision::Cancelled => "Cancelled.",
        };
        content = content.child(
            Text::new(text)
                .text_style(TextStyle {
                    families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
                    size_px: theme.typography.editor_text_desktop,
                    line_height_px: theme.spacing.steps(6.0),
                    ..TextStyle::default()
                })
                .gpui_source_color(theme.colors.text.to_reference_text_color())
                .build(),
        );
    }
    Some(content.build())
}

#[cfg(test)]
#[path = "../../../tests/conversation/view/work.rs"]
mod tests;
