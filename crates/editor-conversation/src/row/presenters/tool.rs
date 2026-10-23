//! Canonical tool and plan presentation. Expansion/live inputs are supplied by the row.
use super::super::{LINE, body_text};
use super::request::decision_note;
use super::text::TextPresenter;
use super::{Action, Actions};
use crate::{activity, content::SourceField};
use artisan_api::{Collection, Item, ItemKind, Output, TextContent, ToolKind};
use editor_ui::code_card::CodeCard;
use editor_ui::terminal_text::{TerminalPalette, TerminalScreen, highlights};
use editor_ui::theme::{ProseTypography, RadiusStep, RadiusTokens};
use gpui::StyledText;
use gpui::{
    AnyElement, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, div, prelude::FluentBuilder as _, px,
};
use std::collections::HashMap;
/// Output a command's terminal parses: its newest bytes, as a terminal's
/// scrollback keeps its newest lines.
const TERMINAL_BYTES: usize = 256 * 1024;
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
        preview: Option<AnyElement>,
    ) -> AnyElement {
        let theme = self.text.theme;
        if activity::category(item).is_none() {
            return div().into_any_element();
        }
        let message = activity::agent_message(item);
        let words = activity::line(item, agents);
        let muted = theme.colors.text_muted.to_paint();
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
        let has_details = details_available(item, message.as_ref(), live);
        let expanded = has_details && expanded;
        let mut sentence = div()
            .id(SharedString::from(format!("activity-{}", item.id)))
            .debug_selector(|| "work-activity".to_owned())
            .w_full()
            .min_w_0()
            .py(theme.spacing.steps(0.5))
            .text_size(px(ProseTypography::BODY_SIZE_PX))
            .line_height(px(LINE))
            .font_weight(ProseTypography::BODY_WEIGHT)
            .tracking(theme.typography.tracking_em)
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
                            .text_color(theme.colors.text.to_paint())
                            .child(words.verb.clone()),
                    )
                    .child(object),
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
            sentence = sentence.cursor_pointer().on_click(move |_, _, cx| flip(cx));
        }
        let mut block = div().w_full().min_w_0().flex().flex_col().child(sentence);
        if expanded {
            block = block.child(match &item.kind {
                // A message opens on its text, a row of its own.
                ItemKind::Tool { .. } if message.is_some() => div().into_any_element(),
                ItemKind::Tool { tool, output, .. } => {
                    let streamed = live.filter(|_| !item.status.is_terminal());
                    div()
                        .mt(theme.spacing.steps(1.0))
                        .child(self.tool_details(item, tool, output.as_ref(), streamed, preview))
                        .into_any_element()
                }
                ItemKind::UserMessage { .. }
                | ItemKind::AssistantMessage { .. }
                | ItemKind::Agent { .. }
                | ItemKind::Question { .. }
                | ItemKind::Plan { .. }
                | ItemKind::Image { .. }
                | ItemKind::Compaction { .. }
                | ItemKind::Notice { .. }
                | ItemKind::Error { .. } => div().into_any_element(),
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
        preview: Option<AnyElement>,
    ) -> AnyElement {
        if matches!(tool, ToolKind::Shell { .. }) {
            return self.terminal(item, output, live);
        }
        if live.is_none()
            && let Some(preview) = preview
        {
            return preview;
        }
        let theme = self.text.theme;
        let mut panel = div()
            .p(px(12.0))
            .rounded(RadiusTokens::value(RadiusStep::Lg))
            .bg(theme.colors.raised.to_paint())
            .font_family(theme.typography.mono.family)
            .tracking(0.0)
            .text_size(px(12.5))
            .line_height(px(19.0))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .overflow_hidden();
        match tool {
            ToolKind::Edit {
                changes: Collection::Stored(source),
            } => {
                panel = panel.child(self.collections.edit(
                    item.id.clone(),
                    source.clone(),
                    self.text.host.clone(),
                    &theme,
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
            ToolKind::AgentMessage { .. }
            | ToolKind::Agents { .. }
            | ToolKind::Shell { .. }
            | ToolKind::Read { .. }
            | ToolKind::Search { .. }
            | ToolKind::WebFetch { .. }
            | ToolKind::WebSearch { .. }
            | ToolKind::Mcp { .. }
            | ToolKind::ViewImage { .. }
            | ToolKind::Other { .. } => {}
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

    /// A command as a terminal shows it: the prompt line, then its output in
    /// the colours it printed, live while it runs.
    fn terminal(&self, item: &Item, output: Option<&Output>, live: Option<&str>) -> AnyElement {
        let theme = self.text.theme;
        let foreground = theme.colors.text.to_paint();
        let mut body = div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .font_family(theme.typography.mono.family)
            .text_size(px(ProseTypography::CODE_SIZE_PX))
            .line_height(px(ProseTypography::CODE_LINE_PX))
            .tracking(0.0)
            .text_color(foreground)
            .child(
                div()
                    .whitespace_normal()
                    .child("$ ")
                    .child(self.text.inherited_content(item, SourceField::Command)),
            );
        let text = live.or_else(|| output.and_then(|output| output.text.inline()));
        if let Some(text) = text.filter(|text| !text.trim().is_empty()) {
            // A terminal keeps its newest lines; so does this one.
            let start = text.len().saturating_sub(TERMINAL_BYTES);
            let start = (start..text.len())
                .find(|at| text.is_char_boundary(*at))
                .unwrap_or(text.len());
            let mut screen = TerminalScreen::new();
            screen.feed(&text.as_bytes()[start..]);
            let palette = TerminalPalette::for_theme(&theme);
            for line in screen.lines() {
                let runs = highlights(line, &palette, foreground, gpui::transparent_black());
                body = body.child(div().min_h(px(ProseTypography::CODE_LINE_PX)).child(
                    StyledText::new(SharedString::from(line.text.clone())).with_highlights(runs),
                ));
            }
        } else if output.is_some_and(|output| !output.is_empty()) {
            body = body.child(self.text.content_text(item, SourceField::Output));
        }
        CodeCard::new(theme, body)
            .debug_selector(format!("terminal-{}", item.id).into())
            .into_any_element()
    }

    pub(in crate::row) fn plan_entry(&self, item: &Item) -> AnyElement {
        // The steps themselves live in the inspector's checklist.
        let ItemKind::Plan { approval, .. } = &item.kind else {
            return div().into_any_element();
        };
        let mut body = div();
        if let Some(decision) = approval.as_ref().and_then(|approval| approval.decision) {
            body = body.child(body_text(decision_note(decision), &self.text.theme));
        }
        body.into_any_element()
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
        ItemKind::UserMessage { .. }
        | ItemKind::AssistantMessage { .. }
        | ItemKind::Agent { .. }
        | ItemKind::Question { .. }
        | ItemKind::Plan { .. }
        | ItemKind::Image { .. }
        | ItemKind::Compaction { .. }
        | ItemKind::Notice { .. }
        | ItemKind::Error { .. } => false,
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
