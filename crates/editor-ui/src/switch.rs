//! A controlled switch with component-owned focus, geometry and semantics.

use crate::theme::ArtisanTheme;
use gpui::prelude::FluentBuilder;
use gpui::{
    App, BoxShadow, ClickEvent, ElementId, InteractiveElement, IntoElement, ParentElement,
    RenderOnce, Role, SharedString, StatefulInteractiveElement, Styled, Toggled, Window, div,
    point, px,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SwitchSize {
    #[default]
    Default,
    Small,
}

type Change = Box<dyn Fn(bool, &ClickEvent, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct Switch {
    id: ElementId,
    theme: ArtisanTheme,
    size: SwitchSize,
    checked: bool,
    label: SharedString,
    disabled: bool,
    on_change: Option<Change>,
    debug_selector: Option<SharedString>,
}

impl Switch {
    pub fn new(
        id: impl Into<ElementId>,
        theme: ArtisanTheme,
        size: SwitchSize,
        checked: bool,
        label: impl Into<SharedString>,
    ) -> Self {
        Self {
            id: id.into(),
            theme,
            size,
            checked,
            label: label.into(),
            disabled: false,
            on_change: None,
            debug_selector: None,
        }
    }
    #[must_use]
    pub const fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
    #[must_use]
    pub fn on_change(
        mut self,
        handler: impl Fn(bool, &ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_change = Some(Box::new(handler));
        self
    }
    #[must_use]
    pub fn debug_selector(mut self, selector: impl Into<SharedString>) -> Self {
        self.debug_selector = Some(selector.into());
        self
    }
}

impl RenderOnce for Switch {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let theme = self.theme;
        let (width, height, thumb) = match self.size {
            SwitchSize::Default => (
                theme.density.switch_default.0,
                theme.density.switch_default.1,
                px(16.0),
            ),
            SwitchSize::Small => (
                theme.density.switch_sm.0,
                theme.density.switch_sm.1,
                px(12.0),
            ),
        };
        let travel = width - thumb - px(2.0);
        let checked = self.checked;
        let mut track = div()
            .relative()
            .w(width)
            .h(height)
            .rounded_full()
            .flex()
            .items_center()
            .border_1()
            .border_color(gpui::transparent_black())
            .bg(if checked {
                theme.colors.primary.to_paint()
            } else {
                theme.colors.line_strong.to_paint()
            })
            .child(
                div()
                    .relative()
                    .left(if checked { travel } else { px(0.0) })
                    .size(thumb)
                    .rounded_full()
                    .bg(if checked {
                        theme.colors.primary_text.to_paint()
                    } else {
                        theme.colors.text.to_paint()
                    }),
            );
        if let Some(selector) = &self.debug_selector {
            let selector = format!("{selector}-track");
            track = track.debug_selector(move || selector);
        }
        let mut root = div()
            .id(self.id)
            .role(Role::Switch)
            .aria_label(self.label)
            .aria_toggled(if checked {
                Toggled::True
            } else {
                Toggled::False
            })
            .flex()
            .items_center()
            .justify_center()
            .flex_shrink_0()
            .w(width + px(24.0))
            .h(height + px(16.0))
            .mx(px(-12.0))
            .my(px(-8.0))
            .rounded_full()
            .child(track)
            .when_some(self.debug_selector, |el, selector| {
                el.debug_selector(move || selector.to_string())
            });
        if self.disabled {
            return root
                .aria_description("Unavailable")
                .opacity(crate::theme::Opacity::Disabled.alpha());
        }
        root = root
            .focusable()
            .tab_index(0)
            .cursor_pointer()
            .focus_visible(move |style| {
                style.shadow(vec![BoxShadow {
                    color: theme
                        .colors
                        .ring
                        .at(crate::theme::Opacity::FocusRing)
                        .to_paint(),
                    offset: point(px(0.0), px(0.0)),
                    blur_radius: px(0.0),
                    spread_radius: theme.interaction.focus_ring_width,
                    inset: false,
                }])
            });
        if let Some(handler) = self.on_change {
            root = root.on_click(move |event, window, cx| handler(!checked, event, window, cx));
        }
        root
    }
}
