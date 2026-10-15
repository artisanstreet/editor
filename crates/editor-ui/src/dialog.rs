//! Modal content composed through the per-window overlay owner.

use crate::theme::{RadiusStep, RadiusTokens, TextStep, TypographyTokens};
use crate::{
    AssetId,
    button::{Button, ButtonVariant},
    overlay,
    theme::ArtisanTheme,
};
use gpui::prelude::FluentBuilder;
use gpui::{
    AnyElement, App, ColorExt as _, FontWeight, InteractiveElement, IntoElement, ParentElement,
    Pixels, RenderOnce, Role, SharedString, StatefulInteractiveElement, Styled, Window, anchored,
    deferred, div, point, px,
};
use std::rc::Rc;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DialogDismissReason {
    Escape,
    Backdrop,
    CloseButton,
}
type Dismiss = Rc<dyn Fn(DialogDismissReason, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct Dialog {
    id: SharedString,
    theme: ArtisanTheme,
    title: SharedString,
    description: Option<SharedString>,
    content: AnyElement,
    on_dismiss: Option<Dismiss>,
    selector: Option<SharedString>,
    max_width: Pixels,
    dismissible: bool,
    bare: bool,
}

impl Dialog {
    pub fn new(
        id: impl Into<SharedString>,
        theme: ArtisanTheme,
        title: impl Into<SharedString>,
        content: impl IntoElement,
    ) -> Self {
        Self {
            id: id.into(),
            theme,
            title: title.into(),
            description: None,
            content: content.into_any_element(),
            on_dismiss: None,
            selector: None,
            max_width: px(560.0),
            dismissible: true,
            bare: false,
        }
    }
    #[must_use]
    pub fn dismissible(mut self, dismissible: bool) -> Self {
        self.dismissible = dismissible;
        self
    }
    /// Keeps modal focus/dismissal ownership while the content supplies its
    /// established card face, without an extra heading or padded frame.
    pub fn bare(mut self) -> Self {
        self.bare = true;
        self
    }
    #[must_use]
    pub fn max_width(mut self, width: Pixels) -> Self {
        self.max_width = width;
        self
    }
    #[must_use]
    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }
    #[must_use]
    pub fn debug_selector(mut self, selector: impl Into<SharedString>) -> Self {
        self.selector = Some(selector.into());
        self
    }
    #[must_use]
    pub fn on_dismiss(
        mut self,
        dismiss: impl Fn(DialogDismissReason, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_dismiss = Some(Rc::new(dismiss));
        self
    }
}

impl RenderOnce for Dialog {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focus = overlay::begin(self.id.clone(), window, cx);
        overlay::retain(self.id.clone(), window, cx);
        let priority = overlay::priority(&self.id, window, cx);
        let viewport = window.viewport_size();
        let theme = self.theme;
        let root_id = self.id.clone();
        let id = self.id;
        let dismissible = self.dismissible;
        let close: Dismiss = Rc::new(move |reason, window, cx| {
            if !dismissible {
                return;
            }
            overlay::dismiss(&id, window, cx);
            if let Some(handler) = &self.on_dismiss {
                handler(reason, window, cx);
            }
        });
        let key_close = close.clone();
        let button_close = close.clone();
        let panel = div()
            .id("dialog-panel")
            .role(Role::Dialog)
            .when_some(self.selector, |el, selector| {
                el.debug_selector(move || selector.to_string())
            })
            .aria_label(self.title.clone())
            .w(self.max_width.min((viewport.width - px(32.0)).max(px(1.0))))
            .max_h((viewport.height - px(32.0)).max(px(1.0)))
            .overflow_y_scroll()
            .when(!self.bare, |panel| {
                panel
                    .p(px(20.0))
                    .rounded(RadiusTokens::value(RadiusStep::XlMid))
                    .bg(theme.colors.popover.to_paint())
                    .border_1()
                    .border_color(theme.colors.border.to_paint())
            })
            .flex()
            .flex_col()
            .gap(px(16.0))
            .on_click(|_, _, cx| cx.stop_propagation())
            .when(!self.bare, |panel| {
                panel.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(12.0))
                        .child(
                            div()
                                .min_w_0()
                                .text_size(TypographyTokens::size(TextStep::Lg))
                                .font_weight(FontWeight::MEDIUM)
                                .child(self.title),
                        )
                        .child(
                            Button::icon("dialog-close", theme, AssetId::TABLER_X, "Close")
                                .variant(ButtonVariant::Ghost)
                                .disabled(!dismissible)
                                .on_activate(move |_, window, cx| {
                                    button_close(DialogDismissReason::CloseButton, window, cx)
                                }),
                        ),
                )
            })
            .when_some(self.description, |el, text| {
                el.child(crate::page::note(&theme, text))
            })
            .child(self.content);
        let key_focus = focus.clone();
        let root = div()
            .id(root_id)
            .w(viewport.width)
            .h(viewport.height)
            .flex()
            .items_center()
            .justify_center()
            .bg(if self.bare {
                gpui::hsla(0.0, 0.0, 0.0, 0.45)
            } else {
                theme.colors.background.to_paint().opacity(0.9)
            })
            .text_color(theme.colors.foreground.to_paint())
            .occlude()
            .track_focus(&focus)
            .tab_group()
            .tab_stop(false)
            .on_click(move |_, window, cx| close(DialogDismissReason::Backdrop, window, cx))
            .on_key_down(move |event, window, cx| {
                if event.keystroke.key == "escape" {
                    key_close(DialogDismissReason::Escape, window, cx);
                    cx.stop_propagation();
                } else if event.keystroke.key == "tab" {
                    overlay::cycle_focus(&key_focus, event.keystroke.modifiers.shift, window, cx);
                    cx.stop_propagation();
                }
            })
            .child(panel);
        deferred(anchored().position(point(px(0.0), px(0.0))).child(root)).with_priority(priority)
    }
}
