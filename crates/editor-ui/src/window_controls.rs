//! Native window controls own caption geometry, focus and platform actions.
use crate::{AssetId, asset_seam::asset_glyph, theme::ArtisanTheme};
use gpui::{
    App, Div, Hsla, InteractiveElement as _, IntoElement, ParentElement as _, Pixels, RenderOnce,
    Stateful, StatefulInteractiveElement as _, Styled as _, Window, WindowControlArea, div, px,
};

pub const CONTROL_WIDTH: f32 = 46.0;
#[derive(IntoElement)]
pub struct WindowControls {
    theme: ArtisanTheme,
    maximized: bool,
}
impl WindowControls {
    pub fn new(theme: ArtisanTheme, maximized: bool) -> Self {
        Self { theme, maximized }
    }
}
impl RenderOnce for WindowControls {
    fn render(self, window: &mut Window, _: &mut App) -> impl IntoElement {
        let stroke = px(1.0 / window.scale_factor());
        div()
            .flex()
            .h_full()
            .flex_shrink_0()
            .child(caption_button(&self.theme, Caption::Minimize, stroke))
            .child(caption_button(
                &self.theme,
                Caption::Maximize(self.maximized),
                stroke,
            ))
            .child(caption_button(&self.theme, Caption::Close, stroke))
    }
}
#[derive(Clone, Copy)]
enum Caption {
    Minimize,
    Maximize(bool),
    Close,
}
impl Caption {
    fn area(self) -> WindowControlArea {
        match self {
            Self::Minimize => WindowControlArea::Min,
            Self::Maximize(_) => WindowControlArea::Max,
            Self::Close => WindowControlArea::Close,
        }
    }
    fn selector(self) -> &'static str {
        match self {
            Self::Minimize => "artisan-desktop-titlebar-minimize",
            Self::Maximize(_) => "artisan-desktop-titlebar-maximize",
            Self::Close => "artisan-desktop-titlebar-close",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Minimize => "Minimize window",
            Self::Maximize(false) => "Maximize window",
            Self::Maximize(true) => "Restore window",
            Self::Close => "Close window",
        }
    }
}
/// The platform handles pointer input in caption areas; keyboard clicks use
/// the same action and geometry for all three controls.
fn caption_button(theme: &ArtisanTheme, caption: Caption, stroke: Pixels) -> Stateful<Div> {
    let (hover_background, hover_foreground): (Hsla, Hsla) = match caption {
        Caption::Close => (
            gpui::rgb_to_hsla(gpui::rgb(0x00e8_1123)),
            gpui::rgb_to_hsla(gpui::rgb(0x00ff_ffff)),
        ),
        Caption::Maximize(_) => (theme.workspace.selected, theme.workspace.foreground),
        _ => (
            theme.colors.muted.with_alpha(0.5).to_paint(),
            theme.colors.foreground.to_paint(),
        ),
    };
    let glyph = match caption {
        Caption::Maximize(maximized) => maximize_glyph(theme, maximized, stroke).into_any_element(),
        Caption::Minimize => asset_glyph(AssetId::TABLER_MINUS)
            .size(px(16.0))
            .into_any_element(),
        Caption::Close => asset_glyph(AssetId::TABLER_X)
            .size(px(16.0))
            .into_any_element(),
    };
    let focus_ring = theme.interaction.focus_ring_color.to_paint();
    let selector = caption.selector();
    div()
        .id(selector)
        .focusable()
        .tab_index(0)
        .role(gpui::Role::Button)
        .aria_label(caption.label())
        .border_1()
        .border_color(gpui::transparent_black())
        .focus_visible(move |style| style.border_color(focus_ring))
        .debug_selector(move || selector.to_owned())
        .w(px(CONTROL_WIDTH))
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(theme.colors.muted_foreground.to_paint())
        .hover(move |style| style.bg(hover_background).text_color(hover_foreground))
        .window_control_area(caption.area())
        .on_click(move |event, window, _| {
            if !event.is_keyboard() {
                return;
            }
            match caption {
                Caption::Minimize => window.minimize_window(),
                Caption::Maximize(_) => window.zoom_window(),
                Caption::Close => window.remove_window(),
            }
        })
        .child(glyph)
}
fn maximize_glyph(theme: &ArtisanTheme, maximized: bool, stroke: Pixels) -> Div {
    let mut glyph = div().relative().w(px(10.0)).h(px(10.0));
    if maximized {
        glyph = glyph
            .child(
                div()
                    .absolute()
                    .top_0()
                    .right_0()
                    .w(px(7.0))
                    .h(px(7.0))
                    .border_t(stroke)
                    .border_r(stroke)
                    .border_color(theme.workspace.foreground),
            )
            .child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .w(px(7.0))
                    .h(px(7.0))
                    .border(stroke)
                    .border_color(theme.workspace.foreground),
            );
    } else {
        glyph = glyph
            .border(stroke)
            .border_color(theme.workspace.foreground);
    }
    glyph
}
