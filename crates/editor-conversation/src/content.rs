//! Mounted authored source fields. Core owns bounded raw paging, requests and export;
//! this module owns field identity and reuse across transcript Item revisions.
use crate::fold::{PlainSection, TextSection};
use artisan_api::{ApprovalRequest, Item, ItemId, ItemKind, TextContent, ToolKind};
use editor_ui::theme::ArtisanTheme;
use gpui::{
    App, AppContext as _, IntoElement, ParentElement as _, RenderOnce, WeakEntity, Window, div,
};
use std::{cell::RefCell, collections::HashMap, rc::Rc};

pub(crate) type SourceOwners = Rc<
    RefCell<
        HashMap<(ItemId, SourceField), (ArtisanTheme, gpui::Entity<editor_sources::ContentText>)>,
    >,
>;

/// A source field has stable identity across Item revisions. Changes replace
/// its Core source window immediately, cancelling old requests before repaint.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum SourceField {
    Plain(PlainSection),
    Text(TextSection),
    Prose,
    Notice,
    Command,
    Output,
    Input,
    Diff(usize),
    PlanStep(usize),
    Question(usize),
    AnswerText,
    ApprovalCommand,
    ApprovalDescription,
    ApprovalReason,
}
impl SourceField {
    pub(crate) fn of_slot(slot: crate::fold::Slot) -> Self {
        match slot {
            crate::fold::Slot::Prose => Self::Prose,
            crate::fold::Slot::Plain(section) => Self::Plain(section),
            crate::fold::Slot::Text(section) => Self::Text(section),
            crate::fold::Slot::Output | crate::fold::Slot::Preview => Self::Output,
            crate::fold::Slot::Answer => Self::AnswerText,
        }
    }
    pub(crate) fn text<'a>(&self, item: &'a Item) -> Option<&'a TextContent> {
        match self {
            Self::Plain(section) => section.text(item),
            Self::Text(section) => section.text(item),
            Self::Prose => match &item.kind {
                ItemKind::AssistantMessage { text, .. } => Some(text),
                _ => None,
            },
            Self::Notice => match &item.kind {
                ItemKind::Notice { text, .. } => Some(text),
                _ => None,
            },
            Self::Command => match &item.kind {
                ItemKind::Tool {
                    tool: ToolKind::Shell { command, .. },
                    ..
                } => Some(command),
                _ => None,
            },
            Self::Output => match &item.kind {
                ItemKind::Tool {
                    output: Some(output),
                    ..
                } => Some(&output.text),
                _ => None,
            },
            Self::Input => match &item.kind {
                ItemKind::Tool {
                    tool: ToolKind::Mcp { input, .. } | ToolKind::Other { input, .. },
                    ..
                } => input.as_ref(),
                _ => None,
            },
            Self::Diff(index) => match &item.kind {
                ItemKind::Tool {
                    tool: ToolKind::Edit { changes },
                    ..
                } => changes.inline()?.get(*index)?.diff.as_ref(),
                _ => None,
            },
            Self::PlanStep(index) => match &item.kind {
                ItemKind::Plan { steps, .. } => steps.inline()?.get(*index).map(|step| &step.text),
                _ => None,
            },
            Self::Question(index) => match &item.kind {
                ItemKind::Question { questions, .. } => questions
                    .inline()?
                    .get(*index)
                    .map(|question| &question.text),
                _ => None,
            },
            Self::AnswerText => match &item.kind {
                ItemKind::Question { answer_text, .. } => answer_text.as_ref(),
                _ => None,
            },
            Self::ApprovalCommand | Self::ApprovalDescription | Self::ApprovalReason => {
                let approval = match &item.kind {
                    ItemKind::Tool { approval, .. } | ItemKind::Plan { approval, .. } => {
                        approval.as_ref()
                    }
                    _ => None,
                }?;
                match (self, &approval.request) {
                    (Self::ApprovalReason, _) => approval.reason.as_ref(),
                    (Self::ApprovalCommand, ApprovalRequest::Command { command, .. }) => {
                        Some(command)
                    }
                    (Self::ApprovalDescription, ApprovalRequest::Action { description }) => {
                        Some(description)
                    }
                    _ => None,
                }
            }
        }
    }
}

/// One mounted authored field. The shared Core owner handles paging, cancellation,
/// raw copy and export; the transcript never treats a source reference as empty text.
#[derive(gpui::IntoElement)]
pub(crate) struct StoredText {
    pub(crate) host: Option<WeakEntity<editor_core::HostData>>,
    pub(crate) source: artisan_api::ContentRef,
    pub(crate) theme: ArtisanTheme,
    pub(crate) key: (ItemId, SourceField),
    pub(crate) owners: SourceOwners,
}
impl RenderOnce for StoredText {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let Some(host) = self.host.and_then(|host| host.upgrade()) else {
            return div()
                .child("This Forge is no longer connected.")
                .into_any_element();
        };
        let mut owners = self.owners.borrow_mut();
        let (theme, owner) = owners.entry(self.key).or_insert_with(|| {
            (
                self.theme,
                cx.new(|cx| {
                    editor_sources::ContentText::new(&host, self.source.clone(), self.theme, cx)
                }),
            )
        });
        owner.update(cx, |owner, cx| {
            if owner.source(cx) != &self.source {
                owner.set_source(self.source, cx);
            }
            if *theme != self.theme {
                owner.set_theme(self.theme, cx);
            }
        });
        *theme = self.theme;
        owner.clone().into_any_element()
    }
}
