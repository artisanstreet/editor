//! Presentation-only dismissal for error notices. Closing never retries,
//! cancels, or deletes the failed operation or its saved prompt.

use artisan_ui::{
    button::{Button, ButtonContent, ButtonSize, ButtonVariant, FocusVisibility},
    motion::MotionPolicy,
    theme::ArtisanTheme,
};
use gpui::{AnyElement, App, ElementId, FocusHandle, Window, div, prelude::*, px};

#[derive(IntoElement)]
pub(crate) struct DismissibleNotice {
    id: String,
    content: AnyElement,
    theme: ArtisanTheme,
}

struct NoticeState {
    dismissed: bool,
    focus: FocusHandle,
}

impl DismissibleNotice {
    pub(crate) fn new(
        id: impl Into<String>,
        content: impl IntoElement,
        theme: ArtisanTheme,
    ) -> Self {
        Self {
            id: id.into(),
            content: content.into_any_element(),
            theme,
        }
    }
}

impl RenderOnce for DismissibleNotice {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = window.use_keyed_state(
            ElementId::Name(format!("{}-dismissal", self.id).into()),
            cx,
            |_, cx| NoticeState {
                dismissed: false,
                focus: cx.focus_handle().tab_stop(true),
            },
        );
        if state.read(cx).dismissed {
            return div().into_any_element();
        }
        let focus = state.read(cx).focus.clone();
        let close = Button::new(
            ElementId::Name(format!("{}-dismiss", self.id).into()),
            focus,
            self.theme,
            MotionPolicy::Reduced,
            ButtonVariant::Ghost,
            ButtonSize::Small,
            ButtonContent::text("Dismiss"),
        )
        .expect("a text dismiss button has valid content")
        .focus_visibility(FocusVisibility::Visible)
        .debug_selector(format!("{}-dismiss", self.id))
        .on_activate(move |_, _, cx| {
            state.update(cx, |state, cx| {
                state.dismissed = true;
                cx.notify();
            });
        });
        div()
            .w_full()
            .flex()
            .items_start()
            .gap(px(4.0))
            .child(div().flex_1().min_w_0().child(self.content))
            .child(close)
            .into_any_element()
    }
}
