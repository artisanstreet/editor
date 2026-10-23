//! Pointer-anchored menus with shared overlay focus and dismissal.

use crate::asset_seam::asset_glyph;
use crate::glass::{
    GlassStrength, glass_blur_radius, glass_card_shadows, glass_foreground_base,
    glass_highlight_layer, glass_material_layer,
};
use crate::hover_card::HoverTone;
use crate::motion::MotionDuration;
use crate::picker_motion::{HoverSurface, smooth_out};
use crate::theme::{RadiusStep, RadiusTokens, TextStep, TypographyTokens};
use crate::{
    overlay,
    theme::{ActiveTheme as _, ArtisanTheme},
};
use editor_assets::AssetId;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    Animation, AnimationExt as _, AnyElement, App, Div, InteractiveElement as _, IntoElement,
    ParentElement, Pixels, Point, RenderOnce, SharedString, Stateful,
    StatefulInteractiveElement as _, Styled, Window, anchored, deferred, div, point, px,
};
use std::rc::Rc;

type Dismiss = Rc<dyn Fn(&mut Window, &mut App)>;

/// Where a pointer menu's entrance starts growing from.
const MENU_PRE_SCALE: f32 = 0.97;

/// A pointer menu's rows over `hover`'s sliding pill. Clear `hover` when
/// the menu opens, so the pill is placed rather than flown from the last
/// menu.
#[must_use]
pub fn menu_rows(hover: &HoverSurface, theme: &ArtisanTheme, reduce_motion: bool) -> Div {
    div()
        .relative()
        .flex()
        .flex_col()
        .child(hover.surface_probe())
        .child(hover.pill(
            "menu-hover",
            theme,
            RadiusTokens::value(RadiusStep::LgMid),
            reduce_motion,
        ))
}

/// A pointer-menu row in [`menu_rows`]: a 16 px icon slot (left empty to
/// indent a row under its iconed siblings), then the label. Destructive
/// rows are red, and so is the pill under them.
#[must_use]
pub fn menu_item(
    id: impl Into<SharedString>,
    theme: &ArtisanTheme,
    hover: &HoverSurface,
    icon: Option<AssetId>,
    label: impl Into<SharedString>,
    destructive: bool,
) -> Stateful<Div> {
    let id = id.into();
    let label = label.into();
    let theme = *theme;
    let foreground = if destructive {
        theme.colors.destructive_text.to_paint()
    } else {
        theme.colors.text.to_paint()
    };
    let selector = id.clone();
    let hover_id = id.to_string();
    let tone = if destructive {
        HoverTone::Destructive
    } else {
        HoverTone::Neutral
    };
    let probe = hover.row_probe(hover_id.clone());
    let hover = hover.clone();
    div()
        .id(id)
        .role(gpui::Role::MenuItem)
        .aria_label(label.clone())
        .focusable()
        .tab_index(0)
        .debug_selector(move || selector.to_string())
        .relative()
        .px(px(12.0))
        .py(px(8.0))
        .flex()
        .items_center()
        .gap(px(8.0))
        .rounded(RadiusTokens::value(RadiusStep::LgMid))
        .cursor_pointer()
        .on_hover(move |hovered, window, _| {
            hover.hovered_toned(&hover_id, *hovered, tone);
            window.refresh();
        })
        .child(probe)
        .child(div().size(px(16.0)).flex_none().children(icon.map(|icon| {
            asset_glyph(icon).size(px(15.0)).text_color(if destructive {
                foreground
            } else {
                theme.colors.text_muted.to_paint()
            })
        })))
        .child(
            div()
                .text_size(TypographyTokens::size(TextStep::Sm))
                .line_height(px(20.0))
                .whitespace_nowrap()
                .text_color(foreground)
                .child(label),
        )
}

#[derive(IntoElement)]
pub struct Popover {
    id: SharedString,
    label: SharedString,
    position: Point<Pixels>,
    content: AnyElement,
    dismiss: Dismiss,
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
    pub const fn menu_padding(mut self, padding: Pixels) -> Self {
        self.menu_padding = padding;
        self
    }
    #[must_use]
    pub const fn quiet_glass_menu(mut self) -> Self {
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
        let close: Dismiss = Rc::new(move |window, cx| {
            overlay::dismiss(&id, window, cx);
            dismiss(window, cx);
        });
        let pointer_close = Rc::clone(&close);
        let keys = focus.clone();
        let theme = *cx.theme();
        let reduce_motion = cx.reduce_motion();
        let entrance_id = SharedString::from(format!("{root_id}-entrance"));
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
        // Opens below and right of the pointer, so it grows from its
        // top-left corner (the transitions-dev dropdown).
        let panel = if reduce_motion {
            panel.into_any_element()
        } else {
            panel
                .with_animation(
                    entrance_id,
                    Animation::new(MotionDuration::Fast.as_duration()).with_easing(smooth_out),
                    |panel, progress| {
                        panel.opacity(progress).transform(
                            gpui::ElementTransform::scale(
                                (1.0 - MENU_PRE_SCALE).mul_add(progress, MENU_PRE_SCALE),
                            )
                            .with_origin(point(0.0, 0.0)),
                        )
                    },
                )
                .into_any_element()
        };
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
    pub const fn menu_padding(mut self, padding: Pixels) -> Self {
        self.menu_padding = padding;
        self
    }
    #[must_use]
    pub const fn quiet_glass_menu(mut self) -> Self {
        self.quiet_glass = true;
        self
    }
    /// The settings picker menu keeps its original padding-free rounded frame.
    #[must_use]
    pub const fn select_menu(mut self) -> Self {
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
                    .rounded(RadiusTokens::value(RadiusStep::Lg))
                    .border_1()
                    .border_color(theme.colors.line.to_paint())
            })
            .when(self.select_menu, |root| {
                root.rounded(crate::theme::RadiusTokens::value(
                    crate::theme::RadiusStep::X2l,
                ))
                .shadow(theme.elevation.shadows(crate::theme::Elevation::Overlay))
                .overflow_hidden()
            })
            .when(!self.quiet_glass, |root| {
                root.bg(theme.colors.raised.to_paint())
            })
            .when(self.quiet_glass, |root| {
                root.rounded(radius)
                    .backdrop_blur(glass_blur_radius(GlassStrength::Quiet))
                    .bg(glass_foreground_base(&theme))
                    .shadow(glass_card_shadows(&theme, crate::theme::Elevation::Overlay))
                    .child(glass_material_layer(&theme, GlassStrength::Quiet, radius))
                    .child(glass_highlight_layer(&theme, GlassStrength::Quiet, radius))
            })
            .text_color(theme.colors.text.to_paint())
            .child(self.content);
        if let Some(selector) = self.selector {
            root = root.debug_selector(move || selector.to_string());
        }
        root
    }
}
/// The glass-menu variant of the popover chrome.
///
/// A strong-glass column whose corners follow `radius`. Model, project and option menus and
/// mention completion are built on it; the caller sizes it, routes its
/// keys, and adds rows.
#[must_use]
pub fn glass_menu(
    id: impl Into<SharedString>,
    theme: &ArtisanTheme,
    radius: Pixels,
) -> gpui::Stateful<gpui::Div> {
    glass_frame(div().id(id.into()), theme, GlassStrength::Strong, radius)
        .occlude()
        .flex()
        .flex_col()
        .text_color(theme.colors.text.to_paint())
}

/// The glass surface every glass popup shares: backdrop blur, the foreground
/// base, the card shadows, then the material and highlight layers.
pub(crate) fn glass_frame<E: Styled + ParentElement>(
    element: E,
    theme: &ArtisanTheme,
    strength: GlassStrength,
    radius: Pixels,
) -> E {
    element
        .relative()
        .overflow_hidden()
        .rounded(radius)
        .backdrop_blur(glass_blur_radius(strength))
        .bg(glass_foreground_base(theme))
        .shadow(glass_card_shadows(theme, crate::theme::Elevation::Overlay))
        .child(glass_material_layer(theme, strength, radius))
        .child(glass_highlight_layer(theme, strength, radius))
}

/// Places `content` above the element this is a child of, `gap` away, its
/// left edge on the trigger's and kept 8 px inside the window. Popups paint
/// in `priority` order.
#[must_use]
pub fn above_trigger(gap: Pixels, priority: usize, content: impl IntoElement) -> gpui::Div {
    div().absolute().top_0().left_0().child(
        deferred(
            anchored()
                .anchor(gpui::Anchor::BottomLeft)
                .offset(point(px(0.0), -gap))
                .snap_to_window_with_margin(px(8.0))
                .child(content),
        )
        .with_priority(priority),
    )
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
                    ArtisanTheme::resolved(),
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
