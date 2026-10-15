//! Pointer-anchored menus with shared overlay focus and dismissal.

use crate::{overlay, theme::ArtisanTheme};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, InteractiveElement as _, IntoElement, ParentElement as _, Pixels, Point,
    RenderOnce, SharedString, StatefulInteractiveElement as _, Styled as _, Window, anchored,
    deferred, div, px,
};
use std::{cell::Cell, rc::Rc};

#[derive(IntoElement)]
pub struct Popover {
    id: SharedString,
    label: SharedString,
    position: Point<Pixels>,
    content: AnyElement,
    dismiss: Rc<dyn Fn(&mut Window, &mut App)>,
    menu_padding: Pixels,
    quiet_glass: bool,
}
impl Popover {
    pub fn new(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        position: Point<Pixels>,
        content: impl IntoElement,
        dismiss: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            position,
            content: content.into_any_element(),
            dismiss: Rc::new(dismiss),
            menu_padding: px(6.0),
            quiet_glass: false,
        }
    }
    #[must_use]
    pub fn menu_padding(mut self, padding: Pixels) -> Self {
        self.menu_padding = padding;
        self
    }
    #[must_use]
    pub fn quiet_glass_menu(mut self) -> Self {
        self.quiet_glass = true;
        self
    }
}
impl RenderOnce for Popover {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focus = overlay::begin(self.id.clone(), window, cx);
        overlay::retain(self.id.clone(), window, cx);
        let priority = overlay::priority(&self.id, window, cx);
        let root_id = self.id.clone();
        let id = self.id;
        let dismiss = self.dismiss;
        let close: Rc<dyn Fn(&mut Window, &mut App)> = Rc::new(move |window, cx| {
            overlay::dismiss(&id, window, cx);
            dismiss(window, cx);
        });
        let pointer_close = close.clone();
        let keys = focus.clone();
        let theme = ArtisanTheme::default();
        let panel = div()
            .id(root_id)
            .role(gpui::Role::Menu)
            .aria_label(self.label)
            .track_focus(&focus)
            .tab_group()
            .tab_stop(false)
            .on_mouse_down_out(move |_, window, cx| pointer_close(window, cx))
            .on_key_down(move |event, window, cx| {
                match event.keystroke.key.as_str() {
                    "escape" | "tab" => close(window, cx),
                    "down" => overlay::cycle_focus(&keys, false, window, cx),
                    "up" => overlay::cycle_focus(&keys, true, window, cx),
                    _ => return,
                }
                cx.stop_propagation();
            })
            .child(
                PopoverPanel::new("panel", theme, self.content)
                    .menu_padding(self.menu_padding)
                    .when(self.quiet_glass, PopoverPanel::quiet_glass_menu),
            );
        deferred(
            anchored()
                .position(self.position)
                .snap_to_window_with_margin(px(8.0))
                .child(panel),
        )
        .with_priority(priority)
    }
}

/// Nonmodal chrome shared by menus, Select and mention completion. Caller
/// wrappers own placement and keyboard routing; this owns the scroll frame.
#[derive(IntoElement)]
pub struct PopoverPanel {
    id: gpui::ElementId,
    theme: ArtisanTheme,
    content: AnyElement,
    selector: Option<SharedString>,
    select_menu: bool,
    menu_padding: Pixels,
    quiet_glass: bool,
}
impl PopoverPanel {
    pub fn new(
        id: impl Into<gpui::ElementId>,
        theme: ArtisanTheme,
        content: impl IntoElement,
    ) -> Self {
        Self {
            id: id.into(),
            theme,
            content: content.into_any_element(),
            selector: None,
            select_menu: false,
            menu_padding: px(6.0),
            quiet_glass: false,
        }
    }
    #[must_use]
    pub fn menu_padding(mut self, padding: Pixels) -> Self {
        self.menu_padding = padding;
        self
    }
    #[must_use]
    pub fn quiet_glass_menu(mut self) -> Self {
        self.quiet_glass = true;
        self
    }
    /// The settings picker menu keeps its original padding-free rounded frame.
    #[must_use]
    pub fn select_menu(mut self) -> Self {
        self.select_menu = true;
        self
    }
    #[must_use]
    pub fn debug_selector(mut self, selector: impl Into<SharedString>) -> Self {
        self.selector = Some(selector.into());
        self
    }
}
impl RenderOnce for PopoverPanel {
    fn render(self, window: &mut Window, _: &mut App) -> impl IntoElement {
        let theme = self.theme;
        let radius = crate::theme::RadiusTokens::value(crate::theme::RadiusStep::X2l);
        let mut root = div()
            .id(self.id)
            .relative()
            .occlude()
            .max_w((window.viewport_size().width - px(16.)).max(px(1.)))
            .max_h((window.viewport_size().height - px(16.)).max(px(1.)))
            .overflow_y_scroll()
            .when(!self.select_menu, |root| {
                root.p(self.menu_padding)
                    .rounded(px(10.))
                    .border_1()
                    .border_color(theme.colors.border.to_paint())
            })
            .when(self.select_menu, |root| {
                root.rounded(crate::theme::RadiusTokens::value(
                    crate::theme::RadiusStep::X2l,
                ))
                .shadow(
                    theme
                        .elevation
                        .menu_shadow
                        .map(crate::theme::ShadowLayer::to_box_shadow)
                        .to_vec(),
                )
                .overflow_hidden()
            })
            .when(!self.quiet_glass, |root| {
                root.bg(theme.colors.popover.to_paint())
            })
            .when(self.quiet_glass, |root| {
                use crate::glass::{
                    GlassStrength, glass_blur_radius, glass_card_shadows, glass_foreground_base,
                    glass_highlight_layer, glass_material_layer,
                };
                root.rounded(radius)
                    .backdrop_blur(glass_blur_radius(GlassStrength::Quiet))
                    .bg(glass_foreground_base(&theme))
                    .shadow(glass_card_shadows())
                    .child(glass_material_layer(GlassStrength::Quiet, radius))
                    .child(glass_highlight_layer(GlassStrength::Quiet, radius))
            })
            .text_color(theme.colors.popover_foreground.to_paint())
            .child(self.content);
        if let Some(selector) = self.selector {
            root = root.debug_selector(move || selector.to_string());
        }
        root
    }
}
/// Trigger-owned popups have two disjoint hit regions. The popup records its
/// actual panel bounds, so dismissal never depends on a list's scroll internals.
#[derive(Clone, Default)]
pub struct PopupBounds(Rc<Cell<Option<gpui::Bounds<Pixels>>>>);
impl PopupBounds {
    pub fn contains(&self, position: &Point<Pixels>) -> bool {
        self.0.get().is_some_and(|bounds| bounds.contains(position))
    }
    pub fn recorder(&self) -> impl IntoElement {
        let bounds = self.0.clone();
        gpui::canvas(
            move |current, _, _| bounds.set(Some(current)),
            |_, (), _, _| {},
        )
        .absolute()
        .size_full()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, Render, TestAppContext};

    struct PanelGallery;
    impl Render for PanelGallery {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().size_full().flex().flex_col().items_start().child(
                PopoverPanel::new(
                    "panel",
                    ArtisanTheme::default(),
                    div().w(px(166.0)).h(px(1000.0)),
                )
                .debug_selector("bounded-panel-test"),
            )
        }
    }

    #[gpui::test]
    fn tall_popup_content_stays_within_the_viewport(cx: &mut TestAppContext) {
        let (_, cx) = cx.add_window_view(|_, _| PanelGallery);
        cx.simulate_resize(gpui::size(px(240.0), px(180.0)));
        let bounds = cx.debug_bounds("bounded-panel-test").expect("panel");
        assert_eq!(bounds.size.width, px(180.0));
        assert!(bounds.size.height > px(0.0));
        assert!(bounds.size.height <= px(164.0));
    }
}
