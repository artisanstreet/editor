//! Approvals and question records consume canonical requests and explicit send state.
use super::super::body_text;
use super::text::TextPresenter;
use super::{Action, Actions};
use crate::content::SourceField;
use artisan_api::{
    Answer, ApprovalRequest, Collection, Decision, Item, ItemKind, Status, ToolKind,
};
use editor_ui::{
    button::ButtonVariant,
    card::{CardStyle, compact_card, compact_card_content},
    chat_bubble::{ChatBubbleSegment, SegmentPosition},
    theme::ArtisanTheme,
};
use gpui::{
    AnyElement, Div, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    Styled as _, div,
};
pub(in crate::row) struct AnswerState {
    pub sending: bool,
    pub error: Option<String>,
}
pub(in crate::row) struct RequestPresenter<'a> {
    pub text: TextPresenter<'a>,
    pub collections: &'a crate::collections::CollectionOwners,
    pub actions: Actions,
}
impl RequestPresenter<'_> {
    fn card(&self, heading: &str, details: impl IntoElement) -> Div {
        let theme = self.text.theme;
        let style = CardStyle::resolve(theme);
        compact_card(style)
            .w_full()
            .child(compact_card_content(style).child(card_heading(heading, &theme)))
            .child(compact_card_content(style).child(details))
    }

    fn details_column(&self) -> Div {
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap(self.text.theme.spacing.steps(2.0))
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one GPUI builder keeps the approval card's parts in visual order"
    )]
    pub(in crate::row) fn approval_card(
        &self,
        item: &Item,
        state: AnswerState,
    ) -> Option<AnyElement> {
        let theme = self.text.theme;
        let muted = theme.colors.text_muted.to_paint();
        let (approval, files) = match &item.kind {
            ItemKind::Tool { approval, tool, .. } => {
                let files = match tool {
                    ToolKind::Edit {
                        changes: Collection::Inline(changes),
                    } => changes.iter().map(|change| change.path.clone()).collect(),
                    ToolKind::Edit {
                        changes: Collection::Stored(source),
                    } => {
                        vec![format!("{} file changes", source.count)]
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
                    | ToolKind::Other { .. } => Vec::new(),
                };
                (approval.as_ref(), files)
            }
            ItemKind::Plan { approval, .. } => (approval.as_ref(), Vec::new()),
            ItemKind::UserMessage { .. }
            | ItemKind::AssistantMessage { .. }
            | ItemKind::Agent { .. }
            | ItemKind::Question { .. }
            | ItemKind::Image { .. }
            | ItemKind::Compaction { .. }
            | ItemKind::Notice { .. }
            | ItemKind::Error { .. } => (None, Vec::new()),
        };
        let approval = approval?;
        let mut details = self.details_column();
        match &approval.request {
            ApprovalRequest::Command { cwd, .. } => {
                details = details.child(
                    div()
                        .font_family(theme.typography.mono.family)
                        .tracking(0.0)
                        .child("$ ")
                        .child(self.text.content_text(item, SourceField::ApprovalCommand)),
                );
                if let Some(cwd) = cwd {
                    details =
                        details.child(body_text(&format!("in {cwd}"), &theme).text_color(muted));
                }
            }
            ApprovalRequest::FileChange => {
                details = details.child(body_text("Apply these file changes?", &theme));
                for file in files {
                    details = details.child(
                        body_text(&file, &theme)
                            .font_family(theme.typography.mono.family)
                            .tracking(0.0),
                    );
                }
            }
            ApprovalRequest::Action { .. } => {
                details = details.child(
                    self.text
                        .content_text(item, SourceField::ApprovalDescription),
                );
            }
        }
        if approval.reason.is_some() {
            details = details.child(self.text.content_text(item, SourceField::ApprovalReason));
        }
        let AnswerState { sending, error } = state;
        let mut actions = div()
            .w_full()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap(theme.spacing.steps(2.0));
        for (index, decision) in approval
            .options
            .iter()
            .copied()
            .enumerate()
            .filter(|_| item.status == Status::AwaitingUser)
        {
            let variant = if index == 0 {
                ButtonVariant::Default
            } else {
                ButtonVariant::Outline
            };
            let id = item.id.clone();
            actions = actions.child(self.actions.button(
                &self.text.theme,
                format!("approve-{}-{index}", item.id),
                decision_label(decision),
                variant,
                sending,
                Action::Answer {
                    id,
                    answer: Answer::Approval { decision },
                },
            ));
        }
        if item.status == Status::AwaitingUser {
            details = details.child(actions);
        } else {
            let outcome = approval.decision.map_or(
                match item.status {
                    Status::Done => "Resolved",
                    Status::Declined => "Declined",
                    Status::Cancelled => "Cancelled",
                    Status::Failed => "Failed",
                    Status::Running | Status::AwaitingUser => "Pending",
                },
                decision_label,
            );
            details = details.child(body_text(outcome, &theme).text_color(muted));
        }
        if let Some(error) = error {
            details = details.child(body_text(&error, &theme).text_color(muted));
        }
        let heading = if matches!(
            &item.kind,
            ItemKind::Plan {
                proposal: Some(_),
                ..
            }
        ) {
            "Approve this plan?"
        } else {
            "Approval requested"
        };
        Some(self.card(heading, details).into_any_element())
    }

    pub(in crate::row) fn question_card(&self, item: &Item) -> AnyElement {
        let theme = self.text.theme;
        let ItemKind::Question {
            questions,
            answers,
            answer_text,
            ..
        } = &item.kind
        else {
            return div().into_any_element();
        };
        if answer_text.is_some() {
            return self.answered(item, questions);
        }
        let mut details = self.details_column().gap(theme.spacing.steps(2.0));
        match questions {
            Collection::Inline(questions) => {
                for (index, _) in questions.iter().enumerate() {
                    details =
                        details.child(self.text.content_text(item, SourceField::Question(index)));
                }
            }
            Collection::Stored(source) => {
                details = details.child(self.collections.question(
                    item.id.clone(),
                    source.clone(),
                    self.text.host.clone(),
                    &theme,
                ));
            }
        }
        details = details.child(
            body_text(
                if item.status == Status::AwaitingUser {
                    "Answer it above the message box."
                } else {
                    "Not answered."
                },
                &theme,
            )
            .text_color(theme.colors.text_muted.to_paint())
            .debug_selector(|| "question-status".to_owned()),
        );
        let heading = match answers {
            None => "Question",
            Some(Collection::Inline(answers)) if answers.iter().all(Option::is_none) => {
                "Skipped question"
            }
            Some(_) => "Answered question",
        };
        self.card(heading, details).into_any_element()
    }
}
impl RequestPresenter<'_> {
    /// An answered question reads as the user's own message: what was asked,
    /// muted, over their answer, in a dashed bubble on their side.
    fn answered(&self, item: &Item, questions: &Collection<artisan_api::Question>) -> AnyElement {
        let theme = self.text.theme;
        let mut asked = div()
            .flex()
            .flex_col()
            .gap(theme.spacing.steps(1.0))
            .text_size(theme.typography.control_text)
            .text_color(theme.colors.text_muted.to_paint());
        match questions {
            Collection::Inline(questions) => {
                for (index, _) in questions.iter().enumerate() {
                    asked = asked.child(self.text.content_text(item, SourceField::Question(index)));
                }
            }
            Collection::Stored(source) => {
                asked = asked.child(self.collections.question(
                    item.id.clone(),
                    source.clone(),
                    self.text.host.clone(),
                    &theme,
                ));
            }
        }
        let content = div()
            .flex()
            .flex_col()
            .gap(theme.spacing.steps(2.0))
            .child(asked)
            .child(self.text.answer(item));
        div()
            .w_full()
            .flex()
            .flex_col()
            .items_end()
            .child(
                ChatBubbleSegment::new(theme, SegmentPosition::Single, content)
                    .answer()
                    .debug_selector("question-answer"),
            )
            .into_any_element()
    }
}

fn card_heading(title: &str, theme: &ArtisanTheme) -> Div {
    div()
        .w_full()
        .min_w_0()
        .text_size(theme.typography.control_text)
        .font_weight(FontWeight::MEDIUM)
        .child(title.to_owned())
}

const fn decision_label(decision: Decision) -> &'static str {
    match decision {
        Decision::AllowOnce => "Approve",
        Decision::AllowAlways => "Always allow",
        Decision::DenyOnce => "Deny",
        Decision::DenyAlways => "Always deny",
        Decision::Cancelled => "Cancel",
    }
}

pub(in crate::row) const fn decision_note(decision: Decision) -> &'static str {
    match decision {
        Decision::AllowOnce => "You approved this request.",
        Decision::AllowAlways => "You allowed this from now on.",
        Decision::DenyOnce => "You denied this request.",
        Decision::DenyAlways => "You denied this from now on.",
        Decision::Cancelled => "Cancelled.",
    }
}
