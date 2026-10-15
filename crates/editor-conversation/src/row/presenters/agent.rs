//! Agent rows consume canonical labels and status; navigation stays a typed action.
use super::super::LINE;
use super::{Action, Actions};
use crate::phase::{self, Phase};
use artisan_api::{Item, ItemKind, Status, TextContent};
use editor_ui::{
    AssetId,
    asset_seam::asset_glyph,
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
        let muted = theme.colors.muted_foreground.to_paint();
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
                .tracking(ProseTypography::TRACKING_EM)
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
        let muted = theme.colors.muted_foreground.to_paint();
        let state = match item.status {
            Status::Running | Status::AwaitingUser => "Working",
            Status::Done => "Done",
            Status::Failed => "Failed",
            Status::Cancelled | Status::Declined => "Stopped",
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
            .tracking(ProseTypography::TRACKING_EM)
            .text_color(muted)
            .child(
                asset_glyph(AssetId::TABLER_BOT_ID)
                    .size(px(16.0))
                    .flex_none(),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_color(theme.colors.foreground.to_paint())
                    .child(item.agent_label().unwrap_or_default()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .child(match description {
                        TextContent::Inline(text) => {
                            text.lines().next().unwrap_or_default().to_owned()
                        }
                        TextContent::Stored(_) => "Stored task. Open the agent to read it.".into(),
                    }),
            )
            .child(div().flex_shrink_0().child(state))
            .on_click(move |_, _, cx| open(cx))
            .into_any_element()
    }
}
