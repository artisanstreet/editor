//! Canonical tool and plan presentation. Expansion/live inputs are supplied by the row.
use super::super::{LINE, body_line, body_text, icon};
use super::request::decision_note;
use super::text::TextPresenter;
use super::{Action, Actions};
use crate::{activity, content::SourceField};
use artisan_api::{Collection, Item, ItemKind, Output, StepStatus, TextContent, ToolKind};
use editor_ui::AssetId;
use editor_ui::theme::{ProseTypography, RadiusStep, RadiusTokens};
use gpui::{
    AnyElement, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, div, prelude::FluentBuilder as _, px,
};
use std::collections::HashMap;
pub(in crate::row) struct ToolPresenter<'a> {
    pub text: TextPresenter<'a>,
    pub collections: &'a crate::collections::CollectionOwners,
    pub actions: Actions,
}
impl ToolPresenter<'_> {
    pub(in crate::row) fn activity_row(
        &self,
        item: &Item,
        expanded: bool,
        live: Option<&str>,
        agents: &HashMap<artisan_api::ItemId, String>,
    ) -> AnyElement {
        let theme = self.text.shared.theme;
        if activity::category(item).is_none() {
            return div().into_any_element();
        }
        let message = activity::agent_message(item);
        let words = activity::line(item, agents);
        let muted = theme.colors.muted_foreground.to_paint();
        let object = div()
            .min_w_0()
            .flex_1()
            .max_h(px(LINE))
            .truncate()
            .text_color(muted)
            .when(words.code, |object| {
                object
                    .font_family(theme.typography.mono.family)
                    .tracking(0.0)
                    .text_size(theme.typography.control_text)
            })
            .child(words.object.clone());
        let tail = words.tail.as_ref().map(|tail| tail_element(tail, &theme));
        let has_details = details_available(item, message.as_ref(), live);
        let expanded = has_details && expanded;
        let mut line = div()
            .id(SharedString::from(format!("activity-{}", item.id)))
            .debug_selector(|| "work-activity".to_owned())
            .w_full()
            .min_w_0()
            .py(theme.spacing.steps(0.5))
            .text_size(px(ProseTypography::BODY_SIZE_PX))
            .line_height(px(LINE))
            .font_weight(ProseTypography::BODY_WEIGHT)
            .tracking(ProseTypography::TRACKING_EM)
            .text_color(muted)
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(theme.spacing.steps(2.0))
                    .min_w_0()
                    .child(
                        div()
                            .flex_shrink_0()
                            .text_color(theme.colors.foreground.to_paint())
                            .child(words.verb.clone()),
                    )
                    .child(object)
                    .children(tail),
            )
            .children(crate::row::test_mark(format!(
                "work-activity-text:{} {}",
                words.verb, words.object
            )));
        if has_details {
            let id = item.id.clone();
            let flip = self.actions.handler(Action::ToggleItem {
                id,
                expanded: !expanded,
            });
            line = line.cursor_pointer().on_click(move |_, _, cx| flip(cx));
        }
        let mut block = div().w_full().min_w_0().flex().flex_col().child(line);
        if expanded {
            block = block.child(match &item.kind {
                // A message opens on its text, a row of its own.
                ItemKind::Tool { .. } if message.is_some() => div().into_any_element(),
                ItemKind::Tool { tool, output, .. } => {
                    let streamed = live.filter(|_| !item.status.is_terminal());
                    div()
                        .mt(theme.spacing.steps(1.0))
                        .child(self.tool_details(item, tool, output.as_ref(), streamed))
                        .into_any_element()
                }
                _ => div().into_any_element(),
            });
        }
        block.into_any_element()
    }

    /// The opened activity: its markdown preview, or while it runs, its
    /// command and the output so far; stored text where it isn't inline.
    fn tool_details(
        &self,
        item: &Item,
        tool: &ToolKind,
        output: Option<&Output>,
        live: Option<&str>,
    ) -> AnyElement {
        if live.is_none()
            && let Some(preview) = self.text.preview(item)
        {
            return preview;
        }
        let theme = self.text.shared.theme;
        let mut panel = div()
            .p(px(12.0))
            .rounded(RadiusTokens::value(RadiusStep::Lg))
            .bg(theme.colors.muted.to_paint())
            .font_family(theme.typography.mono.family)
            .tracking(0.0)
            .text_size(px(12.5))
            .line_height(px(19.0))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .overflow_hidden();
        match tool {
            ToolKind::Shell { .. } => {
                panel = panel.child(
                    div()
                        .text_color(theme.colors.foreground.to_paint())
                        .whitespace_normal()
                        .child("$ ")
                        .child(self.text.inherited_content(item, SourceField::Command)),
                );
            }
            ToolKind::Edit {
                changes: Collection::Stored(source),
            } => {
                panel = panel.child(self.collections.edit(
                    item.id.clone(),
                    source.clone(),
                    self.text.source_host.clone(),
                    theme,
                ));
            }
            ToolKind::Edit {
                changes: Collection::Inline(changes),
            } => {
                for (index, change) in changes.iter().enumerate() {
                    if let Some(TextContent::Stored(_)) = &change.diff {
                        panel = panel
                            .child(change.path.clone())
                            .child(self.text.content_text(item, SourceField::Diff(index)));
                    }
                }
            }
            ToolKind::Mcp { input: Some(_), .. } | ToolKind::Other { input: Some(_), .. } => {
                panel = panel.child(self.text.inherited_content(item, SourceField::Input));
            }
            _ => {}
        }
        if let Some(live) = live {
            panel = panel.child(self.text.output(item, live));
        } else if let Some(output) = output.filter(|output| !output.is_empty()) {
            panel = panel.child(match &output.text {
                TextContent::Inline(text) => self.text.output(item, text),
                TextContent::Stored(_) => self.text.content_text(item, SourceField::Output),
            });
            if output.omitted_bytes > 0 {
                panel = panel.child(format!(
                    "{} bytes were not retained by the original capture.",
                    output.omitted_bytes
                ));
            }
        }
        panel.into_any_element()
    }

    pub(in crate::row) fn plan_entry(&self, item: &Item) -> AnyElement {
        let ItemKind::Plan {
            steps, approval, ..
        } = &item.kind
        else {
            return div().into_any_element();
        };
        let mut body = div();
        if let Collection::Stored(source) = steps {
            body = body.child(self.collections.plan(
                item.id.clone(),
                source.clone(),
                self.text.source_host.clone(),
                self.text.shared.theme,
            ));
        }
        if let Some(decision) = approval.as_ref().and_then(|approval| approval.decision) {
            body = body.child(body_text(decision_note(decision), &self.text.shared.theme));
        }
        body.into_any_element()
    }

    pub(in crate::row) fn plan_step(&self, item: &Item, index: usize) -> AnyElement {
        let ItemKind::Plan { steps, .. } = &item.kind else {
            return div().into_any_element();
        };
        let Some(step) = steps.inline().and_then(|steps| steps.get(index)) else {
            return div().into_any_element();
        };
        let theme = self.text.shared.theme;
        let muted = theme.colors.muted_foreground.to_paint();
        let (asset, color) = match step.status {
            StepStatus::Done => (AssetId::TABLER_CIRCLE_CHECK, muted),
            StepStatus::InProgress => {
                (AssetId::TABLER_LOADER_2, theme.colors.foreground.to_paint())
            }
            StepStatus::Cancelled => (AssetId::TABLER_CIRCLE_X, muted),
            StepStatus::Pending => (AssetId::TABLER_CHECKLIST, muted),
        };
        body_line(div(), &theme)
            .flex()
            .items_center()
            .gap(theme.spacing.steps(2.0))
            .text_color(color)
            .when(step.status == StepStatus::Done, gpui::Styled::line_through)
            .child(icon(asset, color))
            .child(
                self.text
                    .inherited_content(item, SourceField::PlanStep(index)),
            )
            .into_any_element()
    }
}
/// What an edit changed, or why a call didn't finish, after its object.
fn tail_element(tail: &activity::Tail, theme: &editor_ui::theme::ArtisanTheme) -> gpui::Div {
    match tail {
        activity::Tail::Changed { added, removed } => div()
            .flex()
            .flex_shrink_0()
            .gap(theme.spacing.steps(1.0))
            .font_features(crate::motion::tabular_figures())
            .when(*added > 0, |tail| {
                tail.child(
                    div()
                        .text_color(theme.colors.banner_success.to_paint())
                        .child(format!("+{added}")),
                )
            })
            .when(*removed > 0, |tail| {
                tail.child(
                    div()
                        .text_color(theme.colors.destructive.to_paint())
                        .child(format!("\u{2212}{removed}")),
                )
            }),
        activity::Tail::Failed(why) => div()
            .flex_shrink_0()
            .text_color(theme.colors.destructive.to_paint())
            .child(why.clone()),
    }
}

fn details_available(
    item: &Item,
    message: Option<&activity::AgentMessage>,
    live: Option<&str>,
) -> bool {
    match &item.kind {
        // A message opens on what it said.
        ItemKind::Tool { .. } if message.is_some() => message
            .as_ref()
            .is_some_and(|message| message.text.byte_len() > 0),
        ItemKind::Tool { tool, output, .. } => {
            output.as_ref().is_some_and(|output| !output.is_empty())
                || matches!(tool, ToolKind::Shell { .. })
                || matches!(tool, ToolKind::Mcp { input: Some(input), .. } | ToolKind::Other { input: Some(input), .. } if input.byte_len() > 0)
                || matches!(tool, ToolKind::Edit { changes } if !changes.is_empty())
                || live.is_some()
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use artisan_api::Status;
    fn item(tool: ToolKind) -> Item {
        Item {
            id: artisan_api::ItemId::new(),
            thread_id: artisan_api::ThreadId::new(),
            position: 1,
            seq: 1,
            run_id: None,
            parent_id: None,
            status: Status::Running,
            created_at: 0,
            updated_at: 0,
            kind: ItemKind::Tool {
                tool,
                title: None,
                output: None,
                exit_code: None,
                approval: None,
            },
        }
    }
    #[test]
    fn canonical_tool_disclosure_uses_explicit_live_input_and_authored_message_body() {
        let mut tool = item(ToolKind::Read {
            path: "src/main.rs".to_owned(),
            range: None,
        });
        assert!(!details_available(&tool, None, None));
        assert!(details_available(&tool, None, Some("streamed output")));
        if let ItemKind::Tool { output, .. } = &mut tool.kind {
            *output = Some(Output::from_text("saved output"));
        }
        assert!(details_available(&tool, None, None));
        let mut message = item(ToolKind::AgentMessage {
            recipient: "agent-1".to_owned(),
            target: None,
            summary: Some("summary only".to_owned()),
            text: "".into(),
            direction: artisan_api::MessageDirection::Sent,
        });
        assert!(!details_available(
            &message,
            activity::agent_message(&message).as_ref(),
            None
        ));
        if let ItemKind::Tool {
            tool: ToolKind::AgentMessage { text, .. },
            ..
        } = &mut message.kind
        {
            *text = "Review this change".into();
        }
        assert!(details_available(
            &message,
            activity::agent_message(&message).as_ref(),
            None
        ));
    }
}
