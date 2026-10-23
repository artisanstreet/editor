//! Agent rows: the live status line in an agent's own view, and the work
//! row that says what was delegated and opens the agent's view.
use super::super::LINE;
use super::{Action, Actions};
use crate::phase::{self, Phase};
use artisan_api::{Item, ItemKind, TextContent};
use editor_ui::{
    shimmer_text::{ShimmerText, ShimmerTone},
    theme::{ArtisanTheme, ProseTypography},
};
use gpui::{
    AnyElement, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, div, px,
};
pub(in crate::row) struct AgentPresenter {
    pub theme: ArtisanTheme,
    pub actions: Actions,
}
impl AgentPresenter {
    pub(in crate::row) fn agent_status_row(&self, working: bool) -> Option<AnyElement> {
        let theme = self.theme;
        let muted = theme.colors.text_muted.to_paint();
        let copy = phase::status_copy(if working {
            Phase::Working
        } else {
            Phase::Thinking
        })?;
        Some(
            div()
                .w_full()
                .min_w_0()
                .flex()
                .flex_row()
                .items_center()
                .text_size(px(ProseTypography::BODY_SIZE_PX))
                .line_height(px(ProseTypography::BODY_LINE_PX))
                .font_weight(ProseTypography::BODY_WEIGHT)
                .tracking(theme.typography.tracking_em)
                .text_color(muted)
                .child(
                    ShimmerText::new("agent-status", copy, theme)
                        .active(true)
                        .tone(ShimmerTone::Muted)
                        .truncate()
                        .into_any_element(),
                )
                .into_any_element(),
        )
    }

    pub(in crate::row) fn agent_row(&self, item: &Item) -> AnyElement {
        let theme = self.theme;
        let ItemKind::Agent { description, .. } = &item.kind else {
            return div().into_any_element();
        };
        let muted = theme.colors.text_muted.to_paint();
        let task: SharedString = match description {
            TextContent::Inline(text) => text.lines().next().unwrap_or_default().to_owned().into(),
            TextContent::Stored(_) => "Stored task. Open the agent to read it.".into(),
        };
        let id = item.id.clone();
        let open = self.actions.handler(Action::OpenAgent(id));
        div()
            .id(SharedString::from(format!("agent-{}", item.id)))
            .debug_selector(|| "work-agent".to_owned())
            .w_full()
            .min_w_0()
            .py(theme.spacing.steps(0.5))
            .flex()
            .flex_row()
            .items_center()
            .gap(theme.spacing.steps(2.0))
            .cursor_pointer()
            .text_size(px(ProseTypography::BODY_SIZE_PX))
            .line_height(px(LINE))
            .font_weight(ProseTypography::BODY_WEIGHT)
            .tracking(theme.typography.tracking_em)
            .text_color(muted)
            .child(
                div()
                    .flex_shrink_0()
                    .text_color(theme.colors.text.to_paint())
                    .child("Delegated"),
            )
            .child(div().flex_1().min_w_0().truncate().child(task.clone()))
            .children(crate::row::test_mark(format!(
                "work-agent-text:Delegated {task}"
            )))
            .on_click(move |_, _, cx| open(cx))
            .into_any_element()
    }
}
