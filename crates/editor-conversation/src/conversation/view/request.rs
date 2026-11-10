use crate::conversation::adapter::Action;
use crate::conversation::content::SourceField;
use editor_ui::button::{Button, ButtonVariant};
use editor_ui::card::{CardStyle, compact_card, compact_card_content};
use editor_ui::theme::ArtisanTheme;
use aqua_text::{FontFamily, FontWeight, TextStyle};
use aqua_ui::element::{Box as UiBox, Element, Text};
use aqua_ui::style::{Length, Style};
use artisan_api::{
    Answer, ApprovalRequest, Collection, Decision, Item, ItemKind, Status, ToolKind,
};
use std::rc::Rc;

pub type Dispatch = Rc<dyn Fn(Action)>;

fn body(text: impl Into<String>, theme: ArtisanTheme, muted: bool, mono: bool) -> Element {
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
            size_px: theme.typography.editor_text_desktop,
            line_height_px: theme.spacing.steps(6.0),
            ..TextStyle::default()
        })
        .gpui_source_color(if muted {
            theme.colors.text_muted.to_reference_text_color()
        } else {
            theme.colors.text.to_reference_text_color()
        })
        .build()
}

fn heading(text: &str, theme: ArtisanTheme) -> Element {
    Text::new(text.to_owned())
        .text_style(TextStyle {
            families: vec![FontFamily::Named(theme.typography.sans.family.to_owned())],
            size_px: theme.typography.control_text,
            weight: FontWeight::MEDIUM,
            ..TextStyle::default()
        })
        .gpui_source_color(theme.colors.text.to_reference_text_color())
        .build()
}

fn card(theme: ArtisanTheme, title: &str, details: Element) -> Element {
    let style = CardStyle::resolve(theme);
    compact_card(style)
        .width(Length::Pct(100.0))
        .child(
            compact_card_content(style)
                .child(heading(title, theme))
                .build(),
        )
        .child(compact_card_content(style).child(details).build())
        .build()
}

fn decision_label(decision: Decision) -> &'static str {
    match decision {
        Decision::AllowOnce => "Approve",
        Decision::AllowAlways => "Always allow",
        Decision::DenyOnce => "Deny",
        Decision::DenyAlways => "Always deny",
        Decision::Cancelled => "Cancel",
    }
}

pub fn approval<E>(
    item: &Item,
    sending: bool,
    error: Option<&str>,
    theme: ArtisanTheme,
    mut source: impl FnMut(SourceField) -> Result<Element, E>,
    dispatch: Dispatch,
) -> Result<Option<Element>, E> {
    let (approval, files) = match &item.kind {
        ItemKind::Tool { approval, tool, .. } => {
            let files = match tool {
                ToolKind::Edit {
                    changes: Collection::Inline(changes),
                } => changes.iter().map(|change| change.path.clone()).collect(),
                ToolKind::Edit {
                    changes: Collection::Stored(source),
                } => vec![format!("{} file changes", source.count)],
                _ => Vec::new(),
            };
            (approval.as_ref(), files)
        }
        ItemKind::Plan { approval, .. } => (approval.as_ref(), Vec::new()),
        _ => (None, Vec::new()),
    };
    let Some(approval) = approval else {
        return Ok(None);
    };
    let mut details = UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .gap(theme.spacing.steps(2.0));
    match &approval.request {
        ApprovalRequest::Command { cwd, .. } => {
            let command = UiBox::new()
                .layout("row")
                .child(body("$ ", theme, false, true))
                .child(source(SourceField::ApprovalCommand)?)
                .build();
            details = details.child(command);
            if let Some(cwd) = cwd {
                details = details.child(body(format!("in {cwd}"), theme, true, false));
            }
        }
        ApprovalRequest::FileChange => {
            details = details.child(body("Apply these file changes?", theme, false, false));
            for file in files {
                details = details.child(body(file, theme, false, true));
            }
        }
        ApprovalRequest::Action { .. } => {
            details = details.child(source(SourceField::ApprovalDescription)?)
        }
    }
    if approval.reason.is_some() {
        details = details.child(source(SourceField::ApprovalReason)?);
    }
    if item.status == Status::AwaitingUser {
        let mut actions = UiBox::new()
            .layout("row")
            .width(Length::Pct(100.0))
            .gap(theme.spacing.steps(2.0))
            .style(Style::new().wrap(true));
        for (index, decision) in approval.options.iter().copied().enumerate() {
            let id = item.id.clone();
            let dispatch = Rc::clone(&dispatch);
            let variant = if index == 0 {
                ButtonVariant::Default
            } else {
                ButtonVariant::Outline
            };
            actions = actions.child(
                Button::text(
                    format!("approve-{}-{index}", item.id),
                    theme,
                    decision_label(decision),
                )
                .variant(variant)
                .disabled(sending)
                .on_activate(move |_| {
                    dispatch(Action::Answer {
                        id: id.clone(),
                        answer: Answer::Approval { decision },
                    })
                })
                .build(),
            );
        }
        details = details.child(actions.build());
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
        details = details.child(body(outcome, theme, true, false));
    }
    if let Some(error) = error {
        details = details.child(body(error, theme, true, false));
    }
    let title = if matches!(
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
    Ok(Some(card(theme, title, details.build())))
}

pub fn question<E>(
    item: &Item,
    theme: ArtisanTheme,
    mut source: impl FnMut(SourceField) -> Result<Element, E>,
    stored_questions: impl FnOnce(&artisan_api::CollectionRef) -> Result<Element, E>,
    answered: impl FnOnce(&Item, &Collection<artisan_api::Question>) -> Result<Element, E>,
) -> Result<Option<Element>, E> {
    let ItemKind::Question {
        questions,
        answers,
        answer_text,
        ..
    } = &item.kind
    else {
        return Ok(None);
    };
    if answer_text.is_some() {
        return answered(item, questions).map(Some);
    }
    let mut details = UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .gap(theme.spacing.steps(2.0));
    match questions {
        Collection::Inline(questions) => {
            for index in 0..questions.len() {
                details = details.child(source(SourceField::Question(index))?);
            }
        }
        Collection::Stored(source) => details = details.child(stored_questions(source)?),
    }
    let status = if item.status == Status::AwaitingUser {
        "Answer it above the message box."
    } else {
        "Not answered."
    };
    details = details.child(body(status, theme, true, false));
    let title = match answers {
        None => "Question",
        Some(Collection::Inline(answers)) if answers.iter().all(Option::is_none) => {
            "Skipped question"
        }
        Some(_) => "Answered question",
    };
    Ok(Some(card(theme, title, details.build())))
}

pub fn answered<E>(
    questions: &Collection<artisan_api::Question>,
    theme: ArtisanTheme,
    mut source: impl FnMut(SourceField) -> Result<Element, E>,
    stored: impl FnOnce(&artisan_api::CollectionRef) -> Result<Element, E>,
    answer: impl FnOnce() -> Result<Element, E>,
) -> Result<Element, E> {
    let mut asked = UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .gap(theme.spacing.steps(1.0))
        .style(
            Style::new()
                .min_width(Length::Px(0.0))
                .gpui_source_text_color(theme.colors.text_muted.to_reference_text_color()),
        );
    match questions {
        Collection::Inline(questions) => {
            for index in 0..questions.len() {
                asked = asked.child(source(SourceField::Question(index))?);
            }
        }
        Collection::Stored(reference) => asked = asked.child(stored(reference)?),
    }
    let content = UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .style(Style::new().min_width(Length::Px(0.0)))
        .gap(theme.spacing.steps(2.0))
        .child(asked.build())
        .child(answer()?)
        .build();
    let bubble = editor_ui::chat_bubble::ChatBubbleSegment::new(
        theme,
        editor_ui::chat_bubble::SegmentPosition::Single,
        content,
    )
    .answer()
    .build();
    Ok(UiBox::new()
        .layout("column")
        .width(Length::Pct(100.0))
        .style(Style::new().align(aqua_ui::AlignItems::END))
        .child(bubble)
        .build())
}

pub fn question_theme(item: &Item, mut theme: ArtisanTheme) -> ArtisanTheme {
    if matches!(
        &item.kind,
        ItemKind::Question {
            answer_text: Some(_),
            ..
        }
    ) {
        theme.colors.text = theme.colors.text_muted;
    }
    theme
}
