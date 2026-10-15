//! A keyboard-accessible navigation or selection row with shared geometry.
use crate::theme::{ArtisanTheme, RadiusStep, RadiusTokens};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, ClickEvent, ElementId, InteractiveElement as _, IntoElement,
    ParentElement as _, RenderOnce, SharedString, StatefulInteractiveElement as _, Styled as _,
    Window, div, px,
};
type Activation = Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>;
#[derive(IntoElement)]
pub struct ActionRow {
    id: ElementId,
    theme: ArtisanTheme,
    label: SharedString,
    secondary: Option<SharedString>,
    trailing: Option<AnyElement>,
    selected: bool,
    disabled: bool,
    activate: Option<Activation>,
}
impl ActionRow {
    /// The shared single-line row pitch for native collection viewports.
    pub fn row_height(theme: &ArtisanTheme) -> gpui::Pixels {
        theme
            .density
            .control_sm
            .max(theme.typography.control_text * 1.25 + px(12.0))
    }

    pub fn new(
        id: impl Into<ElementId>,
        theme: ArtisanTheme,
        label: impl Into<SharedString>,
    ) -> Self {
        Self {
            id: id.into(),
            theme,
            label: label.into(),
            secondary: None,
            trailing: None,
            selected: false,
            disabled: false,
            activate: None,
        }
    }
    #[must_use]
    pub fn secondary(mut self, value: impl Into<SharedString>) -> Self {
        self.secondary = Some(value.into());
        self
    }
    #[must_use]
    pub fn trailing(mut self, value: impl IntoElement) -> Self {
        self.trailing = Some(value.into_any_element());
        self
    }
    #[must_use]
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }
    #[must_use]
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }
    #[must_use]
    pub fn on_activate(
        mut self,
        callback: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.activate = Some(Box::new(callback));
        self
    }
}
impl RenderOnce for ActionRow {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let theme = self.theme;
        let mut root = div()
            .id(self.id)
            .role(gpui::Role::Button)
            .aria_label(self.label.clone())
            .aria_selected(self.selected)
            .w_full()
            .min_h(Self::row_height(&theme))
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(8.0))
            .py(px(6.0))
            .rounded(RadiusTokens::value(RadiusStep::Md))
            .border_1()
            .border_color(gpui::transparent_black())
            .text_color(theme.colors.foreground.to_paint())
            .text_size(theme.typography.control_text)
            .line_height(theme.typography.control_text * 1.25)
            .when(self.selected, |el| el.bg(theme.colors.muted.to_paint()))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(div().truncate().child(self.label))
                    .children(self.secondary.map(|text| {
                        div()
                            .truncate()
                            .text_xs()
                            .text_color(theme.colors.muted_foreground.to_paint())
                            .child(text)
                    })),
            )
            .children(
                self.trailing
                    .map(|child| div().flex_shrink_0().child(child)),
            );
        if self.disabled {
            return root.aria_description("Unavailable").opacity(0.5);
        }
        root = root
            .focusable()
            .tab_index(0)
            .cursor_pointer()
            .focus_visible(move |style| style.border_color(theme.colors.ring.to_paint()))
            .hover(move |style| style.bg(theme.colors.muted.with_alpha(0.5).to_paint()));
        if let Some(activate) = self.activate {
            root = root.on_click(activate);
        }
        root
    }
}
