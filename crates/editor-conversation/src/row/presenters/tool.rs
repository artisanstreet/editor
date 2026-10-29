//! Tool activity summaries and plan presentation. Only agent messages disclose their text.
use super::super::{LINE, body_text};
use super::request::decision_note;
use super::text::TextPresenter;
use super::{Action, Actions};
use crate::activity;
use artisan_api::{Item, ItemKind, Status};
use editor_ui::theme::ProseTypography;
use gpui::{
    AnyElement, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, div, prelude::FluentBuilder as _, px,
};
use std::collections::HashMap;
pub(in crate::row) struct ToolPresenter<'a> {
    pub text: TextPresenter<'a>,
    pub actions: Actions,
}
impl ToolPresenter<'_> {
    pub(in crate::row) fn activity_row(
        &self,
        item: &Item,
        expanded: bool,
        agents: &HashMap<artisan_api::ItemId, String>,
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
        let error = failure(item);
        let has_details = error.is_some()
            || message
                .as_ref()
                .is_some_and(|message| message.text.byte_len() > 0);
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
        match error.filter(|_| expanded) {
            Some(error) => div()
                .w_full()
                .min_w_0()
                .flex()
                .flex_col()
                .child(sentence)
                .child(
                    div()
                        .w_full()
                        .min_w_0()
                        .pb(theme.spacing.steps(1.0))
                        .text_size(theme.typography.control_text)
                        .whitespace_normal()
                        .text_color(theme.colors.destructive_text.to_paint())
                        .child(error),
                )
                .into_any_element(),
            None => sentence.into_any_element(),
        }
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

/// What a failed call answered: the reason it failed, shown under its row
/// when opened.
fn failure(item: &Item) -> Option<String> {
    let ItemKind::Tool {
        output: Some(output),
        ..
    } = &item.kind
    else {
        return None;
    };
    (item.status == Status::Failed)
        .then(|| output.text.inline())
        .flatten()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_owned)
}
