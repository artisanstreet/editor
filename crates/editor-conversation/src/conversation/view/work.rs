use super::Dispatch;
use crate::conversation::activity;
use crate::conversation::adapter::Action;
use crate::conversation::fold::RunSummary;
use crate::conversation::phase;
use aqua_core::Role;
use aqua_ui::AlignItems;
use aqua_ui::element::{Box as UiBox, Element};
use aqua_ui::style::{Length, Style};
use artisan_api::{Item, ItemId, ItemKind, RunId, Status, TextContent, Timestamp};
use editor_ui::kit::{HitArea, IconSize, Space, Spaced, TextRole};
use editor_ui::theme::ArtisanTheme;
use std::collections::HashMap;

const HEADER_TAIL: Space = Space::S8;
const FAILURE_TAIL: Space = Space::S4;

pub const fn line_role(mono: bool) -> TextRole {
    match mono {
        true => TextRole::Code,
        false => TextRole::Prose,
    }
}

fn label(
    text: impl Into<String>,
    theme: ArtisanTheme,
    foreground: aqua_core::GpuiTextColor,
    mono: bool,
) -> Element {
    line_role(mono)
        .text(&theme, text.into())
        .gpui_source_color(foreground)
        .build()
}

fn spacer(space: Space) -> Element {
    UiBox::new()
        .width(Length::Pct(100.0))
        .height(space.px())
        .style(Style::new().flex_shrink(0.0))
        .build()
}

fn activator(key: String, label: impl Into<String>, child: Element, on_activate: impl Fn() + 'static) -> Element {
    HitArea::new(key, Role::Button, label)
        .child(child)
        .on_activate(on_activate)
        .build()
        .style(Style::new().height(Length::Auto).min_width(Length::Pct(0.0)))
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
        .spaced(Space::S4)
        .style(
            Style::new()
                .min_width(Length::Pct(0.0))
                .align(AlignItems::CENTER),
        )
        .child(animated_label);
    if controlled {
        near = near.child(chevron(summary.open));
    }
    let near = UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .style(Style::new().min_width(Length::Pct(0.0)))
        .child(near.build())
        .child(spacer(HEADER_TAIL))
        .build();
    match run.filter(|_| controlled).cloned() {
        Some(run) => {
            let open = !summary.open;
            let words = match open {
                true => "Show work",
                false => "Hide work",
            };
            Some(activator(format!("work-header-{run}"), words, near, move || {
                dispatch(Action::ToggleWork {
                    run: run.clone(),
                    open,
                })
            }))
        }
        None => Some(near),
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
        .spaced(Space::S4)
        .style(
            Style::new()
                .align(AlignItems::CENTER)
                .min_width(Length::Pct(0.0)),
        )
        .child(IconSize::Md.glyph(icon, color.to_reference_paint()))
        .child(phrase(text.clone(), color.to_reference_text_color()))
        .child(chevron(open, color.to_reference_paint()))
        .build();
    Some(activator(format!("work-chain-{first}"), text, child, move || {
        dispatch(Action::ToggleChain {
            id: first.clone(),
            open: !open,
        })
    }))
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
        .spaced(Space::S8)
        .style(
            Style::new()
                .align(AlignItems::CENTER)
                .min_width(Length::Pct(0.0)),
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
    Some(activator(
        format!("work-agent-{id}"),
        "Open agent",
        content,
        move || dispatch(Action::OpenAgent(id.clone())),
    ))
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
        .spaced(Space::S8)
        .style(
            Style::new()
                .align(AlignItems::CENTER)
                .min_width(Length::Pct(0.0)),
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
    let sentence = match has_details {
        true => {
            let id = item.id.clone();
            activator(format!("work-item-{id}"), "Toggle details", line, move || {
                dispatch(Action::ToggleItem {
                    id: id.clone(),
                    expanded: !expanded,
                })
            })
        }
        false => UiBox::new()
            .width(Length::Pct(100.0))
            .style(Style::new().min_width(Length::Pct(0.0)))
            .child(line)
            .build(),
    };
    Ok(match failure.filter(|_| expanded && has_details) {
        Some(error) => Some(
            UiBox::new()
                .layout("column")
                .width(Length::Pct(100.0))
                .child(sentence)
                .child(match error {
                    TextContent::Stored(_) => stored_failure()?,
                    TextContent::Inline(error) => TextRole::Label
                        .text(&theme, error.trim().to_owned())
                        .gpui_source_color(theme.colors.destructive_text.to_reference_text_color())
                        .build(),
                })
                .child(spacer(FAILURE_TAIL))
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
        content = content.child(TextRole::Label.text(&theme, text).build());
    }
    Some(content.build())
}

#[cfg(test)]
#[path = "../../../tests/conversation/view/work.rs"]
mod tests;
